//! Village POI cache: claims, lookups and invalidation.
//! Moved verbatim from `brain.rs`.

use super::*;

impl PoiCache {
    pub(super) fn admitted(&self, slot: EntitySlot) -> bool {
        // Direct transaction tests use an unscheduled cache. During tick every
        // existing slot has an explicit admission bit, including non-villagers.
        self.full_brains.is_empty()
            || self
                .full_brains
                .get(slot.0 as usize)
                .copied()
                .unwrap_or(false)
    }

    pub(super) fn actors(&self) -> &[EntitySlot] {
        if self.full_brains.is_empty() {
            &self.residents
        } else {
            &self.selected
        }
    }

    pub(super) fn prepare_brains(&mut self, state: &mut TickState) {
        for &slot in &self.selected {
            if let Some(full) = self.full_brains.get_mut(slot.0 as usize) {
                *full = false;
            }
        }
        self.selected.clear();
        self.full_brains.resize(state.entities.len(), false);
        let schedules = [
            schedule(0, Profession::Nitwit, state.day_time),
            schedule(-1, Profession::None, state.day_time),
            schedule(0, Profession::None, state.day_time),
        ];
        let schedule_changed = self.schedules != Some(schedules);
        self.schedules = Some(schedules);
        self.next_player_chunks.clear();
        for &id in state.players.keys() {
            if let Some(slot) = state.entities.slot_of(id) {
                let pos = state.entities.pos(slot);
                self.next_player_chunks.push((
                    (pos[0].floor() as i32).div_euclid(16),
                    (pos[2].floor() as i32).div_euclid(16),
                ));
            }
        }
        self.next_player_chunks.sort_unstable();
        self.next_player_chunks.dedup();
        let players_changed = self.player_chunks != self.next_player_chunks;
        if players_changed {
            std::mem::swap(&mut self.player_chunks, &mut self.next_player_chunks);
            self.player_distance
                .retain(|&(x, z), _| self.loaded_chunks.contains(&ChunkPos { x, z }));
            for (chunk, ids) in state.entities.villager_chunks() {
                if !self.loaded_chunks.contains(chunk) {
                    continue;
                }
                let cell = (chunk.x, chunk.z);
                let previous = self.player_distance.get(&cell).copied().unwrap_or(i32::MAX);
                let distance = player_chunk_distance(&self.player_chunks, cell);
                self.player_distance.insert(cell, distance);
                if (previous <= 2) != (distance <= 2)
                    || [1, 2, 4, 16, 32].into_iter().any(|period| {
                        brain_period(period, previous) != brain_period(period, distance)
                    })
                {
                    self.profile_updates.extend(ids.iter().copied());
                }
            }
        }
        self.urgent_chunks.clear();
        for &(pos, range) in &self.threats {
            let cx = (pos[0].floor() as i32).div_euclid(16);
            let cz = (pos[2].floor() as i32).div_euclid(16);
            let radius = (range / 16.0).ceil() as i32;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    self.urgent_chunks.insert((cx + dx, cz + dz));
                }
            }
        }
        if self.merchants != self.previous_merchants {
            self.profile_updates.extend(self.merchants.iter().copied());
            self.profile_updates
                .extend(self.previous_merchants.iter().copied());
            self.previous_merchants.clear();
            self.previous_merchants.extend_from_slice(&self.merchants);
        }
        if self.urgent_chunks != self.previous_urgent_chunks {
            for &(x, z) in self.urgent_chunks.union(&self.previous_urgent_chunks) {
                if let Some(ids) = state.entities.villager_chunks().get(&ChunkPos { x, z }) {
                    self.profile_updates.extend(ids.iter().copied());
                    if self.urgent_chunks.contains(&(x, z))
                        && !self.previous_urgent_chunks.contains(&(x, z))
                    {
                        for id in ids {
                            if let Some(&index) = self.resident_index.get(id) {
                                self.resident_data[index].wake = true;
                            }
                        }
                    }
                }
            }
            self.previous_urgent_chunks.clear();
            self.previous_urgent_chunks
                .extend(self.urgent_chunks.iter().copied());
        }
        for chunk in &self.motion_chunks {
            if let Some(ids) = state.entities.villager_chunks().get(chunk) {
                self.profile_updates.extend(ids.iter().copied());
            }
        }
        #[cfg(test)]
        let measure_all = self.measure_decisions;
        #[cfg(not(test))]
        let measure_all = false;
        if self.rebuild_schedule || schedule_changed || measure_all {
            self.profile_updates
                .extend(self.resident_data.iter().map(|brain| brain.id));
        }
        self.profile_updates.sort_unstable();
        self.profile_updates.dedup();
        for update in 0..self.profile_updates.len() {
            if let Some(&index) = self.resident_index.get(&self.profile_updates[update]) {
                self.classify_brain(state, index, measure_all);
            }
        }
        self.profile_updates.clear();
        self.rebuild_schedule = false;
        while let Some(&Reverse((due, id, version))) = self.brain_due.peek() {
            if due > state.tick {
                break;
            }
            self.brain_due.pop();
            let Some(&index) = self.resident_index.get(&id) else {
                continue;
            };
            let brain = &mut self.resident_data[index];
            if brain.due_at != due || brain.due_version != version {
                continue;
            }
            brain.due_at = u64::MAX;
            self.classify_brain(state, index, measure_all);
        }
        if self.brain_due.len() > self.resident_data.len().saturating_mul(2).max(64) {
            let residents = &self.resident_data;
            let indexes = &self.resident_index;
            self.brain_due.retain(|Reverse((due, id, version))| {
                indexes.get(id).is_some_and(|&index| {
                    residents[index].due_at == *due && residents[index].due_version == *version
                })
            });
        }
        let ready = self
            .brain_bands
            .iter()
            .flatten()
            .map(|bits| bits.count_ones() as usize)
            .sum();
        state
            .tick_budget
            .note_demand(state.dimension, Consumer::Brain, Kind::Decision, ready);
        if measure_all {
            for band in 0..self.brain_bands.len() {
                self.admit_band(band, usize::MAX, state);
            }
        } else {
            let capacity =
                state
                    .tick_budget
                    .allowance(state.dimension, Consumer::Brain, Kind::Decision);
            let near_wakes = self.admit_band(0, capacity, state);
            let near_share = self.near_residents.div_ceil(20).min(capacity * 3 / 4);
            self.admit_band(
                1,
                (self.selected.len() + near_share.saturating_sub(near_wakes)).min(capacity),
                state,
            );
            for band in 2..self.brain_bands.len() {
                self.admit_band(band, capacity, state);
            }
            self.admit_band(1, capacity, state);
        }
        self.execution_indices.clear();
        for word in 0..self.execution_bits.len() {
            let mut bits = self.execution_bits[word];
            while bits != 0 {
                let index = word * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                self.execution_indices.push(index);
            }
        }
    }

    pub(super) fn admit_band(&mut self, band: usize, limit: usize, state: &mut TickState) -> usize {
        let tick = state.tick;
        #[cfg(test)]
        let charge_budget = !self.measure_decisions;
        #[cfg(not(test))]
        let charge_budget = true;
        let count = self.admission_order.len();
        let start = self.brain_cursors[band].map_or(0, |cursor| {
            self.admission_order
                .partition_point(|&index| self.resident_data[index].id <= cursor)
        });
        let mut near = 0;
        for (begin, end) in [(start, count), (0, start)] {
            let mut rank = begin;
            while rank < end && self.selected.len() < limit {
                let word = rank / 64;
                let mut bits = self.brain_bands[band][word] & (u64::MAX << (rank % 64));
                if end / 64 == word && end % 64 != 0 {
                    bits &= (1_u64 << (end % 64)) - 1;
                }
                if bits == 0 {
                    rank = (word + 1) * 64;
                    continue;
                }
                rank = word * 64 + bits.trailing_zeros() as usize;
                let index = self.admission_order[rank];
                let slot = self.residents[index];
                if !self.full_brains[slot.0 as usize]
                    && charge_budget
                    && !state.tick_budget.try_charge(
                        state.dimension,
                        Consumer::Brain,
                        Kind::Decision,
                        1,
                    )
                {
                    return near;
                }
                self.brain_bands[band][word] &= !(1 << (rank % 64));
                let brain = &mut self.resident_data[index];
                brain.ready_band = None;
                if !self.full_brains[slot.0 as usize] {
                    brain.wake = false;
                    near += usize::from(brain.near);
                    self.full_brains[slot.0 as usize] = true;
                    self.execution_bits[index / 64] |= 1 << (index % 64);
                    brain.cadence.next = tick.saturating_add(brain.cadence.period);
                    brain.last_full = tick;
                    self.brain_cursors[band] = Some(brain.id);
                    self.selected.push(slot);
                    self.queue_brain(index, tick);
                }
                rank += 1;
            }
        }
        near
    }

    pub(super) fn activity_for(&self, age: i32, profession: Profession, day_time: i64) -> Activity {
        let class = if age < 0 {
            1
        } else if profession == Profession::Nitwit {
            0
        } else {
            2
        };
        self.schedules.map_or_else(
            || schedule(age, profession, day_time),
            |schedules| schedules[class],
        )
    }

    pub(super) fn queue_brain(&mut self, index: usize, tick: u64) {
        let brain = &mut self.resident_data[index];
        let due = if !brain.active {
            u64::MAX
        } else if let Some(band) = brain.ready_band {
            if band >= 3 {
                brain.last_full.saturating_add(31)
            } else {
                u64::MAX
            }
        } else {
            let mut due = brain
                .cadence
                .next
                .min(brain.wake_at)
                .min(brain.search_due)
                .min(brain.schedule_due);
            due = due.min(brain.last_full.saturating_add(31));
            if brain.protected_until > tick {
                due = due.min(brain.protected_until);
            }
            due
        };
        if due != brain.due_at {
            brain.due_at = due;
            brain.due_version = brain.due_version.wrapping_add(1);
            if due != u64::MAX {
                self.brain_due
                    .push(Reverse((due, brain.id, brain.due_version)));
            }
        }
    }

    pub(super) fn classify_brain(&mut self, state: &TickState, index: usize, force: bool) {
        let brain = self.resident_data[index];
        if let Some(band) = brain.ready_band {
            self.brain_bands[band][brain.rank / 64] &= !(1 << (brain.rank % 64));
        }
        let cell = (brain.chunk.x, brain.chunk.z);
        let distance = *self
            .player_distance
            .entry(cell)
            .or_insert_with(|| player_chunk_distance(&self.player_chunks, cell));
        let busy = self.merchants.binary_search(&brain.id).is_ok();
        let scheduled = self.activity_for(
            if state.tick < brain.baby_until { -1 } else { 0 },
            brain.profession,
            state.day_time,
        );
        let brain = &mut self.resident_data[index];
        brain.near = distance <= 2;
        let counted = brain.active && brain.near;
        if counted != brain.counted_near {
            if counted {
                self.near_residents += 1;
            } else {
                self.near_residents -= 1;
            }
            brain.counted_near = counted;
        }
        let protected = busy
            || brain.profession == Profession::Soldier
            || state.tick < brain.protected_until
            || brain.reactive
            || self.urgent_chunks.contains(&cell);
        brain.wake |= (busy && !brain.cadence.was_busy)
            || brain.hurt_at == Some(state.tick)
            || (self.motion_revision.is_some() && self.motion_chunks.contains(&brain.chunk));
        let period = if protected || brain.cadence.was_busy {
            1
        } else {
            brain_period(brain.cadence.period, distance)
        };
        if period < brain.cadence.period || brain.wake {
            brain.cadence.next = brain.cadence.next.min(state.tick);
        }
        brain.cadence.period = period;
        brain.cadence.was_busy = busy;
        if state.tick >= brain.panic_until && scheduled != brain.activity {
            if brain.schedule_due == u64::MAX {
                brain.schedule_due = state
                    .tick
                    .saturating_add(u64::from(brain.id.0.unsigned_abs()) % SCHEDULE_WINDOW);
            }
        } else {
            brain.schedule_due = u64::MAX;
        }
        let service_due = brain.last_full.saturating_add(31);
        let due = brain.wake
            || state.tick >= brain.cadence.next
            || state.tick >= brain.wake_at
            || state.tick >= brain.search_due
            || state.tick >= brain.schedule_due
            || state.tick >= service_due
            || force
            || brain.ready_band.is_some();
        brain.ready_band = if brain.active && due {
            Some(if brain.wake {
                0
            } else if brain.near {
                1
            } else if state.tick >= service_due {
                2
            } else {
                match period {
                    1 => 3,
                    2 => 4,
                    4 => 5,
                    16 => 6,
                    _ => 7,
                }
            })
        } else {
            None
        };
        if let Some(band) = brain.ready_band {
            self.brain_bands[band][brain.rank / 64] |= 1 << (brain.rank % 64);
        }
        self.queue_brain(index, state.tick);
        self.update_execution(index);
    }

    pub(super) fn update_execution(&mut self, index: usize) {
        let brain = &self.resident_data[index];
        let bit = 1 << (index % 64);
        if brain.active
            && (brain.clock_changes || brain.motion || self.motion_chunks.contains(&brain.chunk))
        {
            self.execution_bits[index / 64] |= bit;
        } else {
            self.execution_bits[index / 64] &= !bit;
        }
    }

    pub(crate) fn kind(&self, id: u32) -> Option<PoiKind> {
        self.kinds.get(id as usize).copied().flatten()
    }

    pub(crate) fn items_near(&mut self, pos: [f64; 3]) -> &[EntitySlot] {
        self.nearby_items.clear();
        let min_x = ((pos[0] - PICKUP_REACH).floor() as i32).div_euclid(16);
        let max_x = ((pos[0] + PICKUP_REACH).floor() as i32).div_euclid(16);
        let min_z = ((pos[2] - PICKUP_REACH).floor() as i32).div_euclid(16);
        let max_z = ((pos[2] + PICKUP_REACH).floor() as i32).div_euclid(16);
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                if let Some(items) = self.items.get(&(x, z)) {
                    self.nearby_items.extend_from_slice(items);
                }
            }
        }
        // Preserve the original dense-slot custody order across chunk edges.
        self.nearby_items.sort_unstable_by_key(|slot| slot.0);
        &self.nearby_items
    }

    pub(super) fn refresh_residents(&mut self, state: &TickState) {
        #[cfg(test)]
        {
            self.roster_us = 0;
            self.roster_rebuilds = 0;
        }
        // Birth reserves a bed before the child joins the indexed roster.
        // Remove that reservation first, then rebuild it from the live row;
        // a child removed or converted in between must not leave an orphan.
        while let Some((id, home)) = self.newborn_homes.pop() {
            self.update_claims(id, [Some(home), None, None], [None; 3]);
        }
        self.updates.clear();
        self.updates.extend(self.changed.iter().copied());
        self.updates
            .extend(state.entities.brain_changes().iter().copied());
        if self.resident_revision != Some(state.entities.brain_revision()) {
            #[cfg(test)]
            let roster_start = std::time::Instant::now();
            // Swap-removing a nonresident can move a villager's dense slot
            // without changing membership. Preserve its stable brain/rank.
            let slots_only = self.resident_revision.is_some()
                && !self.updates.is_empty()
                && self.updates.iter().all(|id| {
                    self.resident_index.contains_key(id)
                        && state.entities.slot_of_any(*id).is_some_and(|slot| {
                            let pos = state.entities.pos(slot);
                            let chunk = ChunkPos {
                                x: (pos[0].floor() as i32).div_euclid(16),
                                z: (pos[2].floor() as i32).div_euclid(16),
                            };
                            state
                                .entities
                                .villager_chunks()
                                .get(&chunk)
                                .is_some_and(|ids| ids.contains(id))
                        })
                });
            if slots_only {
                for id in &self.updates {
                    self.residents[self.resident_index[id]] =
                        state.entities.slot_of_any(*id).unwrap();
                }
            } else {
                std::mem::swap(&mut self.resident_data, &mut self.previous_data);
                std::mem::swap(&mut self.resident_index, &mut self.previous_index);
                self.resident_data.clear();
                self.resident_index.clear();
                self.residents.clear();
                for ids in state.entities.villager_chunks().values() {
                    self.residents
                        .extend(ids.iter().filter_map(|&id| state.entities.slot_of_any(id)));
                }
                self.residents.sort_unstable_by_key(|slot| slot.0);
                for index in 0..self.residents.len() {
                    let slot = self.residents[index];
                    let id = state.entities.body(slot).id;
                    let brain = if let Some(&previous) = self.previous_index.get(&id) {
                        self.previous_data[previous]
                    } else {
                        let brain = ResidentBrain::read(state, slot);
                        self.update_claims(id, [None; 3], brain.claims);
                        self.updates.push(id);
                        brain
                    };
                    self.resident_index.insert(id, index);
                    self.resident_data.push(brain);
                }
                self.admission_order.clear();
                self.admission_order.extend(0..self.residents.len());
                self.admission_order
                    .sort_unstable_by_key(|&index| self.resident_data[index].id);
                let words = self.residents.len().div_ceil(64);
                for band in &mut self.brain_bands {
                    band.clear();
                    band.resize(words, 0);
                }
                self.execution_bits.clear();
                self.execution_bits.resize(words, 0);
                self.brain_due.clear();
                self.near_residents = 0;
                self.rebuild_schedule = true;
                for rank in 0..self.admission_order.len() {
                    let index = self.admission_order[rank];
                    let brain = &mut self.resident_data[index];
                    brain.rank = rank;
                    brain.ready_band = None;
                    brain.due_at = u64::MAX;
                    brain.counted_near = false;
                }
                for index in 0..self.previous_data.len() {
                    let previous = self.previous_data[index];
                    if !self.resident_index.contains_key(&previous.id) {
                        self.update_claims(previous.id, previous.claims, [None; 3]);
                        self.updates.push(previous.id);
                        self.resident_threats.remove(&previous.id);
                        self.search_at.remove(&previous.id);
                    }
                }
            }
            self.resident_revision = Some(state.entities.brain_revision());
            #[cfg(test)]
            {
                self.roster_us = roster_start.elapsed().as_micros();
                self.roster_rebuilds = usize::from(!slots_only);
            }
        }
        self.loaded_chunks
            .retain(|chunk| state.entities.villager_chunks().contains_key(chunk));
        self.active_chunks
            .retain(|chunk| state.entities.villager_chunks().contains_key(chunk));
        for (chunk, ids) in state.entities.villager_chunks() {
            let changed = if state.world.has_chunk(chunk.x, chunk.z) {
                self.loaded_chunks.insert(*chunk)
            } else {
                self.loaded_chunks.remove(chunk)
            };
            let activity_changed = if state.world.is_active(chunk.x, chunk.z) {
                self.active_chunks.insert(*chunk)
            } else {
                self.active_chunks.remove(chunk)
            };
            if changed || activity_changed {
                self.updates.extend(ids.iter().copied());
            }
        }
        self.motion_chunks.clear();
        if Some(state.world.revision_now()) != self.motion_revision {
            for &chunk in &self.loaded_chunks {
                if self.motion_revision.is_none_or(|previous| {
                    (-1..=1).any(|dx| {
                        (-1..=1).any(|dz| {
                            let revision = state.world.chunk_revision(chunk.x + dx, chunk.z + dz);
                            revision != u64::MAX && revision > previous
                        })
                    })
                }) {
                    self.motion_chunks.insert(chunk);
                }
            }
        }
        for edited in state.world.pending_block_chunks() {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let chunk = ChunkPos {
                        x: edited.x + dx,
                        z: edited.z + dz,
                    };
                    if self.loaded_chunks.contains(&chunk) {
                        self.motion_chunks.insert(chunk);
                    }
                }
            }
        }
        self.updates.sort_unstable();
        self.updates.dedup();
        for update in 0..self.updates.len() {
            let id = self.updates[update];
            self.refresh_resident(state, id);
            self.dirty_claim(id);
        }
        let count = self.resident_data.len();
        for _ in 0..count.div_ceil(128) {
            self.validation_cursor %= count;
            let id = self.resident_data[self.validation_cursor].id;
            self.validation_cursor += 1;
            self.dirty_claim(id);
        }
    }

    pub(super) fn refresh_resident(&mut self, state: &TickState, id: EntityId) {
        let Some(&index) = self.resident_index.get(&id) else {
            return;
        };
        // Zombie villagers share resident custody through infection/cure.
        let hostile = match state.entities.body(self.residents[index]).kind {
            EntityKind::Mob { type_id } => {
                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                    .is_some_and(|contract| {
                        contract
                            .name
                            .strip_prefix("minecraft:")
                            .unwrap_or(contract.name)
                            == "zombie_villager"
                    })
            }
            _ => false,
        };
        if hostile {
            self.resident_threats.insert(id);
        } else {
            self.resident_threats.remove(&id);
        }
        let mut brain = ResidentBrain::read(state, self.residents[index]);
        brain.search_due = *self.search_at.entry(id).or_insert_with(|| {
            let phase = (state.tick % 20 + u64::from(id.0.unsigned_abs()) % 20) % 20;
            state.tick.saturating_add((20 - phase) % 20)
        });
        let previous = self.resident_data[index];
        brain.cadence = previous.cadence;
        brain.near = previous.near;
        brain.rank = previous.rank;
        brain.due_at = previous.due_at;
        brain.due_version = previous.due_version;
        brain.ready_band = previous.ready_band;
        brain.counted_near = previous.counted_near;
        brain.schedule_due = previous.schedule_due;
        brain.last_full = previous.last_full;
        brain.wake = previous.wake
            || brain.claims != previous.claims
            || (brain.hurt_at != previous.hurt_at && brain.hurt_at.is_some())
            || (brain.active && !previous.active);
        self.update_claims(id, previous.claims, brain.claims);
        if brain.active && !previous.active {
            brain.cadence.next = state.tick;
        }
        if brain != previous {
            self.profile_updates.push(id);
        }
        self.resident_data[index] = brain;
        self.update_execution(index);
    }

    pub(crate) fn refresh_allocator_updates(&mut self, state: &TickState) {
        self.allocator_updates.sort_unstable();
        self.allocator_updates.dedup();
        for update in 0..self.allocator_updates.len() {
            self.refresh_resident(state, self.allocator_updates[update]);
        }
        self.allocator_updates.clear();
    }

    pub(crate) fn refresh(&mut self, state: &TickState) {
        self.food.refresh(&state.registry.items);
        if self.kinds.is_empty() {
            self.kinds
                .resize(state.registry.collision_boxes.len(), None);
            for block in state.registry.blocks.states() {
                self.kinds[block.id.0 as usize] = poi_kind(&state.registry, block.id.0);
            }
        }
        std::mem::swap(&mut self.occupied_beds, &mut self.previous_occupied_beds);
        self.occupied_beds.clear();
        self.occupied_beds
            .extend(state.sleepers.values().map(|s| s.bed));
        for pos in self
            .occupied_beds
            .symmetric_difference(&self.previous_occupied_beds)
        {
            if let Some(owners) = self.homes.get(pos) {
                for &id in owners {
                    if self.dirty_claims.insert(id) {
                        self.validation_queue.push_back(id);
                    }
                }
            }
        }
        self.merchants.clear();
        self.merchants
            .extend(state.players.values().filter_map(|player| {
                player
                    .open
                    .as_ref()
                    .and_then(crate::villager::trades::merchant_entity)
            }));
        self.merchants.sort_unstable();
        self.threats.clear();
        self.threat_max_range = 0.0;
        for threats in self.threat_chunks.values_mut() {
            threats.clear();
        }
        for items in self.items.values_mut() {
            items.clear();
        }
        self.golems.clear();
        self.refresh_residents(state);
        for chunk in self
            .revised_chunks
            .iter()
            .chain(state.world.pending_block_chunks())
        {
            if let Some(owners) = self.claim_chunks.get(chunk) {
                for &id in owners {
                    if self.dirty_claims.insert(id) {
                        self.validation_queue.push_back(id);
                    }
                }
            }
        }
        self.refresh_sensor_chunks(state);
        let near_sensor_chunks = &self.near_sensor_chunks;
        for ids in state
            .entities
            .brain_sensor_chunks()
            .iter()
            .filter(|(pos, _)| near_sensor_chunks.contains(pos))
            .map(|(_, ids)| ids)
            .chain(std::iter::once(&self.resident_threats))
        {
            for &id in ids {
                let Some(slot) = state.entities.slot_of(id) else {
                    continue;
                };
                let body = state.entities.body(slot);
                match body.kind {
                    EntityKind::Item { .. } => {
                        let pos = state.entities.pos(slot);
                        let cell = (
                            (pos[0].floor() as i32).div_euclid(16),
                            (pos[2].floor() as i32).div_euclid(16),
                        );
                        self.items.entry(cell).or_default().push(slot);
                    }
                    EntityKind::Mob { type_id } => {
                        if let Some(contract) =
                            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(
                                type_id,
                            )
                        {
                            let name = contract
                                .name
                                .strip_prefix("minecraft:")
                                .unwrap_or(contract.name);
                            if let Some(range) = threat_range(name) {
                                let pos = state.entities.pos(slot);
                                let chunk = (
                                    (pos[0].floor() as i32).div_euclid(16),
                                    (pos[2].floor() as i32).div_euclid(16),
                                );
                                self.threat_chunks
                                    .entry(chunk)
                                    .or_default()
                                    .push(self.threats.len());
                                self.threats.push((pos, range));
                                self.threat_max_range = self.threat_max_range.max(range);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        self.threat_chunks.retain(|_, threats| !threats.is_empty());
        self.items.retain(|_, items| !items.is_empty());
        self.refresh_poi_revisions(state);
        // FIFO admission makes every changed resident chunk progress even if
        // another chunk changes again before its turn. Scan the latest image.
        for _ in 0..POI_REFRESH_CHUNKS_PER_TICK {
            let Some(pos) = self.pending_chunks.pop_front() else {
                break;
            };
            let Some(chunk) = state.world.chunk(pos) else {
                self.chunks.remove(&pos);
                continue;
            };
            let Some(mut entry) = self.chunks.remove(&pos) else {
                continue;
            };
            for (index, section) in chunk.sections.iter().enumerate() {
                let has_poi = match section.palette() {
                    Some(palette) => palette.iter().any(|id| self.kind(id.0).is_some()),
                    None => self.kind(section.get(0, 0, 0).0).is_some(),
                };
                if !has_poi {
                    continue;
                }
                for y in 0..16u8 {
                    for z in 0..16u8 {
                        for x in 0..16u8 {
                            let pos = BlockPos {
                                x: chunk.pos.x * 16 + i32::from(x),
                                y: state.dimension.geometry().min_y()
                                    + index as i32 * 16
                                    + i32::from(y),
                                z: chunk.pos.z * 16 + i32::from(z),
                            };
                            if let Some(kind) = state
                                .world
                                .block_at(pos.x, pos.y, pos.z)
                                .and_then(|id| self.kind(id))
                            {
                                entry.pois.push(Poi { pos, kind });
                            }
                        }
                    }
                }
            }
            entry.revision = state.world.chunk_revision(chunk.pos.x, chunk.pos.z);
            entry.queued = false;
            self.chunks.insert(chunk.pos, entry);
        }
    }

    pub(crate) fn claimed(&self, owner: EntityId, poi: Poi) -> bool {
        let claims = match poi.kind {
            PoiKind::Bell => return false,
            PoiKind::Home => {
                if self.occupied_beds.contains(&poi.pos) {
                    return true;
                }
                &self.homes
            }
            PoiKind::Job(_) => &self.jobs,
        };
        claims
            .get(&poi.pos)
            .and_then(|owners| owners.first())
            .is_some_and(|id| *id != owner)
    }

    pub(crate) fn update_claims(
        &mut self,
        owner: EntityId,
        old: [Option<BlockPos>; 3],
        new: [Option<BlockPos>; 3],
    ) {
        for (claims, before, after) in [
            (&mut self.homes, old[0], new[0]),
            (&mut self.jobs, old[1], new[1]),
        ] {
            if before == after {
                continue;
            }
            if let Some(owners) = before.and_then(|pos| claims.get(&pos)) {
                for &id in owners {
                    if self.dirty_claims.insert(id) {
                        self.validation_queue.push_back(id);
                    }
                }
            }
            if let Some(pos) = before
                && let Some(owners) = claims.get_mut(&pos)
            {
                owners.remove(&owner);
                if owners.is_empty() {
                    claims.remove(&pos);
                }
            }
            if let Some(pos) = after {
                claims.entry(pos).or_default().insert(owner);
                for &id in &claims[&pos] {
                    if self.dirty_claims.insert(id) {
                        self.validation_queue.push_back(id);
                    }
                }
            }
        }
        if old != new {
            self.update_claim_chunks(owner, old, new);
            self.dirty_claim(owner);
        }
        if let Some(&index) = self.resident_index.get(&owner) {
            self.resident_data[index].claims = new;
        }
    }

    pub(crate) fn nearby(
        &mut self,
        pos: [f64; 3],
        owner: EntityId,
        matches: impl Fn(PoiKind) -> bool,
    ) {
        self.candidates.clear();
        let cx = (pos[0].floor() as i32).div_euclid(16);
        let cz = (pos[2].floor() as i32).div_euclid(16);
        for x in cx - 3..=cx + 3 {
            for z in cz - 3..=cz + 3 {
                let Some(chunk) = self.chunks.get(&ChunkPos { x, z }) else {
                    continue;
                };
                for &poi in &chunk.pois {
                    if matches(poi.kind) {
                        let d = distance(pos, centre(poi.pos));
                        if d <= 48.0 * 48.0 && !self.claimed(owner, poi) {
                            self.candidates.push((d, poi));
                        }
                    }
                }
            }
        }
        self.candidates.sort_unstable_by(|a, b| {
            b.0.total_cmp(&a.0).then_with(|| {
                (b.1.pos.x, b.1.pos.y, b.1.pos.z).cmp(&(a.1.pos.x, a.1.pos.y, a.1.pos.z))
            })
        });
    }

    #[cfg(test)]
    pub(crate) fn note_path_denial(&mut self) {
        if self.measure_decisions
            && let Some(actor) = self.measure_actor
        {
            self.path_denied.insert(actor);
        }
    }
}
