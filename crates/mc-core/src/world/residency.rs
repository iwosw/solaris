//! Chunk admission, materialization, load errors and resident rejection.
//! Moved verbatim from `world.rs`.

use super::*;

impl ChunkWorld {
    /// Join finished off-tick loads for wanted chunks and request the
    /// missing ones, lowest priority value first (`want` maps position →
    /// load priority). Worker queues are bounded, so they never hold
    /// positions that went stale long ago. Never
    /// blocks; returns positions that became resident.
    pub fn ensure(&mut self, want: &HashMap<ChunkPos, i32>) -> Vec<ChunkPos> {
        self.ensure_with_clock(want, std::time::Instant::now)
    }

    pub(super) fn ensure_with_clock(
        &mut self,
        want: &HashMap<ChunkPos, i32>,
        mut now: impl FnMut() -> std::time::Instant,
    ) -> Vec<ChunkPos> {
        let admission_start = now();
        let phase_start = std::env::var_os("MC_CORE_PHASE_LOG")
            .is_some()
            .then(std::time::Instant::now);
        #[cfg(target_os = "linux")]
        let cpu_start = phase_start.map(|_| admission_thread_cpu_us());
        let missing = self.admission_candidates(want);
        let mut loaded = Vec::new();
        let mut limit = self.admission_limit();
        let mut batch_start = admission_start;
        let admission_end = loop {
            let previous = loaded.len();
            self.admit_ready_into(want, &missing, limit, false, &mut loaded);
            self.relight_admitted(&loaded[previous..], false);
            let batch_end = now();
            let admitted = loaded.len() - previous;
            self.record_admission_cost(
                batch_end.saturating_duration_since(batch_start).as_micros(),
                admitted,
            );
            // Keep the original progress batch, then spend real shared tick
            // slack one whole light fence at a time. An underestimated chunk
            // may overrun, but it cannot authorize another extra chunk.
            if admitted == 0
                || loaded.len() >= crate::chunk_loader::READY_LOADS_MAX
                || !self.admission_deadline.is_some_and(|deadline| {
                    deadline.saturating_duration_since(batch_end).as_micros()
                        >= self.admission_us_per_chunk
                })
            {
                break batch_end;
            }
            limit = 1;
            batch_start = batch_end;
        };
        self.admission_tick_elapsed += admission_end.saturating_duration_since(admission_start);
        if let Some(start) = phase_start
            && !loaded.is_empty()
        {
            eprintln!(
                "chunk admission ms: loaded={} ready={} pending={} total={:.2}",
                loaded.len(),
                self.ready.len(),
                self.loader.pending(),
                start.elapsed().as_secs_f64() * 1000.0,
            );
            #[cfg(target_os = "linux")]
            eprintln!(
                "chunk admission cpu: loaded={} total_us={}",
                loaded.len(),
                admission_thread_cpu_us() - cpu_start.unwrap(),
            );
        }
        loaded
    }

    /// Called once by the global tick owner, with the same deadline in all worlds.
    pub(crate) fn begin_admission_tick(&mut self, deadline: std::time::Instant) {
        self.admission_deadline = Some(deadline);
        self.admission_tick_elapsed = std::time::Duration::ZERO;
    }

    pub(crate) fn admission_elapsed(&self) -> std::time::Duration {
        self.admission_tick_elapsed
    }

    pub(super) fn admission_limit(&self) -> usize {
        (ADMISSION_BUDGET_US / self.admission_us_per_chunk)
            .clamp(1, crate::chunk_loader::READY_LOADS_MAX as u128) as usize
    }

    pub(super) fn record_admission_cost(&mut self, elapsed_us: u128, loaded: usize) {
        if loaded == 0 {
            return;
        }
        let sample = elapsed_us.div_ceil(loaded as u128).max(1);
        self.admission_us_per_chunk = (self.admission_us_per_chunk * 3 + sample).div_ceil(4);
    }

    /// Admit loaded chunks strictly in `(priority, position)` order: stop at
    /// the first wanted chunk whose load has not finished, so the admission
    /// (and revision stamp) order never depends on worker completion order.
    pub(super) fn admit_ready(&mut self, want: &HashMap<ChunkPos, i32>) -> Vec<ChunkPos> {
        let missing = self.admission_candidates(want);
        let mut loaded = Vec::new();
        self.admit_ready_into(want, &missing, usize::MAX, true, &mut loaded);
        loaded
    }

    fn admission_candidates(&mut self, want: &HashMap<ChunkPos, i32>) -> Vec<(i32, ChunkPos)> {
        self.ready.extend(self.loader.drain());
        // Discard stale results without consuming the admission budget. Do not
        // clear a newer ticket when an older completion finally arrives.
        let mut index = 0;
        while index < self.ready.len() {
            let (pos, ticket, _) = &self.ready[index];
            if self.in_flight.get(pos) != Some(ticket)
                || !want.contains_key(pos)
                || self.chunks.contains_key(pos)
            {
                let (pos, ticket, result) = self.ready.swap_remove(index);
                self.accept(pos, ticket, result, want);
            } else {
                index += 1;
            }
        }
        let mut missing: Vec<(i32, ChunkPos)> = want
            .iter()
            .filter(|(pos, _)| !self.chunks.contains_key(pos) && !self.failed.contains_key(pos))
            .map(|(pos, priority)| (*priority, *pos))
            .collect();
        missing.sort_unstable_by_key(|(priority, pos)| (*priority, pos.x, pos.z));
        missing
    }

