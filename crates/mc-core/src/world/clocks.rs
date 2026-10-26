//! Inhabited time, simulation debt and active-chunk clocks.
//! Moved verbatim from `world.rs`.

use super::*;

impl ChunkWorld {
    /// Read the live counter, including ticks not yet captured for persistence.
    pub(crate) fn inhabited_time(&self, pos: ChunkPos) -> i64 {
        self.inhabited_time.get(&pos).map_or_else(
            || self.chunk(pos).map_or(0, Chunk::inhabited_time).max(0),
            |counter| counter.image.value_at(self.inhabited_tick),
        )
    }

    /// Count a resident chunk once per game tick, regardless of player overlap.
    /// Unchanged active intervals neither allocate nor copy shared chunks.
    pub(crate) fn tick_inhabited_time(&mut self, pos: ChunkPos, tick: u64) {
        let Some(chunk) = self.chunks.get(&pos) else {
            return;
        };
        self.inhabited_tick = tick;
        let counter = self
            .inhabited_time
            .entry(pos)
            .or_insert_with(|| InhabitedCounter {
                image: crate::journal::InhabitedTimeImage {
                    chunk: [pos.x, pos.z],
                    value: chunk.inhabited_time().max(0),
                    tick,
                    active: false,
                },
                last_seen: tick.wrapping_sub(1),
                dirty: false,
            });
        if counter.last_seen == tick {
            return;
        }
        counter.last_seen = tick;
        self.inhabited_current.push(pos);
        if !counter.image.active {
            // Opening/closing an interval changes its stored value. Continuing
            // live time is already covered by the checkpoint's manifest anchor;
            // it must not force a full chunk re-encode every checkpoint.
            counter.dirty = true;
            counter.image.value = counter.image.value.saturating_add(1);
            counter.image.tick = tick;
            counter.image.active = true;
            self.inhabited_changes.push(counter.image);
        }
    }

    pub(crate) fn finish_inhabited_tick(&mut self, tick: u64) {
        self.inhabited_tick = tick;
        for pos in self.inhabited_active.drain(..) {
            let counter = self.inhabited_time.get_mut(&pos).expect("active interval");
            if counter.last_seen != tick {
                counter.image.value = counter.image.value_at(tick.saturating_sub(1));
                counter.image.tick = tick;
                counter.image.active = false;
                counter.dirty |= self
                    .chunks
                    .get(&pos)
                    .is_some_and(|chunk| chunk.inhabited_time() != counter.image.value);
                self.inhabited_changes.push(counter.image);
                self.eviction_check.insert(pos);
            }
        }
        std::mem::swap(&mut self.inhabited_active, &mut self.inhabited_current);
    }

    pub(crate) fn journal_inhabited_time(
        &mut self,
        entries: &mut Vec<crate::journal::JournalEntry>,
    ) {
        // Pending transitions can span more than one tick before capture.
        // Keep the latest absolute image and anchor it at this batch's clock.
        self.inhabited_changes
            .sort_unstable_by_key(|image| (image.chunk, std::cmp::Reverse(image.tick)));
        self.inhabited_changes.dedup_by_key(|image| image.chunk);
        entries.extend(self.inhabited_changes.drain(..).map(|image| {
            crate::journal::JournalEntry::InhabitedTime {
                image: crate::journal::InhabitedTimeImage {
                    value: image.value_at(self.inhabited_tick),
                    tick: self.inhabited_tick,
                    ..image
                },
            }
        }));
    }

    pub(crate) fn inhabited_checkpoint(
        &self,
        tick: u64,
    ) -> Vec<crate::journal::InhabitedTimeImage> {
        let mut images: Vec<_> = self
            .inhabited_active
            .iter()
            .map(|pos| {
                let image = self.inhabited_time[pos].image;
                crate::journal::InhabitedTimeImage {
                    value: image.value_at(tick),
                    tick,
                    ..image
                }
            })
            .collect();
        images.sort_unstable_by_key(|image| image.chunk);
        images
    }

