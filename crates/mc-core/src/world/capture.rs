//! Dirty-chunk capture for checkpoints and block-entity injection.
//! Moved verbatim from `world.rs`.

use super::*;

impl ChunkWorld {
    /// Inject checkpoint block-entity storage into a chunk's typed maps
    /// (cleared first for this chunk's columns so removals persist).
    pub(super) fn inject_block_entities(
        chunk: &mut mc_world::Chunk,
        pos: ChunkPos,
        block_entities: &crate::core::CheckpointBlockEntities,
        registry: &BlockRegistry,
    ) {
        chunk.chests.retain(|block, chest| {
            !Self::in_chunk(*block, pos)
                || (chest.loot_table.is_some()
                    && !block_entities.contains_key(&(block.x, block.y, block.z)))
        });
        chunk
            .furnaces
            .retain(|block, _| !Self::in_chunk(*block, pos));
        chunk
            .hoppers
            .retain(|block, _| !Self::in_chunk(*block, pos));
        chunk
            .brewing_stands
            .retain(|block, _| !Self::in_chunk(*block, pos));
        for ((x, y, z), record) in block_entities {
            if !Self::in_chunk(
                mc_world::BlockPos {
                    x: *x,
                    y: *y,
                    z: *z,
                },
                pos,
            ) {
                continue;
            }
            let block = mc_world::BlockPos {
                x: *x,
                y: *y,
                z: *z,
            };
            // Native opaque owners (including three-slot shelves and nine-slot
            // crafters) must never be retyped according to inventory size.
            use mc_data::block_entity_kinds::BlockEntityKind;
            let kind = chunk
                .get_block(x.rem_euclid(16) as u8, *y, z.rem_euclid(16) as u8)
                .and_then(|id| registry.by_id(id))
                .and_then(|state| BlockEntityKind::for_block(state.block.id.path()));
            if matches!(
                kind,
                Some(
                    BlockEntityKind::Furnace
                        | BlockEntityKind::Smoker
                        | BlockEntityKind::BlastFurnace
                )
            ) {
                chunk.furnaces.insert(
                    block,
                    mc_world::FurnaceBlockEntity {
                        slots: Self::furnace_slots(&record.slots),
                        burn_remaining: record
                            .furnace
                            .as_ref()
                            .map(|furnace| furnace.lit_remaining as i16)
                            .unwrap_or(0),
                        burn_total: record
                            .furnace
                            .as_ref()
                            .map(|furnace| furnace.lit_total as i16)
                            .unwrap_or(0),
                        cook_progress: record
                            .furnace
                            .as_ref()
                            .map(|furnace| furnace.cook_progress as i16)
                            .unwrap_or(0),
                        cook_total: record
                            .furnace
                            .as_ref()
                            .map(|furnace| furnace.cook_total as i16)
                            .unwrap_or(200),
                        recipes_used: record
                            .furnace
                            .as_ref()
                            .map(|furnace| (*furnace.recipes_used).clone())
                            .unwrap_or_default(),
                    },
                );
            } else if matches!(
                kind,
                Some(
                    BlockEntityKind::Chest
                        | BlockEntityKind::TrappedChest
                        | BlockEntityKind::Barrel
                )
            ) {
                let mut slots: [mc_world::FurnaceSlot; 27] =
                    std::array::from_fn(|_| mc_world::FurnaceSlot::default());
                for (index, stack) in record.slots.iter().take(27).enumerate() {
                    slots[index] = mc_world::FurnaceSlot {
                        count: stack.count,
                        item_id: stack.item_id,
                        damage: stack.damage,
                        enchantments: stack.enchantments.clone(),
                        custom_name: stack.custom_name.as_deref().map(str::to_owned),
                        item_model: stack.item_model.clone(),
                        stew_effects: stack.stew_effects.clone(),
                        components: stack.components.clone(),
                    };
                }
                chunk.chests.insert(
                    block,
                    mc_world::ChestBlockEntity {
                        slots,
                        loot_table: None,
                        loot_seed: 0,
                    },
                );
            }
        }
    }

    pub(super) fn in_chunk(block: mc_world::BlockPos, chunk: ChunkPos) -> bool {
        block.x.div_euclid(16) == chunk.x && block.z.div_euclid(16) == chunk.z
    }