    fn admit_ready_into(
        &mut self,
        want: &HashMap<ChunkPos, i32>,
        missing: &[(i32, ChunkPos)],
        limit: usize,
        ordered: bool,
        loaded: &mut Vec<ChunkPos>,
    ) {
        let previous = loaded.len();
        for (_, pos) in missing {
            if loaded.len() - previous == limit {
                break;
            }
            if self.chunks.contains_key(pos) || self.failed.contains_key(pos) {
                continue;
            }
            if let Some(index) = self.ready.iter().position(|entry| entry.0 == *pos) {
                let (pos, ticket, result) = self.ready.swap_remove(index);
                if self.accept(pos, ticket, result, want) {
                    loaded.push(pos);
                }
            } else if let Some(mut chunk) = self.recent.take(*pos) {
                if !self.apply_inhabited_image(*pos, &mut chunk) {
                    continue;
                }
                self.chunks.insert(*pos, chunk);
                self.active_revision += 1;
                self.bump_revision(*pos);
                loaded.push(*pos);
            } else if ordered {
                break;
            }
        }
        // Ready, running and queued jobs share one bound. Recent hits wait in
        // their existing bounded cache and never dispatch another load.
        for &(_, pos) in missing {
            if self.chunks.contains_key(&pos)
                || self.in_flight.contains_key(&pos)
                || self.failed.contains_key(&pos)
                || self.recent.chunks.contains_key(&pos)
            {
                continue;
            }
            let ticket = self.next_ticket + 1;
            if !self.loader.try_submit(pos, ticket, self.ready.len()) {
                break;
            }
            self.next_ticket = ticket;
            self.in_flight.insert(pos, ticket);
        }
    }

    /// [`Self::ensure`], then block until wanted chunks are resident or have
    /// failed closed (initial load and tests; never on the tick). Workers stay
    /// pipelined: finished loads are collected without waiting and each round
    /// admits the finished prefix of the wanted set in `(priority, position)`
    /// order, so stamps never depend on worker completion order. It waits for
    /// one load only when the next chunk in that order is still running.
    pub fn ensure_blocking(&mut self, want: &HashMap<ChunkPos, i32>) -> Vec<ChunkPos> {
        let mut loaded = Vec::new();
        loop {
            let round = self.admit_ready(want);
            let admitted = !round.is_empty();
            loaded.extend(round);
            if !want
                .keys()
                .any(|pos| !self.chunks.contains_key(pos) && !self.failed.contains_key(pos))
            {
                break;
            }
            if !admitted {
                assert!(
                    self.loader.pending() > 0,
                    "wanted chunks neither loading nor resident"
                );
                let finished = self.loader.recv();
                self.ready.push(finished);
            }
        }
        self.relight_admitted(&loaded, true);
        loaded
    }

    /// Make one loader result resident if its ticket is still the accepted
    /// one, the position is still wanted and not resident by another path.
    pub(super) fn accept(
        &mut self,
        pos: ChunkPos,
        ticket: u64,
        result: Result<crate::chunk_loader::Produced, ChunkLoadError>,
        want: &HashMap<ChunkPos, i32>,
    ) -> bool {
        if self.in_flight.get(&pos) != Some(&ticket) {
            return false;
        }
        self.in_flight.remove(&pos);
        if !want.contains_key(&pos) || self.chunks.contains_key(&pos) {
            return false;
        }
        let (chunk, generated) = match result {
            Ok(chunk) => chunk,
            Err(error) => {
                self.record_load_error(error);
                return false;
            }
        };
        self.insert_produced(pos, chunk, generated)
    }