    /// Restore durable checkpoint counters without making them dirty again.
    /// `None` denotes WAL-only/injected images without a persisted base.
    pub(crate) fn restore_inhabited_time(
        &mut self,
        replay: &crate::journal::Replay,
        checkpoint: Option<&crate::journal::CheckpointManifest>,
    ) {
        self.inhabited_tick = replay.tick;
        let persisted = checkpoint.map(|base| {
            (
                base.tick,
                base.inhabited_time
                    .iter()
                    .map(|image| (image.chunk, image))
                    .collect::<HashMap<_, _>>(),
            )
        });
        for (&[x, z], &image) in &replay.inhabited_time {
            let dirty = persisted.as_ref().is_none_or(|(tick, images)| {
                images
                    .get(&image.chunk)
                    .is_none_or(|base| image.value_at(replay.tick) != base.value_at(*tick))
            }) || image.active;
            // Recovery keeps an open interval, whose clean manifest value may be
            // newer than the native chunk after clock-only checkpoints. Pin it
            // dirty so closing it (or an absent chunk) persists the value.
            let pos = ChunkPos { x, z };
            let image = crate::journal::InhabitedTimeImage {
                value: image.value_at(replay.tick),
                tick: replay.tick,
                ..image
            };
            if image.active {
                self.inhabited_active.push(pos);
            }
            self.inhabited_time.insert(
                pos,
                InhabitedCounter {
                    image,
                    last_seen: replay.tick,
                    dirty,
                },
            );
        }
    }

    /// Chunks whose live clock changed admission during this batch. Residency
    /// alone and debt consumption do not invalidate unrelated entity anchors.
    pub(crate) fn simulation_clock_changes(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.activity_changes.keys().copied()
    }

    /// Drain pending debt post-images into the tick WAL batch (called at the
    /// END of `journal_entries_for_tick`). Latest image per chunk wins.
    pub(crate) fn journal_debt(&mut self, entries: &mut Vec<crate::journal::JournalEntry>) {
        let start = entries.len();
        entries.extend(self.debt_changes.drain().map(|(_, image)| {
            crate::journal::JournalEntry::SimDebt {
                image: crate::journal::SimDebtImage {
                    last_sim: image.last_sim_at(self.sim_tick),
                    tick: self.sim_tick,
                    ..image
                },
            }
        }));
        entries[start..].sort_unstable_by_key(|entry| match entry {
            crate::journal::JournalEntry::SimDebt { image } => image.chunk,
            _ => unreachable!("only debt images appended"),
        });
        self.activity_changes.clear();
    }

    /// Authority debt images for the checkpoint manifest at `tick`: dirty or
    /// nonzero-debt chunks. Mirrors `inhabited_checkpoint` (owned Vec, the
    /// manifest stores it; the journal thread never reads live records).
    pub(crate) fn debt_checkpoint(&self, tick: u64) -> Vec<crate::journal::SimDebtImage> {
        let mut images: Vec<_> = self
            .sim_debt
            .iter()
            .filter(|(_, debt)| debt.dirty || debt.remaining > 0 || debt.active)
            .map(|(pos, debt)| crate::journal::SimDebtImage {
                chunk: [pos.x, pos.z],
                last_sim: debt.last_sim,
                remaining: debt.remaining,
                active: debt.active,
                tick,
            })
            .collect();
        images.sort_unstable_by_key(|image| image.chunk);
        images
    }

    /// Restore debt authority, distinguishing durable checkpoint images from
    /// newer WAL images. A consumed zero still pins its chunk until committed;
    /// an already durable inactive image does not need another chunk rewrite.
    /// `None` denotes WAL-only/injected images without a persisted base.
    pub(crate) fn restore_sim_debt(
        &mut self,
        replay: &crate::journal::Replay,
        checkpoint: Option<&crate::journal::CheckpointManifest>,
    ) -> Result<(), CoreError> {
        // Refuse unrepresentable WAL/manifest authority before activation can
        // consume it or any partial restore can mutate the live map.
        for (&[x, z], &image) in &replay.sim_debt {
            crate::debt::native_values(
                ChunkPos { x, z },
                image.last_sim_at(replay.tick),
                image.remaining,
            )?;
        }
        self.sim_tick = replay.tick;
        let persisted = checkpoint.map(|base| {
            (
                base.tick,
                base.debt
                    .iter()
                    .map(|image| (image.chunk, image))
                    .collect::<HashMap<_, _>>(),
            )
        });
        for (&[x, z], &image) in &replay.sim_debt {
            let pos = ChunkPos { x, z };
            let last_sim = image.last_sim_at(replay.tick);
            let dirty = persisted.as_ref().is_none_or(|(tick, images)| {
                images.get(&image.chunk).is_none_or(|base| {
                    last_sim != base.last_sim_at(*tick) || image.remaining != base.remaining
                })
            }) || image.active;
            // Recovery closes open debt intervals. A clean manifest anchor may
            // be newer than the native chunk after clock-only checkpoints, so
            // persist that closure before an inactive zero leaves the manifest.
            self.sim_debt.insert(
                pos,
                crate::debt::SimDebtRecord {
                    last_sim,
                    remaining: image.remaining,
                    dirty,
                    active: false,
                },
            );
            if dirty || image.active {
                self.debt_changes.insert(
                    pos,
                    crate::journal::SimDebtImage {
                        chunk: [x, z],
                        last_sim,
                        remaining: image.remaining,
                        active: false,
                        tick: replay.tick,
                    },
                );
            }
            if dirty {
                self.dirty.insert(pos);
                self.bump_storage_revision(pos);
            }
        }
        Ok(())
    }