    pub(super) fn furnace_slots(slots: &[mc_data::ItemStack]) -> [mc_world::FurnaceSlot; 3] {
        let mut out: [mc_world::FurnaceSlot; 3] =
            std::array::from_fn(|_| mc_world::FurnaceSlot::default());
        for (index, stack) in slots.iter().take(3).enumerate() {
            out[index] = mc_world::FurnaceSlot {
                count: stack.count,
                item_id: stack.item_id,
                damage: stack.damage,
                enchantments: stack.enchantments.clone(),
                custom_name: stack.custom_name.as_deref().map(str::to_owned),
                item_model: stack.item_model.clone(),
                stew_effects: stack.stew_effects.clone(),
                components: stack.components.clone(),
            };
        }
        out
    }

    /// Capture the chunks a checkpoint must write, on the tick thread.
    /// Block-entity storage is injected into the chunks' typed maps first,
    /// so native chunks retain their full `block_entities`; pending absent
    /// chunks load-native-or-generate first so a container change is never
    /// lost to eviction. The capture shares the chunks by `Arc` (later
    /// edits copy on write) and leaves them dirty until [`Self::mark_flushed`].
    /// Absent inhabited/scheduler images are frozen metadata only; the journal
    /// reads their effective chunk and prepares derived light after the WAL gate.
    pub fn capture_dirty(
        &mut self,
        block_entities: &crate::core::CheckpointBlockEntities,
    ) -> ChunkCapture {
        let capture_start = std::time::Instant::now();
        let mut capture = ChunkCapture {
            source: Arc::clone(&self.source),
            chunks: Vec::new(),
            deferred: HashMap::default(),
            load_error: None,
            generated_bases: false,
        };
        // Fold resident intervals only at checkpoint; absent images are frozen
        // below without loading or admitting their chunks. WAL stores boundaries.
        let inhabited: Vec<_> = self
            .inhabited_time
            .iter()
            .filter_map(|(pos, counter)| counter.dirty.then_some(*pos))
            .collect();
        for pos in inhabited {
            if let Some(chunk) = self.chunks.get_mut(&pos) {
                self.inhabited_time[&pos]
                    .image
                    .apply(Arc::make_mut(chunk), self.inhabited_tick);
            }
            let counter = self.inhabited_time.get_mut(&pos).unwrap();
            counter.dirty = false;
            self.dirty.insert(pos);
            self.bump_storage_revision(pos);
        }
        let inhabited_time = capture_start.elapsed();
        if capture.source.native.is_none() {
            return capture;
        }
        // Block writes whose chunk is not resident (journal replay before the
        // chunk loaded, or a chunk still loading off-tick) live only in the
        // overlay. This checkpoint supersedes their journal records, so make
        // those chunks resident and fold the writes before encoding.
        let homeless: HashSet<ChunkPos> = self
            .overlay
            .keys()
            .map(|(x, _, z)| ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            })
            .filter(|pos| !self.chunks.contains_key(pos))
            .collect();
        for pos in homeless {
            if !self.materialize(pos) {
                capture.load_error = self.failed.get(&pos).cloned();
            }
        }
        self.fold_overlay();
        // Absent chunks with pending block-entity changes: make them
        // resident through load-or-generate so the same write path persists
        // them (their block base comes from disk or worldgen).
        for pos in std::mem::take(&mut self.dirty_absent) {
            if self.materialize(pos) {
                self.dirty.insert(pos);
            } else {
                capture.load_error = self.failed.get(&pos).cloned();
                self.dirty_absent.insert(pos);
            }
        }
        let materialize_time = capture_start.elapsed();
        // Changed debt rides the chunk checkpoint. Clean live anchors and
        // unchanged nonzero debt already ride the manifest; advancing live
        // time must not rewrite every active chunk on every save.
        // Projection is durable-only and never changes the wire revision.
        let debt: Vec<ChunkPos> = self
            .sim_debt
            .iter()
            .filter_map(|(pos, debt)| debt.dirty.then_some(*pos))
            .collect();
        for pos in debt {
            if !self.chunks.contains_key(&pos) {
                // Non-resident debt rides the deferred path (frozen image,
                // journal-thread application, revision-fenced ACK) — never
                // dropped pre-ACK.
                let (last_sim, remaining) = self
                    .sim_debt
                    .get(&pos)
                    .map(|debt| (debt.last_sim, debt.remaining))
                    .unwrap_or((0, 0));
                self.dirty.insert(pos);
                self.bump_storage_revision(pos);
                if let Some(deferred) = self.defer_checkpoint_chunk(pos, &mut capture) {
                    deferred.debt = Some(crate::journal::SimDebtImage {
                        chunk: [pos.x, pos.z],
                        last_sim,
                        remaining,
                        active: false,
                        tick: self.inhabited_tick,
                    });
                }
                continue;
            }
            if let Some(chunk) = self.chunks.get_mut(&pos) {
                let (last_sim, remaining) = self
                    .sim_debt
                    .get(&pos)
                    .map(|debt| (debt.last_sim, debt.remaining))
                    .unwrap_or((0, 0));
                if let Err(error) = crate::debt::project(Arc::make_mut(chunk), last_sim, remaining)
                {
                    capture.load_error = Some(ChunkLoadError {
                        dimension: self.source.dimension,
                        pos,
                        reason: error.to_string(),
                    });
                    self.dirty.insert(pos);
                    continue;
                }
                // Projection changes the stored image: keep the fence until ACK.
                self.dirty.insert(pos);
                self.bump_storage_revision(pos);
            }
            if let Some(debt) = self.sim_debt.get_mut(&pos) {
                debt.dirty = false;
            }
        }
        let dirty: Vec<ChunkPos> = self.dirty.iter().copied().collect();
        for pos in dirty {
            if !self.chunks.contains_key(&pos) {
                if let Some(counter) = self.inhabited_time.get(&pos) {
                    let image = crate::journal::InhabitedTimeImage {
                        value: counter.image.value_at(self.inhabited_tick),
                        tick: self.inhabited_tick,
                        active: false,
                        ..counter.image
                    };
                    if let Some(deferred) = self.defer_checkpoint_chunk(pos, &mut capture) {
                        deferred.inhabited = Some(image);
                    }
                }
                continue;
            }
            let chunk = self.chunks.get_mut(&pos).unwrap();
            if let Some(counter) = self.inhabited_time.get(&pos)
                && chunk.inhabited_time() != counter.image.value_at(self.inhabited_tick)
            {
                counter
                    .image
                    .apply(Arc::make_mut(chunk), self.inhabited_tick);
            }
            // Typed inventory/progress projection is durable-only. It must
            // not invalidate a client's already delivered chunk image.
            let has_block_entities = !chunk.chests.is_empty()
                || !chunk.furnaces.is_empty()
                || block_entities.keys().any(|(x, y, z)| {
                    Self::in_chunk(
                        mc_world::BlockPos {
                            x: *x,
                            y: *y,
                            z: *z,
                        },
                        pos,
                    )
                });
            if has_block_entities {
                Self::inject_block_entities(
                    Arc::make_mut(chunk),
                    pos,
                    block_entities,
                    &self.source.registry,
                );
                self.bump_storage_revision(pos);
            }
            capture.chunks.push(self.captured_chunk(pos));
        }
        if std::env::var("MC_CORE_PHASE_LOG").is_ok() {
            eprintln!(
                "checkpoint chunks ms: inhabited={:.2} materialize={:.2} project={:.2} chunks={}",
                inhabited_time.as_secs_f64() * 1000.0,
                (materialize_time - inhabited_time).as_secs_f64() * 1000.0,
                (capture_start.elapsed() - materialize_time).as_secs_f64() * 1000.0,
                capture.chunks.len() + capture.deferred.len(),
            );
        }
        capture
    }

    /// A checkpoint wrote these `(position, revision)`s: chunks still at
    /// that revision are clean; newer edits stay dirty for the next one.
    pub fn mark_flushed(&mut self, written: &[(ChunkPos, u64)]) {
        for (pos, revision) in written {
            if self.revisions.get(pos) == Some(revision) {
                self.dirty.remove(pos);
                self.dirty_sections.remove(pos);
                if let Some(debt) = self.sim_debt.get_mut(pos) {
                    debt.dirty = false;
                }
                self.eviction_check.insert(*pos);
            }
        }
    }

    /// Frozen image of a resident chunk for the next checkpoint. Edits after
    /// this call copy on write and bump the revision, so the ACK fence keeps
    /// them (and this mask's bits) for the next capture.
    pub(super) fn captured_chunk(&self, pos: ChunkPos) -> CapturedChunk {
        CapturedChunk {
            pos,
            revision: self.revisions.get(&pos).copied().unwrap_or(0),
            chunk: Arc::clone(&self.chunks[&pos]),
            dirty_sections: Some(self.dirty_sections.get(&pos).copied().unwrap_or(0)),
        }
    }

    /// Capture, write and acknowledge in one call (tests).
    #[cfg(test)]
    pub fn flush_dirty(
        &mut self,
        block_entities: &crate::core::CheckpointBlockEntities,
    ) -> Result<usize, CoreError> {
        let capture = self.capture_dirty(block_entities);
        let written = capture.write()?;
        self.mark_flushed(&written);
        Ok(written.len())
    }
}
