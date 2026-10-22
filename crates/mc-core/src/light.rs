//! Tick-owned lighting, using the shared vanilla oracle and removal/addition BFS.
//!
//! Workers bake isolated chunks; admission reconciles their boundaries and only
//! propagates changed cells. Block edits use the cached 3x3 removal/addition BFS.
//! Baked arrays live in the authoritative chunks, so snapshots and Anvil writes
//! see the same light. WAL block post-images reproduce derived light on replay.

use std::collections::VecDeque;
use std::sync::Arc;

use mc_data::block_light::BlockLightTable;
use mc_world::light::{
    ChunkLight, LightCache, LightWorkspace, apply_block_change_to_light, compute_chunk_light_in,
};
use mc_world::{BlockStateId, Chunk, ChunkPos};
use rustc_hash::{FxHashMap, FxHashSet};

pub(crate) struct LightEngine {
    table: Arc<BlockLightTable>,
    cache: LightCache,
    workspace: LightWorkspace,
    admission_queue: VecDeque<usize>,
    admission_removal: VecDeque<(usize, u8, u8)>,
    admission_queued: Vec<u8>,
    dirty_seams: FxHashSet<ChunkPos>,
    #[cfg(test)]
    admission_seed_cells: usize,
    #[cfg(test)]
    pub(crate) full_recomputes: usize,
}