    /// Live clock bounds `(start, end]` since a lazy mirror's membership fence.
    /// Refresh mirrors in `simulation_clock_changes` before `journal_debt`
    /// drains the boundary images, so `from` never spans an older transition.
    /// Debt consumption is not an activation. Only countdowns use `end - start`.
    #[must_use]
    pub fn simulation_interval(&self, pos: ChunkPos, from: u64, to: u64) -> Option<(u64, u64)> {
        if to <= from {
            return None;
        }
        let (start, end) = if let Some(Some(image)) = self.activity_changes.get(&pos) {
            if image.active {
                // Activation simulates the boundary tick itself.
                (from.max(image.tick.saturating_sub(1)), to)
            } else {
                // Deactivation freezes before this tick's simulation.
                (from, image.last_sim.min(to))
            }
        } else if self.is_active(pos.x, pos.z) {
            (from, to)
        } else {
            return None;
        };
        (start < end).then_some((start, end))
    }

    /// Parent-computed active set with hysteresis, called pre-gameplay, after
    /// inputs and after ensure at the same output tick (tick+1) idempotently.
    /// Transition-based (no same-tick skip): residency gates activation, so
    /// demand admitted later the same tick still consumes on that call.
    /// Only live-time accrual is tick-guarded (`tick > last_sim`).
    /// Activation records inactive elapsed time as debt, excluding this live
    /// tick. The merge owner consumes bounded whole-chunk steps separately.
    /// Continuing active advances only the live anchor, never remaining debt.
    pub fn set_active_chunks(&mut self, active: HashSet<ChunkPos>, tick: u64) {
        if self.active_set != active {
            self.active_revision += 1;
        }
        self.active_set = active;
        self.refresh_active_clock(tick);
    }

    pub(crate) fn set_block_only_chunks(&mut self, chunks: HashSet<ChunkPos>) {
        if self.block_only != chunks {
            self.active_revision += 1;
            self.block_only = chunks;
        }
    }