    pub(super) fn record_load_error(&mut self, error: ChunkLoadError) {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.failed.entry(error.pos) {
            tracing::error!(%error, "stored chunk unavailable; repair required");
            entry.insert(error);
        }
    }
    /// Quarantine invalid resident metadata without losing checkpoint custody.
    /// Clears a live `active` flag at this mutation boundary with the same
    /// frozen-boundary authority as [`Self::refresh_active_clock`]
    /// deactivation, so the single debt flag in [`Self::is_active`] never
    /// observes stale activity. Absent or already-inactive authority keeps
    /// the `None` absolute-reanchor fallback without erasing a boundary.
    pub(crate) fn reject_resident(&mut self, pos: ChunkPos, reason: String) {
        let was_resident = self.chunks.remove(&pos).is_some();
        self.recent.take(pos);
        self.in_flight.remove(&pos);
        self.revisions.remove(&pos);
        self.wire_revisions.remove(&pos);
        self.villager_route_revisions.remove(&pos);
        self.light.remove(pos);
        self.published = None;
        self.generated.retain(|captured| captured.pos != pos);
        self.dirty.remove(&pos);
        self.dirty_sections.remove(&pos);
        self.dirty_absent.insert(pos);
        if was_resident {
            self.active_revision += 1;
            self.poi_revisions.insert(pos);
            let tick = self.sim_tick;
            let boundary = match self.sim_debt.get_mut(&pos) {
                Some(debt) if debt.active => {
                    let boundary = (debt.last_sim, debt.remaining);
                    debt.active = false;
                    debt.dirty = true;
                    Some(boundary)
                }
                _ => None,
            };
            if let Some((last_sim, remaining)) = boundary {
                self.next_revision += 1;
                self.revisions.insert(pos, self.next_revision);
                self.eviction_check.insert(pos);
                self.dirty.insert(pos);
                let image = crate::journal::SimDebtImage {
                    chunk: [pos.x, pos.z],
                    last_sim,
                    remaining,
                    active: false,
                    tick,
                };
                self.activity_changes.insert(pos, Some(image));
                self.debt_changes.insert(pos, image);
            } else {
                self.activity_changes.entry(pos).or_insert(None);
            }
        }
        self.record_load_error(ChunkLoadError {
            dimension: self.source.dimension,
            pos,
            reason,
        });
    }

    /// Stored-load failures, including dimension and exact decoder reason.
    pub fn load_errors(&self) -> impl Iterator<Item = &ChunkLoadError> {
        self.failed.values()
    }

    pub fn load_error(&self, pos: ChunkPos) -> Option<&ChunkLoadError> {
        self.failed.get(&pos)
    }

    /// Make a loaded or generated chunk resident; queue its immutable
    /// generated base while the bounded save buffer has room.
    pub(super) fn insert_produced(&mut self, pos: ChunkPos, chunk: Chunk, generated: bool) -> bool {
        let mut chunk = Arc::new(chunk);
        let generated_base = (generated
            && self.source.native.is_some()
            && self.generated.len() < GENERATED_PENDING_MAX)
            .then(|| Arc::clone(&chunk));
        if !self.apply_inhabited_image(pos, &mut chunk) {
            return false;
        }
        if let Some(base) = generated_base {
            self.generated.push(CapturedChunk {
                pos,
                revision: 0,
                chunk: base,
                dirty_sections: None,
            });
        }
        self.chunks.insert(pos, chunk);
        self.active_revision += 1;
        self.bump_revision(pos);
        true
    }

    /// A load may have read native storage before an absent counter capture
    /// committed. Its live interval remains authoritative across admission.
    pub(super) fn apply_inhabited_image(&mut self, pos: ChunkPos, chunk: &mut Arc<Chunk>) -> bool {
        if let Err(error) = crate::debt::adopt_native_extras(pos, chunk, &mut self.sim_debt) {
            self.record_load_error(ChunkLoadError {
                dimension: self.source.dimension,
                pos,
                reason: error.to_string(),
            });
            return false;
        }
        if let Some(counter) = self.inhabited_time.get(&pos)
            && chunk.inhabited_time() != counter.image.value_at(self.inhabited_tick)
        {
            counter
                .image
                .apply(Arc::make_mut(chunk), self.inhabited_tick);
        }
        true
    }

    /// Generated immutable bases to save once at least `min` are queued.
    /// Native fill-generated checks coverage atomically, so a delayed batch
    /// never overwrites imported data or checkpointed gameplay.
    pub fn take_generated(&mut self, min: usize) -> Option<ChunkCapture> {
        if self.generated.is_empty() || self.generated.len() < min {
            return None;
        }
        Some(ChunkCapture {
            source: Arc::clone(&self.source),
            chunks: std::mem::take(&mut self.generated),
            deferred: HashMap::default(),
            load_error: None,
            generated_bases: true,
        })
    }

    /// Make `pos` resident now (checkpoint paths). Any off-tick load of the
    /// same chunk loses its ticket: its data was read before this
    /// checkpoint's native commit and must never land.
    pub(super) fn materialize(&mut self, pos: ChunkPos) -> bool {
        if self.chunks.contains_key(&pos) {
            return true;
        }
        if self.failed.contains_key(&pos) {
            return false;
        }
        self.in_flight.remove(&pos);
        match self.recent.take(pos) {
            Some(mut chunk) => {
                if !self.apply_inhabited_image(pos, &mut chunk) {
                    return false;
                }
                self.chunks.insert(pos, chunk);
                self.active_revision += 1;
                self.bump_revision(pos);
            }
            None => match self.source.load_or_generate(pos) {
                Ok((chunk, generated)) => {
                    if !self.insert_produced(pos, chunk, generated) {
                        return false;
                    }
                }
                Err(error) => {
                    self.record_load_error(error);
                    return false;
                }
            },
        }
        self.relight_admitted(&[pos], false);
        true
    }
}

/// Opt-in phase diagnostics exclude descheduling on a busy shared host.
#[cfg(target_os = "linux")]
fn admission_thread_cpu_us() -> u128 {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
    time.tv_sec as u128 * 1_000_000 + time.tv_nsec as u128 / 1_000
}