impl LightEngine {
    pub(crate) fn new(table: Arc<BlockLightTable>) -> Self {
        Self {
            table,
            cache: LightCache::new(),
            workspace: LightWorkspace::new(),
            admission_queue: VecDeque::new(),
            admission_removal: VecDeque::new(),
            admission_queued: Vec::new(),
            dirty_seams: FxHashSet::default(),
            #[cfg(test)]
            admission_seed_cells: 0,
            #[cfg(test)]
            full_recomputes: 0,
        }
    }
    /// Registry-owned oracle handle behind this engine.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn table(&self) -> &Arc<BlockLightTable> {
        &self.table
    }

    pub(crate) fn remove(&mut self, pos: ChunkPos) {
        self.cache.remove(pos);
        self.dirty_seams.remove(&pos);
        // Eviction leaves adjacent baked/cache interiors valid, but their
        // seams may still contain the removed chunk's influence. Keep only
        // currently cached residents dirty: this set is bounded by the cache,
        // not by all positions visited during world traversal. Do not discard
        // their light: edit() requires those existing cache entries.
        for dz in -1..=1 {
            for dx in -1..=1 {
                let neighbour = ChunkPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                };
                if self.cache.contains(neighbour) {
                    self.dirty_seams.insert(neighbour);
                }
            }
        }
    }

    /// Reconcile each affected resident chunk once, regardless of batch order.
    pub(crate) fn admit(
        &mut self,
        chunks: &FxHashMap<ChunkPos, Arc<Chunk>>,
        loaded: &[ChunkPos],
    ) -> Vec<(ChunkPos, ChunkLight)> {
        let targets = admission_targets(chunks, loaded);
        let Some(first) = targets.iter().next() else {
            return Vec::new();
        };
        let geometry = chunks[first].geometry();
        let height = geometry.height() as usize;
        let volume = height * 256;

        // The 2026-10-15 profile attributed 9.46% of sampled cycles to full
        // admission recomputes: each target decoded and flooded another 3x3.
        // Baked/cache interiors already satisfy the light equations. Only
        // chunk seams change on admission. As in mc-world's incremental edit
        // solver, invalidate dependent light using its previous level, then
        // refill from surviving sources with strict improvements. Counting a
        // decrease down level by level repeatedly walks the same interior.
        //
        // Correctness fence: lateral/indirect propagation always costs >=1;
        // sky=15 therefore identifies an intrinsic vertical-column source.
        // Complete producer/native arrays describe their final block image;
        // workers bake omitted layers. The one-chunk halo holds the full oracle
        // neighbourhood; anything beyond it is >=17 steps from a target and
        // cannot affect a channel capped at 15. Missing chunks must transmit
        // block light through air, including paths around a diagonal corner.
        let mut context = FxHashSet::default();
        for centre in &targets {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    context.insert(ChunkPos {
                        x: centre.x + dx,
                        z: centre.z + dz,
                    });
                }
            }
        }
        let mut positions: Vec<_> = context.into_iter().collect();
        positions.sort_unstable_by_key(|pos| (pos.x, pos.z));
        let indices: FxHashMap<_, _> = positions
            .iter()
            .enumerate()
            .map(|(index, pos)| (*pos, index))
            .collect();
        let mut slots = Vec::with_capacity(positions.len());
        for pos in &positions {
            let chunk = chunks.get(pos).map(Arc::as_ref);
            let seed = loaded.contains(pos)
                || (chunk.is_some()
                    && (!self.cache.contains(*pos) || self.dirty_seams.contains(pos)));
            let light = if let Some(chunk) = chunk {
                assert_eq!(
                    chunk.geometry(),
                    geometry,
                    "lighting neighbourhood chunks must share geometry",
                );
                if let Some(light) = self.cache.get(*pos) {
                    light.clone()
                } else if has_complete_light(chunk)
                    && let Some(light) = ChunkLight::from_chunk(chunk)
                {
                    light
                } else {
                    // Raw test/checkpoint insertions may bypass workers.
                    // Never rebake a complete prepared chunk on the tick.
                    #[cfg(test)]
                    {
                        self.full_recomputes += 1;
                    }
                    compute_chunk_light_in(
                        &mut self.workspace,
                        [[None; 3], [None, Some(chunk), None], [None; 3]],
                        &self.table,
                    )
                }
            } else {
                ChunkLight::filled_for_geometry(15, 0, geometry)
            };
            slots.push(AdmissionSlot {
                chunk,
                seed,
                light,
                adjacent: [(-1, 0), (1, 0), (0, -1), (0, 1)].map(|(dx, dz)| {
                    indices
                        .get(&ChunkPos {
                            x: pos.x + dx,
                            z: pos.z + dz,
                        })
                        .copied()
                }),
            });
        }

        seed_admission_boundaries(
            &slots,
            volume,
            &self.table,
            &mut self.admission_queue,
            &mut self.admission_queued,
        );
        #[cfg(test)]
        {
            self.admission_seed_cells = self.admission_queue.len();
        }
        self.admission_removal.clear();
        // Classify the original seam residuals before draining either wave.
        // Keep every seed for addition: a presently unchanged seed may supply
        // an adjacent cell after removal. Newly queued peers go behind the
        // original seeds and are not classified a second time.
        for _ in 0..self.admission_queue.len() {
            let cell = self.admission_queue.pop_front().unwrap();
            self.admission_queued[cell] = 0;
            enqueue_admission(&mut self.admission_queue, &mut self.admission_queued, cell);
            let old = admission_light(&slots, cell, volume);
            let peers = admission_neighbours(&slots, cell, volume);
            let mut strongest = (0_u8, 0_u8);
            for (direction, peer) in peers.iter().enumerate() {
                if let Some(peer) = peer {
                    let light = admission_light(&slots, *peer, volume);
                    strongest.0 = strongest.0.max(light.0);
                    strongest.1 = strongest.1.max(light.1);
                } else if direction < 4 {
                    strongest.0 = 15;
                }
            }
            if old.1 == 0 && strongest.1 <= 1 && (old.0 == 15 || (old.0 == 0 && strongest.0 <= 1)) {
                continue;
            }
            let (cost, emission) = admission_material(&slots, cell, volume, &self.table);
            let desired = (
                if old.0 == 15 {
                    15
                } else {
                    strongest.0.saturating_sub(cost)
                },
                emission.max(strongest.1.saturating_sub(cost)),
            );
            if desired == old {
                continue;
            }
            let sky_source = if old.0 == 15 {
                15
            } else if peers[..4].contains(&None) {
                15_u8.saturating_sub(cost)
            } else {
                0
            };
            // An increasing residual can also mask a lost old source: dirty
            // halo light is scratch-only, so adjacent cached arrays need not
            // share the same old gradient. Invalidate either residual before
            // removal; a genuinely stronger surviving peer refills it later.
            let next = (
                if desired.0 != old.0 {
                    sky_source
                } else {
                    old.0
                },
                if desired.1 != old.1 { emission } else { old.1 },
            );
            set_admission_light(&mut slots, cell, volume, old, next);
            let removed = (
                if next.0 < old.0 { old.0 } else { 0 },
                if next.1 < old.1 { old.1 } else { 0 },
            );
            if removed != (0, 0) {
                self.admission_removal
                    .push_back((cell, removed.0, removed.1));
            }
            for peer in peers.into_iter().flatten() {
                enqueue_admission(&mut self.admission_queue, &mut self.admission_queued, peer);
            }
        }

        while let Some((cell, sky_previous, block_previous)) = self.admission_removal.pop_front() {
            for peer in admission_neighbours(&slots, cell, volume)
                .into_iter()
                .flatten()
            {
                let old = admission_light(&slots, peer, volume);
                // Values at least as strong cannot depend on this source:
                // propagation always pays a positive receiving cost.
                let sky_candidate = old.0 > 0 && old.0 < sky_previous;
                let block_candidate = old.1 > 0 && old.1 < block_previous;
                if sky_candidate || block_candidate {
                    let (cost, emission) = admission_material(&slots, peer, volume, &self.table);
                    let mut next = old;
                    if sky_candidate && old.0 == sky_previous.saturating_sub(cost) {
                        // An omitted outer horizontal neighbour is still open
                        // sky. Preserve that boundary source during removal;
                        // virtual air inside the context has intrinsic sky=15.
                        next.0 = if admission_neighbours(&slots, peer, volume)[..4].contains(&None)
                        {
                            15_u8.saturating_sub(cost)
                        } else {
                            0
                        };
                    }
                    if block_candidate && old.1 == block_previous.saturating_sub(cost) {
                        next.1 = emission;
                    }
                    set_admission_light(&mut slots, peer, volume, old, next);
                    let removed = (
                        if next.0 < old.0 { old.0 } else { 0 },
                        if next.1 < old.1 { old.1 } else { 0 },
                    );
                    if removed != (0, 0) {
                        self.admission_removal
                            .push_back((peer, removed.0, removed.1));
                    }
                }
                // Surviving emitters, direct sky and independent paths refill
                // the cleared region only after the entire removal wave.
                enqueue_admission(&mut self.admission_queue, &mut self.admission_queued, peer);
            }
        }

        while let Some(cell) = self.admission_queue.pop_front() {
            self.admission_queued[cell] = 0;
            let current = admission_light(&slots, cell, volume);
            if current.0 <= 1 && current.1 <= 1 {
                continue;
            }
            let best = (current.0.saturating_sub(1), current.1.saturating_sub(1));
            for peer in admission_neighbours(&slots, cell, volume)
                .into_iter()
                .flatten()
            {
                let old = admission_light(&slots, peer, volume);
                if best.0 <= old.0 && best.1 <= old.1 {
                    continue;
                }
                let (cost, _) = admission_material(&slots, peer, volume, &self.table);
                let next = (
                    old.0.max(current.0.saturating_sub(cost)),
                    old.1.max(current.1.saturating_sub(cost)),
                );
                if next == old {
                    continue;
                }
                set_admission_light(&mut slots, peer, volume, old, next);
                enqueue_admission(&mut self.admission_queue, &mut self.admission_queued, peer);
            }
        }
        // Halo corrections are scratch only. Preserve admission's original
        // resident-target boundary, including authoritative arrays/revisions.
        let mut updates = Vec::with_capacity(targets.len());
        for (pos, slot) in positions.into_iter().zip(slots) {
            if targets.contains(&pos) {
                self.cache.insert(pos, slot.light.clone());
                self.dirty_seams.remove(&pos);
                updates.push((pos, slot.light));
            }
        }
        updates
    }

    /// Solve a joined startup batch directly; never called by tick admission.
    pub(crate) fn admit_blocking(
        &mut self,
        chunks: &FxHashMap<ChunkPos, Arc<Chunk>>,
        loaded: &[ChunkPos],
    ) -> Vec<(ChunkPos, ChunkLight)> {
        // Blocking startup callers already wait for the complete joined
        // batch, so solve each target's final 3x3 directly. Tick admission
        // instead reconciles cached seams with removal/addition waves.
        let targets = admission_targets(chunks, loaded);
        let mut updates = Vec::with_capacity(targets.len());
        for pos in targets {
            let light =
                compute_chunk_light_in(&mut self.workspace, neighbours(chunks, pos), &self.table);
            #[cfg(test)]
            {
                self.full_recomputes += 1;
            }
            self.cache.insert(pos, light.clone());
            self.dirty_seams.remove(&pos);
            updates.push((pos, light));
        }
        updates
    }

    pub(crate) fn edit(
        &mut self,
        chunks: &FxHashMap<ChunkPos, Arc<Chunk>>,
        pos: (i32, i32, i32),
        previous: BlockStateId,
        next: BlockStateId,
    ) -> Vec<(ChunkPos, ChunkLight)> {
        if self.table.emission(previous.0) == self.table.emission(next.0)
            && self.table.opacity(previous.0) == self.table.opacity(next.0)
            && self.table.propagates_sky(previous.0) == self.table.propagates_sky(next.0)
        {
            return Vec::new();
        }
        let centre = ChunkPos {
            x: pos.0.div_euclid(16),
            z: pos.2.div_euclid(16),
        };
        let refs = neighbours(chunks, centre);
        let touched = apply_block_change_to_light(
            &mut self.cache,
            &refs,
            &self.table,
            centre,
            pos.0.rem_euclid(16) as u8,
            pos.1,
            pos.2.rem_euclid(16) as u8,
            previous,
            next,
        );
        touched
            .into_iter()
            .filter_map(|pos| self.cache.get(pos).cloned().map(|light| (pos, light)))
            .collect()
    }
}