    pub(crate) fn entity_active_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.active_set.iter().copied()
    }

    /// All ticking demand (resident or not), without duplicates.
    pub fn active_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.active_set.iter().chain(&self.block_only).copied()
    }

    /// Parent resident-union input: chunks pinning eviction (dirty, absent,
    /// debt or inhabited images). Duplicates possible; the caller dedups.
    pub fn dirty_pins(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.dirty
            .iter()
            .copied()
            .chain(self.dirty_absent.iter().copied())
            .chain(
                self.sim_debt
                    .iter()
                    .filter_map(|(pos, debt)| debt.dirty.then_some(*pos)),
            )
            .chain(
                self.inhabited_time
                    .iter()
                    .filter_map(|(pos, counter)| counter.dirty.then_some(*pos)),
            )
    }

    /// Reconcile resident/active transitions and advance `last_sim` at `tick`
    /// without taking a membership set. Parent's cached-basis fast path:
    /// stationary ticks skip the set rebuild and call this after ensure so
    /// newly admitted chunks with active demand activate in the same tick.
    /// [`Self::set_active_chunks`] stores then calls this same method.
    pub fn refresh_active_clock(&mut self, tick: u64) {
        self.sim_tick = tick;
        // Hot path: no per-tick allocation or sorting. Disjoint field borrows
        // cover activation, accrual and their storage revisions inline.
        // `debt_changes` order is normalized at WAL drain (`journal_debt`).
        let Self {
            chunks,
            sim_debt,
            active_set,
            dirty,
            debt_changes,
            activity_changes,
            revisions,
            next_revision,
            eviction_check,
            active_revision,
            ..
        } = self;
        // Deactivations first: freeze records leaving membership.
        for (pos, debt) in sim_debt.iter_mut() {
            if debt.active && (!active_set.contains(pos) || !chunks.contains_key(pos)) {
                debt.active = false;
                *active_revision += 1;
                debt.dirty = true;
                *next_revision += 1;
                revisions.insert(*pos, *next_revision);
                eviction_check.insert(*pos);
                dirty.insert(*pos);
                let image = crate::journal::SimDebtImage {
                    chunk: [pos.x, pos.z],
                    last_sim: debt.last_sim,
                    remaining: debt.remaining,
                    active: false,
                    tick,
                };
                activity_changes.insert(*pos, Some(image));
                debt_changes.insert(*pos, image);
            }
        }
        // Activations + live accrual over stored membership. Residency gates
        // activation: demand admitted later the same tick still consumes on
        // this call; demand that loses residency deactivates and freezes.
        for pos in active_set.iter() {
            let resident = chunks.contains_key(pos);
            if !resident {
                if let Some(record) = sim_debt.get_mut(pos) {
                    record.active = false;
                }
                continue;
            }
            let record = sim_debt.entry(*pos).or_insert(crate::debt::SimDebtRecord {
                last_sim: tick,
                remaining: 0,
                dirty: false,
                active: false,
            });
            if record.active {
                if tick > record.last_sim {
                    record.last_sim = tick;
                }
                continue;
            }
            // Accrue only the inactive interval, never the activation's live tick.
            record.remaining = record
                .remaining
                .saturating_add(tick.saturating_sub(1).saturating_sub(record.last_sim));
            record.active = true;
            *active_revision += 1;
            record.last_sim = tick;
            record.dirty = true;
            // Storage-only fence inline (never wire/published/hydration: this
            // is metadata authority, not client-visible content).
            *next_revision += 1;
            revisions.insert(*pos, *next_revision);
            eviction_check.insert(*pos);
            dirty.insert(*pos);
            let image = crate::journal::SimDebtImage {
                chunk: [pos.x, pos.z],
                last_sim: tick,
                remaining: record.remaining,
                active: true,
                tick,
            };
            activity_changes.insert(*pos, Some(image));
            debt_changes.insert(*pos, image);
        }
        // Level 32 runs block time live, not entity time. Do not later replay
        // those same ticks as whole-chunk catch-up when this chunk reaches 31.
        // These anchors are rebuilt from persisted ticket demand on recovery.
        for pos in &self.block_only {
            if !chunks.contains_key(pos) {
                continue;
            }
            let record = sim_debt.entry(*pos).or_insert(crate::debt::SimDebtRecord {
                last_sim: tick,
                remaining: 0,
                dirty: false,
                active: false,
            });
            record.last_sim = tick;
            record.remaining = 0;
        }
    }

    pub(crate) fn simulation_debt(&self, pos: ChunkPos) -> u64 {
        self.sim_debt.get(&pos).map_or(0, |debt| debt.remaining)
    }

    /// Resident-only worklist: dormant work never requests chunk admission.
    pub(crate) fn catch_up_chunks(&self) -> Vec<ChunkPos> {
        let mut chunks: Vec<_> = self
            .sim_debt
            .iter()
            .filter_map(|(&pos, debt)| {
                (debt.active && debt.remaining > 0 && self.chunks.contains_key(&pos)).then_some(pos)
            })
            .collect();
        chunks.sort_unstable_by_key(|pos| (pos.x, pos.z));
        chunks
    }

    /// Debt and effects are captured by the same tick WAL/checkpoint fence.
    pub(crate) fn consume_simulation_debt(&mut self, pos: ChunkPos, elapsed: u64) {
        let Some(debt) = self.sim_debt.get_mut(&pos) else {
            return;
        };
        debt.remaining = debt.remaining.saturating_sub(elapsed);
        debt.dirty = true;
        self.debt_changes.insert(
            pos,
            crate::journal::SimDebtImage {
                chunk: [pos.x, pos.z],
                last_sim: debt.last_sim,
                remaining: debt.remaining,
                active: debt.active,
                tick: self.sim_tick,
            },
        );
        self.dirty.insert(pos);
        // Metadata must not request rehydration or invalidate client light.
        self.next_revision += 1;
        self.revisions.insert(pos, self.next_revision);
        self.eviction_check.insert(pos);
    }

    /// 001-f: an accepted external write invalidates old time, including an
    /// inactive interval not yet materialized as remaining debt.
    pub(crate) fn cancel_simulation_debt(&mut self, pos: ChunkPos) {
        let Some(debt) = self.sim_debt.get_mut(&pos) else {
            return;
        };
        if debt.remaining == 0 && (debt.active || debt.last_sim >= self.sim_tick) {
            return;
        }
        debt.last_sim = self.sim_tick;
        self.consume_simulation_debt(pos, u64::MAX);
    }

    /// Retire acknowledged empty queue coverage only when no old native image
    /// can still become resident. Worker tickets and bounded recent-cache
    /// copies may predate the checkpoint commit even after content is clean.
    pub(crate) fn unloaded_clean(&self, x: i32, z: i32) -> bool {
        let pos = ChunkPos { x, z };
        !self.chunks.contains_key(&pos)
            && !self.in_flight.contains_key(&pos)
            && !self.recent.chunks.contains_key(&pos)
            && !self.dirty.contains(&pos)
            && !self.dirty_absent.contains(&pos)
            && !self
                .overlay
                .keys()
                .any(|(bx, _, bz)| bx.div_euclid(16) == x && bz.div_euclid(16) == z)
    }
}