fn admission_targets(
    chunks: &FxHashMap<ChunkPos, Arc<Chunk>>,
    loaded: &[ChunkPos],
) -> FxHashSet<ChunkPos> {
    let mut targets = FxHashSet::default();
    for centre in loaded {
        for dz in -1..=1 {
            for dx in -1..=1 {
                let pos = ChunkPos {
                    x: centre.x + dx,
                    z: centre.z + dz,
                };
                if chunks.contains_key(&pos) {
                    targets.insert(pos);
                }
            }
        }
    }
    targets
}

struct AdmissionSlot<'a> {
    chunk: Option<&'a Chunk>,
    light: ChunkLight,
    seed: bool,
    adjacent: [Option<usize>; 4],
}

fn admission_light(slots: &[AdmissionSlot<'_>], cell: usize, volume: usize) -> (u8, u8) {
    let light = &slots[cell / volume].light;
    let local = cell % volume;
    let (x, y, z) = (local % 16, local / 256, local / 16 % 16);
    (light.sky.get(x, y, z), light.block.get(x, y, z))
}

fn set_admission_light(
    slots: &mut [AdmissionSlot<'_>],
    cell: usize,
    volume: usize,
    old: (u8, u8),
    next: (u8, u8),
) {
    let light = &mut slots[cell / volume].light;
    let local = cell % volume;
    let (x, y, z) = (local % 16, local / 256, local / 16 % 16);
    if next.0 != old.0 {
        light.sky.set(x, y, z, next.0);
    }
    if next.1 != old.1 {
        light.block.set(x, y, z, next.1);
    }
}

fn admission_material(
    slots: &[AdmissionSlot<'_>],
    cell: usize,
    volume: usize,
    table: &BlockLightTable,
) -> (u8, u8) {
    slots[cell / volume].chunk.map_or((1, 0), |chunk| {
        let local = cell % volume;
        let state = chunk
            .get_block(
                (local % 16) as u8,
                chunk.geometry().min_y() + (local / 256) as i32,
                (local / 16 % 16) as u8,
            )
            .unwrap_or(BlockStateId(0));
        (
            table.opacity(state.0).unwrap_or(0).max(1),
            table.emission(state.0).unwrap_or(0),
        )
    })
}

fn seed_admission_boundaries(
    slots: &[AdmissionSlot<'_>],
    volume: usize,
    table: &BlockLightTable,
    queue: &mut VecDeque<usize>,
    queued: &mut Vec<u8>,
) {
    queue.clear();
    let mut scratch_ready = false;
    // Bulk recent returns sit inside the measured 107.17ms travel refresh:
    // .analysis/codex-logs/tick-perf/checkpoint-fixed-localization.json.
    // Certify whole section faces before expanding them into queued cells.
    // Matching zero-block sections with uniform sky=0 or intrinsic sky=15
    // have no initial seam residual. Sky=15 beside dark requires the dark
    // section to be entirely opaque/nonemitting, so neither side can change.
    // This skips INITIAL seeds only: the solver still propagates every actual
    // correction through all six neighbours, including these section faces.
    for (index, slot) in slots.iter().enumerate() {
        for (face, peer) in slot.adjacent.iter().copied().enumerate() {
            // Virtual air is rebuilt with block=0, not retained from the
            // resident cache. Existing emitters must seed its incoming faces
            // too, or a cached diagonal emitter could disappear behind an
            // otherwise certified Open/Open newly loaded seam.
            let virtual_ingress =
                slot.chunk.is_some() && peer.is_some_and(|peer| slots[peer].chunk.is_none());
            if !slot.seed && !virtual_ingress {
                continue;
            }
            // Seed both sides once when a batch admits adjacent chunks.
            if peer.is_some_and(|peer| slots[peer].seed && peer < index) {
                continue;
            }
            for section in 0..volume / 4096 {
                if !slot.seed
                    && slot
                        .light
                        .block
                        .section(section)
                        .is_none_or(|light| light.is_zero())
                {
                    continue;
                }
                if slot.seed
                    && admission_seam_equilibrated(
                        slot,
                        peer.map(|peer| &slots[peer]),
                        section,
                        table,
                    )
                {
                    continue;
                }
                for ly in section * 16..(section + 1) * 16 {
                    for edge in 0..16 {
                        let (x, z, peer_x, peer_z) = match face {
                            0 => (0, edge, 15, edge),
                            1 => (15, edge, 0, edge),
                            2 => (edge, 0, edge, 15),
                            _ => (edge, 15, edge, 0),
                        };
                        if !slot.seed && slot.light.block.get(x, ly, z) <= 1 {
                            continue;
                        }
                        if slot.seed && slot.light.block.get(x, ly, z) == 0 {
                            let sky = slot.light.sky.get(x, ly, z);
                            // Valid interiors can change initially only across
                            // this seam. Equal dark/source sky cells and zero
                            // block light already satisfy both sides, even in
                            // a mixed section. Later real changes still enqueue
                            // these cells through the normal six-neighbour walk.
                            if (sky == 0 || sky == 15)
                                && peer.map_or(sky == 15, |peer| {
                                    slots[peer].light.block.get(peer_x, ly, peer_z) == 0
                                        && slots[peer].light.sky.get(peer_x, ly, peer_z) == sky
                                })
                            {
                                continue;
                            }
                        }
                        if !scratch_ready {
                            // Every queued cell clears its mark when popped.
                            // Only newly grown storage needs initialization;
                            // all-uniform bulk admission allocates no scratch.
                            queued.resize(queued.len().max(slots.len() * volume), 0);
                            scratch_ready = true;
                        }
                        if slot.seed {
                            enqueue_admission(
                                queue,
                                queued,
                                index * volume + ly * 256 + z * 16 + x,
                            );
                        }
                        if let Some(peer) = peer {
                            enqueue_admission(
                                queue,
                                queued,
                                peer * volume + ly * 256 + peer_z * 16 + peer_x,
                            );
                        }
                    }
                }
            }
        }
    }
}

fn admission_seam_equilibrated(
    left: &AdmissionSlot<'_>,
    right: Option<&AdmissionSlot<'_>>,
    section: usize,
    table: &BlockLightTable,
) -> bool {
    let Some(left_sky) = admission_uniform_zero_block_section(&left.light, section) else {
        return false;
    };
    let Some(right_sky) = right.map_or(Some(15), |slot| {
        admission_uniform_zero_block_section(&slot.light, section)
    }) else {
        return false;
    };
    if left_sky == right_sky {
        return true;
    }
    // The classifier returns only0/15. A dark cached section need not be
    // opaque (e.g. previously enclosed water): zero light alone cannot prove
    // it stays dark when a missing-air neighbour starts supplying sky=15.
    if left_sky == 0 {
        admission_section_is_opaque(left, section, table)
    } else {
        right.is_some_and(|slot| admission_section_is_opaque(slot, section, table))
    }
}

fn admission_uniform_zero_block_section(light: &ChunkLight, section: usize) -> Option<u8> {
    if !light.block.section(section)?.is_zero() {
        return None;
    }
    let sky = light.sky.section(section)?;
    if sky.is_zero() {
        Some(0)
    } else if sky.shared_allocation().is_none() && sky.byte(0) == 0xff {
        Some(15)
    } else {
        None
    }
}

fn admission_section_is_opaque(
    slot: &AdmissionSlot<'_>,
    section: usize,
    table: &BlockLightTable,
) -> bool {
    let Some(section) = slot.chunk.and_then(|chunk| chunk.sections.get(section)) else {
        return false;
    };
    let opaque = |state: BlockStateId| {
        table.opacity(state.0) == Some(15) && table.emission(state.0).unwrap_or(0) == 0
    };
    if let Some(palette) = section.palette() {
        palette.iter().all(|state| opaque(*state))
    } else {
        opaque(section.get(0, 0, 0))
    }
}

fn enqueue_admission(queue: &mut VecDeque<usize>, queued: &mut [u8], cell: usize) {
    if queued[cell] == 0 {
        queued[cell] = 1;
        queue.push_back(cell);
    }
}

fn admission_neighbours(
    slots: &[AdmissionSlot<'_>],
    cell: usize,
    volume: usize,
) -> [Option<usize>; 6] {
    let slot = &slots[cell / volume];
    let local = cell % volume;
    let (x, ly, z) = (local % 16, local / 256, local / 16 % 16);
    [
        if x > 0 {
            Some(cell - 1)
        } else {
            slot.adjacent[0].map(|index| index * volume + local + 15)
        },
        if x < 15 {
            Some(cell + 1)
        } else {
            slot.adjacent[1].map(|index| index * volume + local - 15)
        },
        if z > 0 {
            Some(cell - 16)
        } else {
            slot.adjacent[2].map(|index| index * volume + local + 240)
        },
        if z < 15 {
            Some(cell + 16)
        } else {
            slot.adjacent[3].map(|index| index * volume + local - 240)
        },
        (ly > 0).then(|| cell - 256),
        (ly + 1 < volume / 256).then(|| cell + 256),
    ]
}

fn neighbours(
    chunks: &FxHashMap<ChunkPos, Arc<Chunk>>,
    centre: ChunkPos,
) -> [[Option<&Chunk>; 3]; 3] {
    std::array::from_fn(|dz| {
        std::array::from_fn(|dx| {
            chunks
                .get(&ChunkPos {
                    x: centre.x + dx as i32 - 1,
                    z: centre.z + dz as i32 - 1,
                })
                .map(Arc::as_ref)
        })
    })
}

fn has_complete_light(chunk: &Chunk) -> bool {
    chunk.section_lights.len() == chunk.geometry().section_count()
        && chunk
            .section_lights
            .iter()
            .all(|section| section.sky.is_some() && section.block.is_some())
}

/// Prepare omitted light off tick; complete arrays belong to the producer's
/// final block image and need no rebake (including prelit template/native loads).
/// Scratch is retained per producer thread, never allocated per chunk request.
pub(crate) fn bake_missing(chunk: &mut Chunk, table: &BlockLightTable) {
    if has_complete_light(chunk) {
        return;
    }
    thread_local! {
        static WORKSPACE: std::cell::RefCell<LightWorkspace> =
            std::cell::RefCell::new(LightWorkspace::new());
    }
    let light = WORKSPACE.with(|workspace| {
        compute_chunk_light_in(
            &mut workspace.borrow_mut(),
            [
                [None, None, None],
                [None, Some(&*chunk), None],
                [None, None, None],
            ],
            table,
        )
    });
    chunk.set_baked_light(&light);
}

/// Time/weather darkening affects gameplay, never the stored sky-light channel.
/// The 26.1.2 day timeline is the old periodic-spawn oracle's piecewise curve,
/// not the legacy celestial-angle cosine. Midnight sky brightness is four.
pub(crate) fn sky_darken(day_time: i64, rain_level: f32, thunder_level: f32) -> u8 {
    let tick = day_time.rem_euclid(24_000) as f32;
    let factor = if (133.0..=11_867.0).contains(&tick) {
        1.0
    } else if (11_867.0..13_670.0).contains(&tick) {
        daylight_lerp(tick, 11_867.0, 13_670.0, 1.0, 0.266_666_68)
    } else if (13_670.0..=22_330.0).contains(&tick) {
        0.266_666_68
    } else {
        let wrapped = if tick < 133.0 { tick + 24_000.0 } else { tick };
        daylight_lerp(wrapped, 22_330.0, 24_133.0, 0.266_666_68, 1.0)
    };
    let clear_darken = 15.0 - 15.0 * factor;
    let weather = (1.0 - rain_level * 5.0 / 16.0) * (1.0 - thunder_level * rain_level * 5.0 / 16.0);
    (11.0 - (11.0 - clear_darken) * weather) as u8
}

fn daylight_lerp(tick: f32, start: f32, end: f32, from: f32, to: f32) -> f32 {
    from + (to - from) * ((tick - start) / (end - start))
}

impl crate::tick::TickState {
    /// Real local brightness for crop growth and spawning, after sky darkening.
    #[must_use]
    pub(crate) fn brightness(&self, x: i32, y: i32, z: i32) -> u8 {
        self.world.light_at(x, y, z).map_or(0, |(sky, block)| {
            if !self.dimension.has_weather() {
                return block;
            }
            block.max(sky.saturating_sub(sky_darken(
                self.day_time,
                self.rain_level,
                self.thunder_level,
            )))
        })
    }

    /// Vanilla `getRawBrightness(pos, 0)`: sky and block light with no
    /// day/weather sky darkening. Crops, stems and bamboo saplings gate on
    /// this, so open-sky farms grow at night; spawning and saplings
    /// (`getMaxLocalRawBrightness`) use the darkened [`Self::brightness`].
    #[must_use]
    pub(crate) fn raw_brightness(&self, x: i32, y: i32, z: i32) -> u8 {
        self.world
            .light_at(x, y, z)
            .map_or(0, |(sky, block)| sky.max(block))
    }
}

#[cfg(test)]
#[path = "light_tests.rs"]
mod tests;
