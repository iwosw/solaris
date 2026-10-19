//! Villager schedules and owner-thread transactions. Constants follow the local
//! 26.1.2 villager brain, population, gossip, and village-defense oracle modules.
use super::{Activity, Gossip, VillagerProfession as Profession, VillagerState};
use crate::boundary::{BlockPos, ChangeSet, ChunkPos, EntityEvent, EntityId};
use crate::entities::{EntityKind, EntitySlot};
#[cfg(test)]
use crate::registry::CoreRegistry;
use crate::settlements::allocator::{
    self, JOB_SITE_REACH_SQUARED, PICKUP_REACH, POI_REFRESH_CHUNKS_PER_TICK, Poi, PoiCache,
    PoiKind, active, at, capture_route, centre, distance, free_bed, poi_kind, route_changed,
    standing, threat_range,
};
use crate::tick::{TickSampler, TickState};
#[cfg(test)]
use crate::tick_budget::DECISIONS_PER_TICK;
use crate::tick_budget::{Consumer, Kind};
use mc_data::ItemStack;
use mc_physics::mth;
#[cfg(test)]
use rustc_hash::FxHashSet;
use std::cmp::Reverse;

// Full decisions spend the global tick allowance shared with settlement
// allocation and other dimensions; the brain never refills it locally.

// Only the ignored all-brains profile enables these timers. Release servers
// compile the expression itself, with no measurement fields or clock reads.
macro_rules! decision_phase {
    ($cache:expr, $index:expr, $body:expr) => {{
        #[cfg(test)]
        let started = $cache.measure_decisions.then(std::time::Instant::now);
        let value = $body;
        #[cfg(test)]
        if let Some(started) = started {
            $cache.decision_current[$index] += started.elapsed().as_nanos();
            $cache.decision_calls[$index] += 1;
        }
        value
    }};
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct BrainCadence {
    period: u64,
    next: u64,
    was_busy: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResidentBrain {
    id: EntityId,
    chunk: ChunkPos,
    active: bool,
    near: bool,
    activity: Activity,
    profession: Profession,
    baby_until: u64,
    wake_at: u64,
    search_due: u64,
    panic_until: u64,
    protected_until: u64,
    hurt_at: Option<u64>,
    wake: bool,
    reactive: bool,
    motion: bool,
    clock_changes: bool,
    claims: [Option<BlockPos>; 3],
    cadence: BrainCadence,
    rank: usize,
    due_at: u64,
    due_version: u64,
    ready_band: Option<usize>,
    counted_near: bool,
    schedule_due: u64,
    last_full: u64,
}

impl ResidentBrain {
    fn read(state: &TickState, slot: EntitySlot) -> Self {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        let pos = state.entities.pos(slot);
        let chunk = ChunkPos {
            x: (pos[0].floor() as i32).div_euclid(16),
            z: (pos[2].floor() as i32).div_euclid(16),
        };
        let loaded = state.world.is_active(chunk.x, chunk.z);
        let mut brain = Self {
            id: body.id,
            chunk,
            active: false,
            near: false,
            activity: Activity::Idle,
            profession: Profession::None,
            baby_until: 0,
            wake_at: u64::MAX,
            search_due: u64::MAX,
            panic_until: 0,
            protected_until: 0,
            hurt_at: body.hurt_at,
            wake: false,
            reactive: false,
            motion: false,
            clock_changes: false,
            claims: [None; 3],
            cadence: BrainCadence::default(),
            rank: 0,
            due_at: u64::MAX,
            due_version: 0,
            ready_band: None,
            counted_near: false,
            schedule_due: u64::MAX,
            last_full: state.tick,
        };
        let Some(v) = row.villager.as_ref() else {
            return brain;
        };
        brain.active = loaded && active(state, slot);
        brain.activity = v.activity;
        brain.profession = v.data.profession;
        // This is a no-op proof, not deferred clock projection: every clock
        // which can change still advances through the shared projection now.
        brain.clock_changes =
            v.age != 0 || v.sleeping || matches!(v.activity, Activity::Work | Activity::Panic);
        if v.age < 0 {
            brain.baby_until = state.tick.saturating_add(u64::from(v.age.unsigned_abs()));
        }
        brain.wake_at = v.last_gossip_decay.saturating_add(24_000);
        if v.age != 0 {
            brain.wake_at = brain.wake_at.min(
                state
                    .tick
                    .saturating_add(u64::from(v.age.unsigned_abs()).saturating_sub(1)),
            );
        } else if body.height < 1.0 {
            brain.wake_at = state.tick;
        }
        if v.navigation_target.is_some() {
            brain.wake_at = brain.wake_at.min(v.navigation_repath_at);
        }
        brain.panic_until = v.panic_until;
        brain.protected_until = v
            .panic_until
            .max(body.hurt_at.map_or(0, |at| at.saturating_add(100)));
        brain.reactive = v.partner.is_some() || !v.navigation_path.is_empty();
        brain.motion = !v.opened_doors.is_empty()
            || (!v.sleeping
                && (!state.entities.ground(slot)
                    || state.entities.vel(slot) != [0.0; 3]
                    || state.entities.horizontal_impulse(slot) != [0.0; 2]
                    || !v.navigation_path.is_empty()));
        if body.alive && v.conversion_time.is_none() {
            let dimension = state.dimension.name();
            brain.claims = [
                v.home.filter(|_| v.home_dimension == dimension),
                v.job_site.filter(|_| v.job_site_dimension == dimension),
                v.meeting_point
                    .filter(|_| v.meeting_point_dimension == dimension),
            ];
        }
        brain
    }
}

fn player_chunk_distance(players: &[(i32, i32)], cell: (i32, i32)) -> i32 {
    players
        .iter()
        .map(|&(x, z)| cell.0.abs_diff(x).max(cell.1.abs_diff(z)))
        .min()
        .unwrap_or(i32::MAX as u32)
        .min(i32::MAX as u32) as i32
}

fn brain_period(previous: u64, chunks: i32) -> u64 {
    // Retain a ring one chunk beyond its outer boundary; promote immediately
    // at the inner boundary. A player moving on an edge cannot flap the LOD.
    match previous {
        1 if chunks <= 3 => 1,
        2 if (3..=5).contains(&chunks) => 2,
        4 if (5..=7).contains(&chunks) => 4,
        16 if (7..=9).contains(&chunks) => 16,
        32 if chunks > 8 => 32,
        _ => match chunks {
            0..=2 => 1,
            3..=4 => 2,
            5..=6 => 4,
            7..=8 => 16,
            _ => 32,
        },
    }
}

#[derive(Clone, Copy, Default)]
struct FoodItem {
    points: u8,
    farmer_pickup: bool,
}

#[derive(Default)]
pub(crate) struct FoodTable {
    items: std::sync::Weak<mc_data::items::ItemRegistry>,
    entries: Vec<FoodItem>,
}

impl FoodTable {
    pub(crate) fn refresh(&mut self, items: &std::sync::Arc<mc_data::items::ItemRegistry>) {
        if self.items.as_ptr() == std::sync::Arc::as_ptr(items) {
            return;
        }
        self.entries.clear();
        for (_, id) in items.iter() {
            // The reverse map is authoritative, including duplicate protocol IDs.
            let name = items.name_of(id).expect("registered item has a name");
            let points = match name.path() {
                "bread" => 4,
                "carrot" | "potato" | "beetroot" => 1,
                _ => 0,
            };
            let farmer_pickup = matches!(name.path(), "wheat" | "wheat_seeds" | "beetroot_seeds");
            if points == 0 && !farmer_pickup {
                continue;
            }
            let index = id as usize;
            if self.entries.len() <= index {
                self.entries.resize(index + 1, FoodItem::default());
            }
            self.entries[index] = FoodItem {
                points,
                farmer_pickup,
            };
        }
        // Keeping a weak reference prevents allocation reuse and makes
        // Arc::make_mut extensions acquire a new registry generation.
        self.items = std::sync::Arc::downgrade(items);
    }
}

impl PoiCache {
    fn admitted(&self, slot: EntitySlot) -> bool {
        // Direct transaction tests use an unscheduled cache. During tick every
        // existing slot has an explicit admission bit, including non-villagers.
        self.full_brains.is_empty()
            || self
                .full_brains
                .get(slot.0 as usize)
                .copied()
                .unwrap_or(false)
    }

    fn actors(&self) -> &[EntitySlot] {
        if self.full_brains.is_empty() {
            &self.residents
        } else {
            &self.selected
        }
    }

    fn prepare_brains(&mut self, state: &mut TickState) {
        for &slot in &self.selected {
            if let Some(full) = self.full_brains.get_mut(slot.0 as usize) {
                *full = false;
            }
        }
        self.selected.clear();
        self.full_brains.resize(state.entities.len(), false);
        if self.schedule_window == 0 {
            self.schedule_window =
                if std::thread::available_parallelism().is_ok_and(|n| n.get() <= 2) {
                    600
                } else {
                    200
                };
        }
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

    fn admit_band(&mut self, band: usize, limit: usize, state: &mut TickState) -> usize {
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

    fn activity_for(&self, age: i32, profession: Profession, day_time: i64) -> Activity {
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

    fn queue_brain(&mut self, index: usize, tick: u64) {
        let brain = &mut self.resident_data[index];
        let due = if !brain.active {
            u64::MAX
        } else if let Some(band) = brain.ready_band {
            if band >= 3 {
                brain
                    .last_full
                    .saturating_add(if self.schedule_window == 600 { 63 } else { 31 })
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
            due = due.min(
                brain
                    .last_full
                    .saturating_add(if self.schedule_window == 600 { 63 } else { 31 }),
            );
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

    fn classify_brain(&mut self, state: &TickState, index: usize, force: bool) {
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
        let period = if period == 32 && self.schedule_window == 600 {
            64
        } else {
            period
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
                    .saturating_add(u64::from(brain.id.0.unsigned_abs()) % self.schedule_window);
            }
        } else {
            brain.schedule_due = u64::MAX;
        }
        let service_due = brain
            .last_full
            .saturating_add(if self.schedule_window == 600 { 63 } else { 31 });
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

    fn update_execution(&mut self, index: usize) {
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

    fn refresh_residents(&mut self, state: &TickState) {
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

    fn refresh_resident(&mut self, state: &TickState, id: EntityId) {
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
                    .and_then(super::trades::merchant_entity)
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

pub(super) fn bodies_near(
    state: &TickState,
    pos: [f64; 3],
    radius: f64,
    residents_only: bool,
) -> impl Iterator<Item = EntitySlot> + '_ {
    let min_x = ((pos[0] - radius).floor() as i32).div_euclid(16);
    let max_x = ((pos[0] + radius).floor() as i32).div_euclid(16);
    let min_z = ((pos[2] - radius).floor() as i32).div_euclid(16);
    let max_z = ((pos[2] + radius).floor() as i32).div_euclid(16);
    (min_x..=max_x).flat_map(move |x| {
        (min_z..=max_z).flat_map(move |z| {
            let chunk = ChunkPos { x, z };
            let villagers = state.entities.villager_chunks().get(&chunk);
            let sensors = if residents_only {
                None
            } else {
                state.entities.brain_sensor_chunks().get(&chunk)
            };
            villagers
                .into_iter()
                .chain(sensors)
                .flat_map(|ids| ids.iter().copied())
                .filter_map(|id| state.entities.slot_of(id))
        })
    })
}

pub(super) fn schedule(age: i32, profession: Profession, day_time: i64) -> Activity {
    let time = day_time.rem_euclid(24_000);
    if profession == Profession::Nitwit && age >= 0 {
        return if !(2_000..14_000).contains(&time) {
            Activity::Rest
        } else {
            Activity::Idle
        };
    }
    if age < 0 {
        match time {
            0..=9 | 12_000.. => Activity::Rest,
            3_000..=5_999 | 10_000..=11_999 => Activity::Play,
            _ => Activity::Idle,
        }
    } else {
        match time {
            0..=9 | 12_000.. => Activity::Rest,
            2_000..=8_999 => Activity::Work,
            9_000..=10_999 => Activity::Meet,
            _ => Activity::Idle,
        }
    }
}

fn clear_step(state: &TickState, begin: [f64; 3], end: [f64; 3], width: f64, height: f64) -> bool {
    // A jump needs headroom over the starting column; a drop crosses the
    // edge horizontally before falling. Keep A* and short strolls identical.
    let travel_y = begin[1].max(end[1]);
    allocator::clear_body_for_route(
        &(state).world,
        &(state).registry,
        [begin[0], travel_y, begin[2]],
        width,
        height,
        true,
    ) && allocator::clear_body_for_route(
        &(state).world,
        &(state).registry,
        [
            (begin[0] + end[0]) * 0.5,
            travel_y,
            (begin[2] + end[2]) * 0.5,
        ],
        width,
        height,
        true,
    )
}

fn wander_to(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    target: Option<BlockPos>,
    scheduled: Activity,
) {
    let pos = state.entities.pos(slot);
    let mut unchanged = false;
    if let Some(target) = target {
        let new_target = v.navigation_target != Some(target);
        let due = state.tick >= v.navigation_repath_at;
        unchanged = !v.navigation_path.is_empty() && !route_changed(state, v);
        // Failed/exhausted strolls keep their deadline, even when an unknown
        // neighboring chunk would otherwise report invalidation every tick.
        if new_target || due || (!v.navigation_path.is_empty() && !unchanged) {
            let body = state.entities.body(slot);
            let (width, height) = (body.half_width, body.height);
            let start = at(pos);
            let mut cell = start;
            let nx = start.x.abs_diff(target.x);
            let nz = start.z.abs_diff(target.z);
            let mut walked_x = 0;
            let mut walked_z = 0;
            v.navigation_path.clear();
            if nx.saturating_add(nz) <= 6 {
                while cell.x != target.x || cell.z != target.z {
                    // Cardinal rasterization of the straight segment: never
                    // cut a diagonal corner or search around an obstruction.
                    let along_x = walked_z == nz
                        || (walked_x < nx && (2 * walked_x + 1) * nz <= (2 * walked_z + 1) * nx);
                    let (dx, dz) = if along_x {
                        walked_x += 1;
                        ((target.x - cell.x).signum(), 0)
                    } else {
                        walked_z += 1;
                        (0, (target.z - cell.z).signum())
                    };
                    let begin = allocator::standing_for_route(
                        &(state).world,
                        &(state).registry,
                        cell,
                        width,
                        height,
                        true,
                    )
                    .unwrap_or_else(|| centre(cell));
                    let next = [0, 1, -1, -2, -3].into_iter().find_map(|dy| {
                        let next = BlockPos {
                            x: cell.x + dx,
                            y: cell.y + dy,
                            z: cell.z + dz,
                        };
                        let end = allocator::standing_for_route(
                            &(state).world,
                            &(state).registry,
                            next,
                            width,
                            height,
                            true,
                        )?;
                        clear_step(state, begin, end, width, height).then_some(next)
                    });
                    let Some(next) = next else {
                        // An incomplete stroll must not move toward the wall.
                        v.navigation_path.clear();
                        break;
                    };
                    v.navigation_path.push(next);
                    cell = next;
                }
            }
            v.navigation_path.reverse();
            v.navigation_target = Some(target);
            if new_target || due {
                v.navigation_repath_at = state.tick + 40;
            }
            capture_route(state, v, pos, true);
            unchanged = !route_changed(state, v);
        }
    } else {
        v.navigation_path.clear();
        v.navigation_target = None;
        v.navigation_chunks.clear();
    }
    move_committed_with_route(state, slot, v, scheduled, unchanged);
}

fn nearest_threat(cache: &PoiCache, pos: [f64; 3]) -> Option<[f64; 3]> {
    let range = cache.threat_max_range;
    let min_x = ((pos[0] - range).floor() as i32).div_euclid(16);
    let max_x = ((pos[0] + range).floor() as i32).div_euclid(16);
    let min_z = ((pos[2] - range).floor() as i32).div_euclid(16);
    let max_z = ((pos[2] + range).floor() as i32).div_euclid(16);
    (min_x..=max_x)
        .flat_map(|x| (min_z..=max_z).map(move |z| (x, z)))
        .filter_map(|chunk| cache.threat_chunks.get(&chunk))
        .flatten()
        .filter_map(|&index| {
            let (at, range) = cache.threats[index];
            let d = distance(pos, at);
            (d <= range * range).then_some((d, index, at))
        })
        // Chunk traversal must not change the original sensor-order tie break.
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|(_, _, at)| at)
}
/// Test-only immutable A/B reference: disabling both caches leaves the same
/// single-standing algorithm and ordinary per-tick collision integration.
#[cfg(test)]
const DISABLE_MOTION_GEOMETRY: bool = false;

/// Route-local sampled block states and the next standing point. Neither cache
/// participates in durable equality or journal-image cloning.
#[derive(Debug, Default)]
pub(crate) struct MotionGeometryCache {
    blocks: Vec<u32>,
    min: [i32; 3],
    max: [i32; 3],
    body: [f64; 2],
    standing: Option<(BlockPos, Option<[f64; 3]>)>,
    revision: u64,
    pending: bool,
    #[cfg(test)]
    disable: bool,
    #[cfg(test)]
    geometry_hits: usize,
    #[cfg(test)]
    geometry_misses: usize,
    #[cfg(test)]
    standing_hits: usize,
}

impl Clone for MotionGeometryCache {
    fn clone(&self) -> Self {
        #[cfg(test)]
        {
            Self {
                disable: self.disable,
                ..Self::default()
            }
        }
        #[cfg(not(test))]
        {
            Self::default()
        }
    }
}

impl PartialEq for MotionGeometryCache {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl MotionGeometryCache {
    fn enabled(&self) -> bool {
        #[cfg(test)]
        {
            !DISABLE_MOTION_GEOMETRY && !self.disable
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    // Preserve allocation capacity and the test-only reference-mode flag.
    pub(crate) fn clear(&mut self) {
        self.blocks.clear();
        self.standing = None;
    }

    fn begin(&mut self, state: &TickState, v: &VillagerState, width: f64, height: f64) -> bool {
        let unchanged = !route_changed(state, v);
        self.begin_route(state, v, width, height, unchanged);
        unchanged
    }

    fn begin_route(
        &mut self,
        state: &TickState,
        v: &VillagerState,
        width: f64,
        height: f64,
        unchanged: bool,
    ) {
        // Door flips preserve a villager's route, not its physical shapes.
        let pending = !state.world.overlay_is_empty()
            && v.navigation_chunks
                .iter()
                .any(|&(chunk, _)| state.world.pending_block_chunks().contains(&chunk));
        let physical_changed = self.pending
            || pending
            || (state.world.revision_now() != self.revision
                && v.navigation_chunks.iter().any(|&(chunk, _)| {
                    let revision = state.world.chunk_revision(chunk.x, chunk.z);
                    revision == u64::MAX || revision > self.revision
                }));
        if !unchanged || physical_changed || self.body != [width, height] || !self.enabled() {
            self.clear();
            self.body = [width, height];
        }
        self.revision = state.world.revision_now();
        // A cache sampled from the overlay cannot survive its fold, even when
        // a second toggle restores the original state without a wire revision.
        self.pending = pending;
    }

    fn standing(
        &mut self,
        state: &TickState,
        v: &VillagerState,
        cell: BlockPos,
        unchanged: bool,
    ) -> Option<[f64; 3]> {
        let [width, height] = self.body;
        let x = f64::from(cell.x) + 0.5;
        let z = f64::from(cell.z) + 0.5;
        let covered = unchanged
            && self.enabled()
            && route_covers_columns(
                v,
                [(x - width).floor() as i32, (z - width).floor() as i32],
                [(x + width).floor() as i32, (z + width).floor() as i32],
            );
        if covered
            && let Some((cached_cell, point)) = self.standing
            && cached_cell == cell
        {
            #[cfg(test)]
            {
                self.standing_hits += 1;
            }
            return point;
        }
        let point = allocator::standing(&(state).world, &(state).registry, cell, width, height);
        if covered {
            self.standing = Some((cell, point));
        }
        point
    }

    fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if self.blocks.is_empty()
            || x < self.min[0]
            || x > self.max[0]
            || y < self.min[1]
            || y > self.max[1]
            || z < self.min[2]
            || z > self.max[2]
        {
            return None;
        }
        let index = ((x - self.min[0]) as usize * (self.max[1] - self.min[1] + 1) as usize
            + (y - self.min[1]) as usize)
            * (self.max[2] - self.min[2] + 1) as usize
            + (z - self.min[2]) as usize;
        self.blocks.get(index).copied()
    }

    fn sample_motion(
        &mut self,
        state: &TickState,
        v: &VillagerState,
        body: mc_physics::EntityBody,
        desired: mc_physics::Vec3,
        unchanged: bool,
    ) -> bool {
        if !unchanged || !self.enabled() {
            return false;
        }
        let p = body.position;
        let w = body.aabb.half_width;
        let h = body.aabb.height;
        if ![p.x, p.y, p.z, w, h, desired.x, desired.y, desired.z]
            .into_iter()
            .all(f64::is_finite)
            || w <= 0.0
            || h <= 0.0
            || state.registry.max_collision_box_y == 0
        {
            return false;
        }
        // Match the ordinary resolver's swept-body/step scan, with no eager
        // halo prefill. Per-query misses still use TickSampler if that scan ever
        // grows additional sampling requirements.
        let step = mc_physics::STEP_HEIGHT;
        let local_y = f64::from(state.registry.max_collision_box_y) / 16.0;
        let min = [
            (p.x - w + desired.x.min(0.0)).floor() as i32,
            ((p.y + desired.y.min(-step) - local_y).floor() as i32).saturating_add(1),
            (p.z - w + desired.z.min(0.0)).floor() as i32,
        ];
        let max = [
            ((p.x + w + desired.x.max(0.0)).ceil() as i32).saturating_sub(1),
            ((p.y + h + desired.y.max(step)).ceil() as i32).saturating_sub(1),
            ((p.z + w + desired.z.max(0.0)).ceil() as i32).saturating_sub(1),
        ];
        let Some(cells) = (0..3).try_fold(1usize, |count, axis| {
            let len = usize::try_from(i64::from(max[axis]) - i64::from(min[axis]) + 1).ok()?;
            count.checked_mul(len)
        }) else {
            return false;
        };
        // Large impulses/bodies retain the ordinary bounded collision scan;
        // operational storage is reserved for a small local motion envelope.
        if !(1..=512).contains(&cells) {
            return false;
        }
        if !route_covers_columns(v, [min[0], min[2]], [max[0], max[2]]) {
            return false;
        }
        if !self.blocks.is_empty()
            && (0..3).all(|axis| min[axis] >= self.min[axis] && max[axis] <= self.max[axis])
        {
            #[cfg(test)]
            {
                self.geometry_hits += 1;
            }
            return true;
        }
        self.blocks.clear();
        #[cfg(test)]
        {
            self.geometry_misses += 1;
        }
        self.blocks.reserve(cells);
        for x in min[0]..=max[0] {
            for y in min[1]..=max[1] {
                for z in min[2]..=max[2] {
                    let Some(id) = state.world.physics_block_at(x, y, z) else {
                        self.blocks.clear();
                        return false;
                    };
                    if state.registry.collision_boxes.get(id as usize).is_none() {
                        self.blocks.clear();
                        return false;
                    }
                    self.blocks.push(id);
                }
            }
        }
        self.min = min;
        self.max = max;
        true
    }
}

/// Coverage is only a scope fence. Route validity uses the capability-aware
/// world proof; physical caches separately observe wire revisions and pending edits.
fn route_covers_columns(v: &VillagerState, min: [i32; 2], max: [i32; 2]) -> bool {
    (min[0].div_euclid(16)..=max[0].div_euclid(16)).all(|x| {
        (min[1].div_euclid(16)..=max[1].div_euclid(16)).all(|z| {
            v.navigation_chunks
                .iter()
                .any(|&(chunk, _)| chunk == ChunkPos { x, z })
        })
    })
}

struct MotionSampler<'a> {
    live: TickSampler<'a>,
    geometry: Option<&'a MotionGeometryCache>,
}

impl mc_physics::BlockSampler for MotionSampler<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> mc_physics::BlockMaterial {
        self.geometry
            .and_then(|geometry| geometry.block_at(x, y, z))
            .map_or_else(
                || self.live.material_at(x, y, z),
                |id| self.live.registry.materials.classify(id),
            )
    }

    fn max_collision_box_y(&self) -> u8 {
        self.live.max_collision_box_y()
    }

    fn collision_boxes_at(
        &self,
        x: i32,
        y: i32,
        z: i32,
        emit: &mut dyn FnMut(mc_physics::BlockCollisionBox),
    ) {
        if let Some(boxes) = self
            .geometry
            .and_then(|geometry| geometry.block_at(x, y, z))
            .and_then(|id| self.live.registry.collision_boxes.get(id as usize))
        {
            for &collision_box in boxes {
                emit(collision_box);
            }
        } else {
            self.live.collision_boxes_at(x, y, z, emit);
        }
    }
}
fn move_to(
    state: &mut TickState,
    cache: &mut PoiCache,
    slot: EntitySlot,
    v: &mut VillagerState,
    target: Option<BlockPos>,
    exact: bool,
) {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let (width, height) = (body.half_width, body.height);
    let mut unchanged = false;
    if let Some(target) = target {
        let new_target = v.navigation_target != Some(target);
        let due = state.tick >= v.navigation_repath_at;
        let changed = route_changed(state, v);
        unchanged = !changed;
        // Progress is observed only at due checks, never every brain step: a
        // shorter path or a closer next cell means the villager is still
        // walking its route. A pushed or blocked villager stalls and must
        // replan from its actual position instead of keeping a stale route.
        let mut stalled = false;
        if due && !new_target && !changed && !v.navigation_path.is_empty() {
            let next = v.navigation_path.last().copied();
            let dist_sq = next.map(|cell| distance(pos, centre(cell))).unwrap_or(0.0);
            stalled = !(v.navigation_path.len() < v.navigation_progress_len
                || (next == v.navigation_progress_next && dist_sq < v.navigation_progress_dist_sq));
            v.navigation_progress_len = v.navigation_path.len();
            v.navigation_progress_next = next;
            v.navigation_progress_dist_sq = dist_sq;
        }
        // A due route is reused only while progressing; a stalled villager
        // replans from its current position even when the world is unchanged.
        if (new_target || changed || (due && v.navigation_path.is_empty()) || stalled)
            && cache.spend_path(state, Consumer::Brain)
        {
            let from = at(pos);
            let horizontal = from
                .x
                .abs_diff(target.x)
                .max(from.z.abs_diff(target.z))
                .saturating_add(8)
                .clamp(48, 512);
            let vertical = from.y.abs_diff(target.y).saturating_add(4).clamp(16, 512);
            let mut seek_path = std::mem::take(&mut cache.candidate_path);
            let mut ctx = allocator::RouteCtx {
                world: &state.world,
                registry: &state.registry,
                dimension: state.dimension,
                consumer: Consumer::Brain,
                budget: &mut state.tick_budget,
            };
            let found = allocator::find_path_ctx(
                &mut ctx,
                cache,
                pos,
                target,
                width,
                height,
                &mut seek_path,
                exact,
                horizontal,
                vertical,
                true,
            );
            let exhausted = ctx
                .budget
                .allowance(ctx.dimension, ctx.consumer, Kind::PathNode)
                == 0;
            // Preserve only a budget-denied route whose target and geometry
            // remain valid. Successful zero-step arrival clears the old path.
            if found || !exhausted || new_target || changed || v.navigation_path.is_empty() {
                std::mem::swap(&mut seek_path, &mut v.navigation_path);
                v.navigation_target = Some(target);
                capture_route(state, v, pos, true);
            }
            seek_path.clear();
            cache.candidate_path = seek_path;
            unchanged = !route_changed(state, v);
            // A world change invalidates the route, not the activity's target
            // deadline. Renewing it on every chunk admission freezes a villager
            // at an exhausted path because its next stroll never becomes due.
            if new_target || due {
                v.navigation_repath_at = state.tick + 40;
            }
        } else if changed || new_target {
            // Never follow a stale route while the bounded search is deferred.
            v.navigation_path.clear();
            v.navigation_chunks.clear();
            unchanged = false;
        } else if due && !v.navigation_path.is_empty() {
            // Keep the brain's next wake bounded without rerunning A*.
            v.navigation_repath_at = state.tick + 40;
        }
    } else {
        v.navigation_path.clear();
        v.navigation_target = None;
        v.navigation_chunks.clear();
    }
    let scheduled = cache.activity_for(v.age, v.data.profession, state.day_time);
    move_committed_with_route(state, slot, v, scheduled, unchanged);
}

#[cfg(test)]
fn move_committed(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    scheduled: Activity,
) {
    let unchanged = !route_changed(state, v);
    move_committed_with_route(state, slot, v, scheduled, unchanged);
}

/// Physics and the already committed path only: no sensors or route searches.
/// The proof is reused only within this synchronous movement call.
fn move_committed_with_route(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    scheduled: Activity,
    unchanged: bool,
) {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let (width, height) = (body.half_width, body.height);
    let mut geometry = std::mem::take(&mut v.motion_geometry);
    geometry.begin_route(state, v, width, height, unchanged);
    let next = loop {
        let Some(&cell) = v.navigation_path.last() else {
            break None;
        };
        let standing = geometry.standing(state, v, cell, unchanged);
        let reached = standing.unwrap_or_else(|| centre(cell));
        if (pos[0] - reached[0]).powi(2) + (pos[2] - reached[2]).powi(2) < 0.09
            && (pos[1] - reached[1]).abs() < 0.6
        {
            v.navigation_path.pop();
        } else {
            break standing;
        }
    };
    let motion = resolve_body_motion(
        state,
        slot,
        next,
        walking_speed(v, scheduled),
        v,
        &mut geometry,
        unchanged,
    );
    v.motion_geometry = geometry;
    apply_body_motion(state, slot, motion);
}

fn walking_speed(v: &VillagerState, scheduled: Activity) -> f64 {
    match v.activity {
        Activity::Panic => match scheduled {
            Activity::Idle => 10.0,
            Activity::Play => 7.2,
            _ => 6.0,
        },
        Activity::Play => 3.6,
        Activity::Idle => 5.0,
        _ => 3.0,
    }
}

fn move_skipped(
    state: &mut TickState,
    slot: EntitySlot,
    terrain_changed: bool,
    merchant_busy: bool,
    scheduled: Activity,
) {
    if state.entities.row(slot).villager.as_ref().unwrap().sleeping {
        return;
    }
    let mut geometry = std::mem::take(state.entities.villager_motion_geometry_mut(slot).unwrap());
    let pos = state.entities.pos(slot);
    let row = state.entities.row(slot);
    let v = row.villager.as_ref().unwrap();
    let effective = if state.tick < v.panic_until {
        Activity::Panic
    } else {
        scheduled
    };
    let unchanged = geometry.begin(
        state,
        v,
        state.entities.body(slot).half_width,
        state.entities.body(slot).height,
    );
    let allowed =
        !merchant_busy && effective == v.activity && (v.navigation_path.is_empty() || unchanged);
    let mut reached = 0;
    let mut next = None;
    let mut has_next = false;
    if allowed {
        for &cell in v.navigation_path.iter().rev() {
            let standing = geometry.standing(state, v, cell, unchanged);
            let point = standing.unwrap_or_else(|| centre(cell));
            if (pos[0] - point[0]).powi(2) + (pos[2] - point[2]).powi(2) < 0.09
                && (pos[1] - point[1]).abs() < 0.6
            {
                reached += 1;
            } else {
                has_next = true;
                next = standing;
                break;
            }
        }
    }
    let speed = walking_speed(v, scheduled);
    let still = !terrain_changed
        && !has_next
        && state.entities.ground(slot)
        && state.entities.vel(slot) == [0.0; 3]
        && state.entities.horizontal_impulse(slot) == [0.0; 2];
    let motion = if still {
        None
    } else {
        resolve_body_motion(state, slot, next, speed, v, &mut geometry, unchanged)
    };
    *state.entities.villager_motion_geometry_mut(slot).unwrap() = geometry;
    if reached != 0 {
        state.entities.advance_villager_path(slot, reached);
    }
    apply_body_motion(state, slot, motion);
}

#[derive(Debug, PartialEq)]
struct BodyMotion {
    position: [f64; 3],
    velocity: [f64; 3],
    grounded: bool,
    impulse: [f64; 2],
    yaw: Option<f32>,
}

fn resolve_body_motion(
    state: &TickState,
    slot: EntitySlot,
    next: Option<[f64; 3]>,
    speed: f64,
    v: &VillagerState,
    geometry: &mut MotionGeometryCache,
    unchanged: bool,
) -> Option<BodyMotion> {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let (width, height, impulse) = (
        body.half_width,
        body.height,
        state.entities.horizontal_impulse(slot),
    );
    let mut velocity = state.entities.vel(slot);
    let dt = mc_physics::TICK_SECONDS;
    velocity[1] = (velocity[1] - mc_physics::GRAVITY_BLOCKS_PER_SECOND_SQUARED * dt)
        .max(mc_physics::TERMINAL_VELOCITY_BLOCKS_PER_SECOND);
    let mut horizontal = [0.0; 2];
    if let Some(next) = next {
        let dx = next[0] - pos[0];
        let dz = next[2] - pos[2];
        let length = mth::length2(dx, dz);
        if length > 1e-6 {
            horizontal = [dx / length * speed, dz / length * speed];
        }
        if next[1] > pos[1] + mc_physics::STEP_HEIGHT && state.entities.ground(slot) {
            // JumpControl replaces vertical velocity after gravity: subtracting
            // gravity from the launch impulse prevents clearing a full block.
            velocity[1] = mc_physics::LIVING_JUMP_SPEED_BLOCKS_PER_SECOND;
        }
    }
    velocity = [
        horizontal[0] + impulse[0],
        velocity[1],
        horizontal[1] + impulse[1],
    ];
    let requested = mc_physics::Vec3::new(velocity[0] * dt, velocity[1] * dt, velocity[2] * dt);
    let body = mc_physics::EntityBody {
        position: mc_physics::Vec3::new(pos[0], pos[1], pos[2]),
        velocity: mc_physics::Vec3::ZERO,
        aabb: mc_physics::Aabb {
            half_width: width,
            height,
        },
        on_ground: state.entities.ground(slot),
    };
    let sampled = geometry.sample_motion(state, v, body, requested, unchanged);
    let sampler = MotionSampler {
        live: TickSampler {
            world: &state.world,
            registry: &state.registry,
        },
        geometry: sampled.then_some(&*geometry),
    };
    let fixed = mc_physics::resolve_collision_displacement(
        &sampler,
        body.position,
        body.aabb,
        requested,
        body.on_ground,
        mc_physics::STEP_HEIGHT,
    )?;
    let new = [
        pos[0] + fixed.delta.x,
        pos[1] + fixed.delta.y,
        pos[2] + fixed.delta.z,
    ];
    let drag = if fixed.grounded { 0.91 * 0.6 } else { 0.91 };
    let mut next_impulse = impulse.map(|v| {
        let n = v * drag;
        if n.abs() < 0.06 { 0.0 } else { n }
    });
    if (fixed.delta.x - requested.x).abs() > 1e-7 {
        velocity[0] = 0.0;
        next_impulse[0] = 0.0;
    }
    if (fixed.delta.z - requested.z).abs() > 1e-7 {
        velocity[2] = 0.0;
        next_impulse[1] = 0.0;
    }
    velocity[1] = if fixed.grounded || (fixed.delta.y - requested.y).abs() > 1e-7 {
        0.0
    } else {
        velocity[1] * 0.98
    };
    Some(BodyMotion {
        position: new,
        velocity,
        grounded: fixed.grounded,
        impulse: next_impulse,
        yaw: (horizontal[0] != 0.0 || horizontal[1] != 0.0).then(|| {
            -(mc_physics::mth::atan2(horizontal[0], horizontal[1]) as f32)
                * mc_physics::mth::RAD_TO_DEG
        }),
    })
}

fn apply_body_motion(state: &mut TickState, slot: EntitySlot, motion: Option<BodyMotion>) {
    if let Some(motion) = motion {
        state.entities.set_pos(slot, motion.position);
        state.entities.set_vel(slot, motion.velocity);
        state.entities.set_ground(slot, motion.grounded);
        state.entities.set_brain_impulse(slot, motion.impulse);
        if let Some(yaw) = motion.yaw {
            state.entities.set_rot(slot, yaw, 0.0);
        }
    }
}

fn food_points(food: &FoodTable, item: u32) -> i32 {
    food.entries
        .get(item as usize)
        .map_or(0, |entry| i32::from(entry.points))
}
fn total_food(food: &FoodTable, v: &VillagerState) -> i32 {
    v.food_level
        + v.inventory
            .iter()
            .flatten()
            .map(|s| s.count.max(0) * food_points(food, s.item_id))
            .sum::<i32>()
}
fn willing(food: &FoodTable, v: &VillagerState) -> bool {
    v.age == 0 && !v.sleeping && v.activity != Activity::Panic && total_food(food, v) >= 12
}
fn eat_for_birth(food: &FoodTable, v: &mut VillagerState) {
    for slot in &mut v.inventory {
        if v.food_level >= 12 {
            break;
        }
        if let Some(stack) = slot {
            let points = food_points(food, stack.item_id);
            while points > 0 && stack.count > 0 && v.food_level < 12 {
                stack.count -= 1;
                v.food_level += points;
            }
            if stack.count == 0 {
                *slot = None;
            }
        }
    }
    v.food_level -= 12;
}
pub(super) fn add_inventory(v: &mut VillagerState, moving: &mut ItemStack, maximum: i32) -> i32 {
    let before = moving.count;
    if before <= 0 || maximum <= 0 {
        return 0;
    }
    for slot in &mut v.inventory {
        if let Some(existing) = slot
            && mc_data::inventory_semantics_26_1_2::can_stack(existing, moving)
        {
            let amount = (maximum - existing.count).max(0).min(moving.count);
            existing.count += amount;
            moving.count -= amount;
        }
        if moving.count == 0 {
            return before;
        }
    }
    for slot in &mut v.inventory {
        if slot.is_none() {
            let amount = moving.count.min(maximum);
            let mut stack = moving.clone();
            stack.count = amount;
            *slot = Some(stack);
            moving.count -= amount;
        }
        if moving.count == 0 {
            break;
        }
    }
    before - moving.count
}
fn pickup(
    state: &mut TickState,
    food: &FoodTable,
    items: &[EntitySlot],
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
) {
    if !crate::command_rules::boolean(state, "mob_griefing") || v.sleeping {
        return;
    }
    let pos = state.entities.pos(slot);
    let collector = state.entities.body(slot).id;
    for &item_slot in items {
        let body = state.entities.body(item_slot);
        let row = state.entities.row(item_slot);
        if !body.alive || !matches!(body.kind, EntityKind::Item { .. }) || body.pickup_delay > 0 {
            continue;
        }
        let at = state.entities.pos(item_slot);
        if (pos[0] - at[0]).abs() > PICKUP_REACH
            || (pos[2] - at[2]).abs() > PICKUP_REACH
            || at[1] + 0.25 < pos[1]
            || at[1] > pos[1] + state.entities.body(slot).height
        {
            continue;
        }
        let Some(item) = row.item.as_ref() else {
            continue;
        };
        let edible = food_points(food, item.item_id) > 0;
        let farmer = v.data.profession == Profession::Farmer
            && food
                .entries
                .get(item.item_id as usize)
                .is_some_and(|entry| entry.farmer_pickup);
        if !edible && !farmer {
            continue;
        }
        let mut moving = item.full_stack();
        let id = body.id;
        let maximum = state.registry.item_rules().max_stack(&moving);
        let count = add_inventory(v, &mut moving, maximum);
        if count == 0 {
            continue;
        }
        // Both custodians mutate in this single tick-owner transaction. Dead
        // consumed rows remain slot-stable until all villagers have finished.
        let (body, row) = state.entities.parts_mut(item_slot);
        if moving.count == 0 {
            body.alive = false;
            row.item = None;
        } else {
            if let Some(item) = row.item.as_mut() {
                item.count = moving.count;
                item.stack = Some(moving.clone());
            }
            std::sync::Arc::make_mut(&mut row.appearance).item = Some(moving);
            state.appearance_changed.push(id);
        }
        changes.entity_events.push(EntityEvent::Pickup {
            item: id,
            collector,
            count: count.clamp(0, 255) as u8,
        });
    }
}
fn share_food(
    state: &mut TickState,
    food_table: &FoodTable,
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
) {
    if v.sleeping || v.activity == Activity::Panic || !state.tick.is_multiple_of(20) {
        return;
    }
    let food = v
        .inventory
        .iter()
        .flatten()
        .map(|s| food_points(food_table, s.item_id) * s.count)
        .sum::<i32>();
    if food < 24 {
        return;
    }
    let pos = state.entities.pos(slot);
    let recipient = bodies_near(state, pos, 3.0, true).find(|&other| {
        other != slot
            && active(state, other)
            && distance(pos, state.entities.pos(other)) <= 5.0
            && state
                .entities
                .row(other)
                .villager
                .as_ref()
                .is_some_and(|v| total_food(food_table, v) - v.food_level < 12)
    });
    let Some(recipient) = recipient else {
        return;
    };
    for inventory in &mut v.inventory {
        let Some(stack) = inventory else {
            continue;
        };
        if food_points(food_table, stack.item_id) == 0 {
            continue;
        }
        let maximum = state.registry.item_rules().max_stack(stack);
        let count = if stack.count > maximum / 2 {
            stack.count / 2
        } else {
            (stack.count - 24).max(0)
        };
        if count <= 0 {
            continue;
        }
        let mut shared = stack.clone();
        shared.count = count;
        stack.count -= count;
        let target = state.entities.pos(recipient);
        let dx = target[0] - pos[0];
        let dz = target[2] - pos[2];
        let length = mth::length2(dx, dz).max(1e-6);
        let id = crate::tick::spawn_item_entity(state, [pos[0], pos[1] + 1.32, pos[2]], shared);
        if let Some(item) = state.entities.slot_of(id) {
            state
                .entities
                .set_vel(item, [dx / length * 6.0, 2.0, dz / length * 6.0]);
        }
        changes.entities_spawned.push(id);
        if stack.count == 0 {
            *inventory = None;
        }
        break;
    }
}

fn gossip_limits(kind: &str) -> Option<(i32, i32, i32, i32)> {
    // maximum, daily decay, transfer decay, absolute reputation weight.
    Some(match kind {
        "trading" => (25, 2, 20, 1),
        "minor_negative" => (200, 20, 20, 1),
        "major_negative" => (100, 10, 10, 5),
        "minor_positive" => (25, 1, 5, 1),
        "major_positive" => (20, 0, 20, 5),
        _ => return None,
    })
}
pub(crate) fn record_gossip(v: &mut VillagerState, target: uuid::Uuid, kind: &str, amount: i32) {
    let Some((maximum, _, _, _)) = gossip_limits(kind) else {
        return;
    };
    if let Some(g) = v
        .gossips
        .iter_mut()
        .find(|g| g.target == target && g.kind == kind)
    {
        g.value = (g.value + amount).clamp(0, maximum);
    } else if amount >= 2
        && (v.gossips.iter().any(|g| g.target == target)
            || v.gossips
                .iter()
                .enumerate()
                .filter(|(i, g)| !v.gossips[..*i].iter().any(|other| other.target == g.target))
                .count()
                < 64)
    {
        v.gossips.push(Gossip {
            target,
            kind: kind.to_owned(),
            value: amount.min(maximum),
        });
    }
    v.gossips.retain(|g| g.value >= 2);
}
fn decay_gossip(v: &mut VillagerState, tick: u64) {
    if v.last_gossip_decay == 0 {
        v.last_gossip_decay = tick;
        return;
    }
    if tick.saturating_sub(v.last_gossip_decay) < 24_000 {
        return;
    }
    for g in &mut v.gossips {
        if let Some((_, decay, _, _)) = gossip_limits(&g.kind) {
            g.value -= decay;
        }
    }
    v.gossips.retain(|g| g.value >= 2);
    v.last_gossip_decay = tick;
}
fn transfer_gossip(receiver: &mut VillagerState, source: &VillagerState, seed: u64) {
    let total = source
        .gossips
        .iter()
        .filter_map(|g| gossip_limits(&g.kind).map(|(_, _, _, w)| g.value.max(0) as u32 * w as u32))
        .sum::<u32>();
    if total == 0 {
        return;
    }
    let mut random = (seed ^ 0x5DEECE66D) & ((1 << 48) - 1);
    let mut selected = [usize::MAX; 10];
    let mut n = 0;
    for _ in 0..10 {
        let choice = loop {
            random = random.wrapping_mul(0x5DEECE66D).wrapping_add(11) & ((1 << 48) - 1);
            let bits = (random >> 17) as u32;
            if total.is_power_of_two() {
                break ((u64::from(total) * u64::from(bits)) >> 31) as u32;
            }
            let value = bits % total;
            if bits.wrapping_sub(value).wrapping_add(total - 1) as i32 >= 0 {
                break value;
            }
        };
        let mut cumulative = 0;
        for (index, g) in source.gossips.iter().enumerate() {
            let Some((_, _, decay, weight)) = gossip_limits(&g.kind) else {
                continue;
            };
            cumulative += g.value.max(0) as u32 * weight as u32;
            if choice >= cumulative {
                continue;
            }
            if !selected[..n].contains(&index) {
                selected[n] = index;
                n += 1;
                let value = g.value - decay;
                if value >= 2 {
                    let existing = receiver
                        .gossips
                        .iter()
                        .find(|r| r.target == g.target && r.kind == g.kind)
                        .map_or(0, |r| r.value);
                    record_gossip(receiver, g.target, &g.kind, (value - existing).max(0));
                }
            }
            break;
        }
    }
}
/// Called only after an accepted player hit, before infection/removal.
pub(crate) fn harmed(
    state: &mut TickState,
    victim: EntityId,
    attacker: Option<EntityId>,
    killed: bool,
) {
    let Some(victim) = state.entities.slot_of_any(victim) else {
        return;
    };
    if state
        .entities
        .row(victim)
        .villager
        .as_ref()
        .is_none_or(|v| v.wandering || v.conversion_time.is_some())
    {
        return;
    }
    let Some(player) = attacker
        .and_then(|id| state.entities.slot_of_any(id))
        .filter(|&slot| matches!(state.entities.body(slot).kind, EntityKind::Player { .. }))
        .map(|slot| state.entities.row(slot).uuid)
    else {
        return;
    };
    let pos = state.entities.pos(victim);
    let mut witnesses = std::mem::take(&mut state.villager_pois.nearby_residents);
    witnesses.clear();
    if killed {
        witnesses.extend(bodies_near(state, pos, 16.0, true));
    } else {
        witnesses.push(victim);
    }
    for &witness in &witnesses {
        if (!killed && witness != victim)
            || (killed && !active(state, witness))
            || distance(pos, state.entities.pos(witness)) > 256.0
        {
            continue;
        }
        if let Some(v) = state.entities.row_mut(witness).villager.as_mut() {
            record_gossip(
                v,
                player,
                if killed {
                    "major_negative"
                } else {
                    "minor_negative"
                },
                25,
            );
        }
    }
    state.villager_pois.nearby_residents = witnesses;
}
fn interact_gossip(state: &mut TickState, cache: &PoiCache) {
    let count = cache.actors().len();
    let start = (state.tick.wrapping_mul(64) % count.max(1) as u64) as usize;
    for offset in 0..count.min(64) {
        let first = cache.actors()[(start + offset) % count];
        let i = first.0 as usize;
        if !active(state, first) || !cache.admitted(first) {
            continue;
        }
        let a = state.entities.row(first).villager.as_ref().unwrap();
        if a.sleeping
            || a.activity == Activity::Panic
            || state.tick.saturating_sub(a.last_gossip) < 1_200
        {
            continue;
        }
        let pos = state.entities.pos(first);
        let second = bodies_near(state, pos, 3.0, true).find(|&s| {
            s.0 > first.0
                && active(state, s)
                && distance(pos, state.entities.pos(s)) <= 5.0
                && state.entities.row(s).villager.as_ref().is_some_and(|b| {
                    !b.sleeping
                        && b.activity != Activity::Panic
                        && state.tick.saturating_sub(b.last_gossip) >= 1_200
                })
        });
        let Some(second) = second else {
            continue;
        };
        let mut a = state.entities.row_mut(first).villager.take().unwrap();
        let b = state.entities.row_mut(second).villager.as_mut().unwrap();
        transfer_gossip(&mut a, b, state.tick ^ i as u64);
        a.last_gossip = state.tick;
        b.last_gossip = state.tick;
        state.entities.row_mut(first).villager = Some(a);
    }
}

fn abort_courtship(v: &mut VillagerState) {
    v.partner = None;
    v.courtship_until = None;
    v.courtship_started_tick = None;
}
fn courtship(state: &mut TickState, cache: &mut PoiCache, changes: &mut ChangeSet) {
    // Birth searches charge the same shared pool as movement.
    let count = cache.actors().len();
    for offset in 0..count {
        let first = cache.actors()[offset];
        if !active(state, first) || !cache.admitted(first) {
            continue;
        }
        let a = state.entities.row(first).villager.as_ref().unwrap();
        let first_id = state.entities.body(first).id;
        if let Some(partner) = a.partner {
            let Some(second) = state.entities.slot_of(partner) else {
                abort_courtship(state.entities.row_mut(first).villager.as_mut().unwrap());
                continue;
            };
            if first_id.0 > partner.0 {
                continue;
            }
            let valid = active(state, second)
                && state
                    .entities
                    .row(second)
                    .villager
                    .as_ref()
                    .is_some_and(|b| {
                        b.partner == Some(first_id)
                            && b.courtship_until == a.courtship_until
                            && b.courtship_started_tick == a.courtship_started_tick
                            && willing(&cache.food, b)
                    })
                && willing(&cache.food, a)
                && a.courtship_started_tick.is_some()
                && crate::settlements::households::can_breed(state, [first_id, partner])
                && distance(state.entities.pos(first), state.entities.pos(second)) <= 8.0 * 8.0;
            let due = a.courtship_until.is_some_and(|until| state.tick >= until);
            if !valid {
                abort_courtship(state.entities.row_mut(first).villager.as_mut().unwrap());
                if let Some(b) = state.entities.row_mut(second).villager.as_mut()
                    && b.partner == Some(first_id)
                {
                    abort_courtship(b);
                }
                continue;
            }
            if !due {
                continue;
            }
            if distance(state.entities.pos(first), state.entities.pos(second)) > 5.0 {
                abort_courtship(state.entities.row_mut(first).villager.as_mut().unwrap());
                abort_courtship(state.entities.row_mut(second).villager.as_mut().unwrap());
                continue;
            }
            let mut path = std::mem::take(
                &mut state
                    .entities
                    .row_mut(first)
                    .villager
                    .as_mut()
                    .unwrap()
                    .navigation_path,
            );
            let bed = free_bed(state, cache, first, &mut path);
            if bed.is_none() && allocator::nodes_exhausted(state) {
                state
                    .entities
                    .row_mut(first)
                    .villager
                    .as_mut()
                    .unwrap()
                    .navigation_path = path;
                continue;
            }
            let birth_sequence = if bed.is_some() {
                let ordinal = state
                    .entities
                    .slots()
                    .filter(|&slot| {
                        state
                            .entities
                            .row(slot)
                            .villager
                            .as_ref()
                            .and_then(|v| v.birth_origin.as_ref())
                            .is_some_and(|origin| origin.birth_tick == state.tick)
                    })
                    .count() as u64
                    + 1;
                let Some(sequence) = state
                    .tick
                    .checked_mul(65_536)
                    .and_then(|start| start.checked_add(ordinal))
                    .filter(|_| ordinal <= 65_535)
                else {
                    let v = state.entities.row_mut(first).villager.as_mut().unwrap();
                    v.navigation_path = path;
                    abort_courtship(v);
                    abort_courtship(state.entities.row_mut(second).villager.as_mut().unwrap());
                    continue;
                };
                Some(sequence)
            } else {
                None
            };
            let mut a = state.entities.row_mut(first).villager.take().unwrap();
            a.navigation_path = path;
            let kind = a.data.kind;
            let pos = state.entities.pos(first);
            let started_tick = a.courtship_started_tick.expect("validated courtship clock");
            eat_for_birth(&cache.food, &mut a);
            abort_courtship(&mut a);
            let b = state.entities.row_mut(second).villager.as_mut().unwrap();
            eat_for_birth(&cache.food, b);
            abort_courtship(b);
            if bed.is_some() {
                a.age = 6_000;
                b.age = 6_000;
            }
            state.entities.row_mut(first).villager = Some(a);
            if let Some(home) = bed {
                let EntityKind::Mob { type_id } = state.entities.body(first).kind else {
                    continue;
                };
                let id = state.spawn_mob(type_id, pos, 20.0);
                if let Some(child) = state.entities.slot_of(id) {
                    let dimension = state.dimension.name().to_owned();
                    let mut parents = [
                        state.entities.row(first).uuid.to_string(),
                        state.entities.row(second).uuid.to_string(),
                    ];
                    parents.sort_unstable();
                    let baby = VillagerState {
                        age: -24_000,
                        data: super::VillagerData {
                            kind,
                            ..Default::default()
                        },
                        home: Some(home),
                        home_dimension: dimension,
                        birth_origin: Some(mc_script::ScriptNativeBirthOrigin::new(
                            parents,
                            started_tick,
                            state.tick,
                        )),
                        birth_sequence,
                        ..Default::default()
                    };
                    state.entities.row_mut(child).villager = Some(Box::new(baby));
                    cache.update_claims(id, [None; 3], [Some(home), None, None]);
                    cache.newborn_homes.push((id, home));
                    super::sync_appearance(state, child);
                    crate::settlements::households::inherit_birth(state, id, [first_id, partner]);
                }
                changes.entities_spawned.push(id);
            }
        } else if state.tick.is_multiple_of(20) && willing(&cache.food, a) {
            let pos = state.entities.pos(first);
            let second = bodies_near(state, pos, 8.0, true).find(|&s| {
                s.0 > first.0
                    && active(state, s)
                    && distance(pos, state.entities.pos(s)) <= 8.0 * 8.0
                    && state
                        .entities
                        .row(s)
                        .villager
                        .as_ref()
                        .is_some_and(|b| b.partner.is_none() && willing(&cache.food, b))
                    && crate::settlements::households::can_breed(
                        state,
                        [first_id, state.entities.body(s).id],
                    )
            });
            let Some(second) = second else {
                continue;
            };
            let second_id = state.entities.body(second).id;
            let until =
                state.tick + 275 + ((first_id.0 as u64 ^ second_id.0 as u64 ^ state.tick) % 50);
            let a = state.entities.row_mut(first).villager.as_mut().unwrap();
            a.partner = Some(second_id);
            a.courtship_until = Some(until);
            a.courtship_started_tick = Some(state.tick);
            let b = state.entities.row_mut(second).villager.as_mut().unwrap();
            b.partner = Some(first_id);
            b.courtship_until = Some(until);
            b.courtship_started_tick = Some(state.tick);
        }
    }
}
fn golem_eligible(v: &VillagerState, tick: u64) -> bool {
    v.age >= 0
        && v.activity == Activity::Panic
        && v.last_slept
            .is_some_and(|last| tick.saturating_sub(last) < 24_000)
        && v.golem_detected_until.is_none_or(|until| tick > until)
}
fn golems(state: &mut TickState, cache: &mut PoiCache, changes: &mut ChangeSet) {
    if !state.tick.is_multiple_of(100) || cache.actors().is_empty() {
        return;
    }
    static GOLEM: std::sync::LazyLock<mc_data::Identifier> = std::sync::LazyLock::new(|| {
        mc_data::Identifier::parse("minecraft:iron_golem").expect("static identifier")
    });
    let Some(type_id) = state.registry.entity_types.id_of(&GOLEM) else {
        return;
    };
    // Brains may move before village defense. Gather from their current
    // chunks now, not from the pre-motion threat/pickup projection.
    cache.refresh_sensor_chunks(state);
    cache.golems.clear();
    for pos in &cache.near_sensor_chunks {
        if let Some(ids) = state.entities.brain_sensor_chunks().get(pos) {
            for &id in ids {
                if let Some(slot) = state.entities.slot_of(id)
                    && state.entities.body(slot).kind == (EntityKind::Mob { type_id })
                {
                    cache.golems.push(slot);
                }
            }
        }
    }
    if state.tick.is_multiple_of(200) {
        for index in 0..cache.actors().len() {
            let slot = cache.actors()[index];
            if !active(state, slot) || !cache.admitted(slot) {
                continue;
            }
            let pos = state.entities.pos(slot);
            let nearby = cache
                .golems
                .iter()
                .any(|&s| distance(pos, state.entities.pos(s)) <= 256.0);
            if nearby {
                state
                    .entities
                    .row_mut(slot)
                    .villager
                    .as_mut()
                    .unwrap()
                    .golem_detected_until = Some(state.tick + 599);
            }
        }
    }
    let mut spawned = 0;
    let count = cache.actors().len();
    let start = ((state.tick / 100).wrapping_mul(64) % count.max(1) as u64) as usize;
    for offset in 0..count.min(64) {
        if spawned >= 4 {
            break;
        }
        let slot = cache.actors()[(start + offset) % count];
        if !active(state, slot)
            || !cache.admitted(slot)
            || !golem_eligible(
                state.entities.row(slot).villager.as_ref().unwrap(),
                state.tick,
            )
        {
            continue;
        }
        let origin = state.entities.pos(slot);
        let agrees = bodies_near(state, origin, 10.0, true)
            .filter(|&s| {
                active(state, s)
                    && golem_eligible(state.entities.row(s).villager.as_ref().unwrap(), state.tick)
                    && state
                        .entities
                        .pos(s)
                        .iter()
                        .zip(origin)
                        .all(|(a, b)| (a - b).abs() <= 10.0)
            })
            .take(3)
            .count();
        if agrees < 3
            || cache
                .golems
                .iter()
                .any(|&s| distance(origin, state.entities.pos(s)) <= 256.0)
        {
            continue;
        }
        let mut spawn = None;
        for attempt in 0..10u64 {
            let mut seed = state.tick.rotate_left(17)
                ^ (state.entities.body(slot).id.0 as u64).wrapping_mul(0x9E3779B97F4A7C15)
                ^ attempt.wrapping_mul(0xD1B54A32D192ED03);
            let mut draw = |range: i32| {
                seed ^= seed >> 30;
                seed = seed.wrapping_mul(0xBF58476D1CE4E5B9);
                seed ^= seed >> 27;
                (seed % (range * 2 + 1) as u64) as i32 - range
            };
            let offset = if attempt == 0 {
                [0, 0, 0]
            } else {
                [draw(8), draw(6), draw(8)]
            };
            let p = BlockPos {
                x: origin[0].floor() as i32 + offset[0],
                y: origin[1].floor() as i32 + offset[1],
                z: origin[2].floor() as i32 + offset[2],
            };
            let Some(pos) = standing(&state.world, &state.registry, p, 0.7, 2.7) else {
                continue;
            };
            if bodies_near(state, pos, 3.0, false).any(|s| {
                let body = state.entities.body(s);
                let at = state.entities.pos(s);
                body.alive
                    && (pos[0] - at[0]).abs() < 0.7 + body.half_width
                    && (pos[2] - at[2]).abs() < 0.7 + body.half_width
                    && pos[1] < at[1] + body.height
                    && pos[1] + 2.7 > at[1]
            }) {
                continue;
            }
            spawn = Some(pos);
            break;
        }
        if let Some(pos) = spawn {
            let id = state.spawn_mob(type_id, pos, 100.0);
            if let Some(slot) = state.entities.slot_of(id) {
                cache.golems.push(slot);
            }
            changes.entities_spawned.push(id);
            spawned += 1;
            cache.nearby_residents.clear();
            cache
                .nearby_residents
                .extend(bodies_near(state, origin, 10.0, true));
            for &s in &cache.nearby_residents {
                if active(state, s)
                    && state
                        .entities
                        .pos(s)
                        .iter()
                        .zip(origin)
                        .all(|(a, b)| (a - b).abs() <= 10.0)
                {
                    state
                        .entities
                        .row_mut(s)
                        .villager
                        .as_mut()
                        .unwrap()
                        .golem_detected_until = Some(state.tick + 599);
                }
            }
        }
    }
}

/// Called after merge effects and conversion, before publication. No player
/// item reservations survive here; all villager mutations have one owner.
pub(crate) fn tick(state: &mut TickState, changes: &mut ChangeSet) {
    #[cfg(test)]
    let brain_start = std::time::Instant::now();
    if state.entities.villager_chunks().is_empty() {
        state.tick_budget.finish(state.dimension, Consumer::Brain);
        return;
    }
    // The allocator ran immediately before this tick and owns every POI
    // memory; the brain never acquires or validates claims itself.
    let mut cache = std::mem::take(&mut state.villager_pois);
    #[cfg(test)]
    let refreshed = brain_start.elapsed().as_micros();
    cache.prepare_brains(state);
    #[cfg(test)]
    let prepared = brain_start.elapsed().as_micros();
    #[cfg(test)]
    let acquired = brain_start.elapsed().as_micros();
    #[cfg(test)]
    let mut full_ns = 0;
    for execution in 0..cache.execution_indices.len() {
        let index = cache.execution_indices[execution];
        if !cache.resident_data[index].active {
            continue;
        }
        let slot = cache.residents[index];
        let id = cache.resident_data[index].id;
        let merchant_busy = cache.merchants.binary_search(&id).is_ok();
        let scheduled = cache.activity_for(
            if state.tick < cache.resident_data[index].baby_until {
                -1
            } else {
                0
            },
            cache.resident_data[index].profession,
            state.day_time,
        );
        if state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .is_some_and(|v| !v.opened_doors.is_empty() || !v.navigation_path.is_empty())
        {
            let mut v = state
                .entities
                .row_mut_unmarked(slot)
                .villager
                .take()
                .unwrap();
            super::doors::interact(state, slot, &mut v, changes);
            state.entities.row_mut_unmarked(slot).villager = Some(v);
        }
        if !cache.admitted(slot) {
            let brain = &mut cache.resident_data[index];
            if brain.clock_changes {
                let was_baby = state
                    .entities
                    .row(slot)
                    .villager
                    .as_ref()
                    .is_some_and(|v| v.age < 0);
                if let Some(panic_until) = state.entities.advance_villager_clocks(
                    slot,
                    state.tick,
                    state.tick.saturating_add(1),
                ) {
                    brain.panic_until = panic_until;
                    brain.protected_until = brain.protected_until.max(panic_until);
                    // Only the baby -> adult crossing changes the wire projection.
                    if was_baby
                        && state
                            .entities
                            .row(slot)
                            .villager
                            .as_ref()
                            .is_some_and(|v| v.age >= 0)
                    {
                        super::sync_appearance(state, slot);
                    }
                }
            }
            if brain.motion || cache.motion_chunks.contains(&brain.chunk) {
                move_skipped(
                    state,
                    slot,
                    cache.motion_chunks.contains(&brain.chunk),
                    merchant_busy,
                    scheduled,
                );
                let row = state.entities.row(slot);
                brain.motion = row.villager.as_ref().is_some_and(|v| {
                    !v.opened_doors.is_empty()
                        || (!v.sleeping
                            && (!state.entities.ground(slot)
                                || state.entities.vel(slot) != [0.0; 3]
                                || state.entities.horizontal_impulse(slot) != [0.0; 2]
                                || !v.navigation_path.is_empty()))
                });
            }
            cache.update_execution(index);
            continue;
        }
        #[cfg(test)]
        if cache.measure_decisions {
            cache.measure_actor = Some(id);
            cache.decision_current = [0; 8];
        }
        #[cfg(test)]
        let full_start = std::time::Instant::now();
        let pos = state.entities.pos(slot);
        let mut v = state.entities.row_mut(slot).villager.take().unwrap();
        let baby = v.age < 0;
        let old_data = v.data;
        let old_activity = v.activity;
        v.age -= v.age.signum();
        decay_gossip(&mut v, state.tick);
        // POI memories are allocator-owned (validated + acquired in the
        // allocator pass immediately before this tick): the brain only reads
        // them through the local-dimension filter below.
        let home = v
            .home
            .filter(|_| v.home_dimension == state.dimension.name());
        let job_site = v
            .job_site
            .filter(|_| v.job_site_dimension == state.dimension.name());
        let meeting_point = v
            .meeting_point
            .filter(|_| v.meeting_point_dimension == state.dimension.name());
        if let Some(job) = job_site
            && v.age >= 0
            && !crate::settlements::allocator::traded(&v)
            && distance(pos, centre(job)) <= JOB_SITE_REACH_SQUARED
            && let Some(profession) = if v.settlement.is_some() {
                crate::settlements::buildings::job_profession(&state.settlements, v.settlement, job)
            } else {
                state
                    .world
                    .block_at(job.x, job.y, job.z)
                    .and_then(|id| match cache.kind(id) {
                        Some(PoiKind::Job(profession)) => Some(profession),
                        _ => None,
                    })
            }
            && v.data.profession != profession
            && crate::settlements::allocator::confirm_profession(state, &cache, id, job, profession)
        {
            v.data.profession = profession;
            v.offers.clear();
        }
        let threat = decision_phase!(cache, 2, nearest_threat(&cache, pos));
        if let Some(hurt_at) = state.entities.body(slot).hurt_at {
            v.panic_until = v.panic_until.max(hurt_at.saturating_add(100));
        }
        if threat.is_some() {
            v.panic_until = state.tick.saturating_add(100);
        }
        v.activity = if state.tick < v.panic_until {
            Activity::Panic
        } else {
            cache.activity_for(v.age, v.data.profession, state.day_time)
        };
        if old_activity != v.activity {
            v.navigation_target = None;
            v.navigation_repath_at = 0;
            v.navigation_path.clear();
            v.navigation_chunks.clear();
        }
        let was_sleeping = v.sleeping;
        if merchant_busy || v.activity != Activity::Rest || home.is_none() {
            v.sleeping = false;
        }
        if !merchant_busy
            && !v.sleeping
            && v.activity == Activity::Rest
            && let Some(home) = home
        {
            let available = decision_phase!(cache, 1, {
                !cache.occupied_beds.contains(&home)
                    && state.world.block_at(home.x, home.y, home.z).is_some()
            });
            if available && distance(pos, centre(home)) <= 4.0 {
                v.sleeping = true;
                state.entities.set_pos(
                    slot,
                    [
                        f64::from(home.x) + 0.5,
                        f64::from(home.y) + 0.6875,
                        f64::from(home.z) + 0.5,
                    ],
                );
                state.entities.set_vel(slot, [0.0; 3]);
            }
        }
        if was_sleeping && !v.sleeping {
            let (width, height) = if v.age < 0 {
                (0.15, 0.975)
            } else {
                (0.3, 1.95)
            };
            state.entities.set_half_width(slot, width);
            state.entities.body_mut(slot).height = height;
            let cell = at(pos);
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                if let Some(p) = standing(
                    &state.world,
                    &state.registry,
                    BlockPos {
                        x: cell.x + dx,
                        y: cell.y,
                        z: cell.z + dz,
                    },
                    width,
                    height,
                ) {
                    state.entities.set_pos(slot, p);
                    break;
                }
            }
        }
        if v.sleeping {
            v.last_slept = Some(state.tick);
        } else {
            let pickup_pos = state.entities.pos(slot);
            decision_phase!(cache, 3, {
                cache.items_near(pickup_pos);
                pickup(
                    state,
                    &cache.food,
                    &cache.nearby_items,
                    slot,
                    &mut v,
                    changes,
                )
            });
            decision_phase!(cache, 4, {
                super::harvest::work(state, slot, &mut v, changes);
                share_food(state, &cache.food, slot, &mut v, changes)
            });
            let worker_target = if merchant_busy {
                None
            } else {
                match v.data.profession {
                    Profession::Miner => super::mining::work(state, slot, &mut v, changes),
                    Profession::Lumberjack => super::lumber::work(state, slot, &mut v, changes),
                    _ => None,
                }
            };
            let mut short_wander = false;
            let mut exact_target = false;
            let target = if v.activity == Activity::Panic {
                if state.tick >= v.navigation_repath_at || v.navigation_target.is_none() {
                    let source = threat.or_else(|| {
                        state
                            .entities
                            .row(slot)
                            .last_player_hit
                            .and_then(|(id, _)| state.entities.slot_of(id))
                            .map(|s| state.entities.pos(s))
                    });
                    let direction = source
                        .map(|p| [pos[0] - p[0], pos[2] - p[2]])
                        .unwrap_or([1.0, 0.0]);
                    let length = mth::length2(direction[0], direction[1]).max(1e-6);
                    Some(BlockPos {
                        x: (pos[0] + direction[0] / length * 12.0).floor() as i32,
                        y: pos[1].ceil() as i32,
                        z: (pos[2] + direction[1] / length * 12.0).floor() as i32,
                    })
                } else {
                    v.navigation_target
                }
            } else if let Some(target) = crate::settlements::economy::target(
                state,
                v.settlement,
                state.entities.row(slot).uuid,
            ) {
                exact_target = true;
                Some(target)
            } else if let Some(partner) = v.partner.and_then(|id| state.entities.slot_of(id)) {
                Some(at(state.entities.pos(partner)))
            } else if worker_target.is_some()
                || (v.activity == Activity::Work
                    && matches!(
                        v.data.profession,
                        Profession::Miner | Profession::Lumberjack
                    ))
            {
                worker_target
            } else {
                match v.activity {
                    Activity::Work
                        if job_site.is_some_and(|job| {
                            distance(pos, centre(job)) > JOB_SITE_REACH_SQUARED
                        }) =>
                    {
                        job_site
                    }
                    Activity::Meet => meeting_point,
                    Activity::Rest => home,
                    Activity::Work | Activity::Idle | Activity::Play => {
                        short_wander = true;
                        if state.tick >= v.navigation_repath_at || v.navigation_target.is_none() {
                            let mut seed =
                                (state.tick / 40) ^ (id.0 as u64).wrapping_mul(0x9E3779B97F4A7C15);
                            // Vary both axes each attempt, including before the
                            // low tick counter reaches the upper 16 bits.
                            seed ^= seed >> 30;
                            seed = seed.wrapping_mul(0xBF58476D1CE4E5B9);
                            seed ^= seed >> 27;
                            // Strolls remain bounded from the current cell,
                            // including Work already within workstation reach.
                            let origin = at(pos);
                            let range = if v.activity == Activity::Work { 5 } else { 13 };
                            let dx = (seed % range) as i32 - range as i32 / 2;
                            let remaining = 6 - dx.abs();
                            let dz = (((seed >> 16) % range) as i32 - range as i32 / 2)
                                .clamp(-remaining, remaining);
                            Some(BlockPos {
                                x: origin.x + dx,
                                y: origin.y,
                                z: origin.z + dz,
                            })
                        } else {
                            v.navigation_target
                        }
                    }
                    Activity::Panic => None,
                }
            };
            if !merchant_busy
                && v.activity == Activity::Work
                && job_site.is_some_and(|p| {
                    distance(state.entities.pos(slot), centre(p)) <= JOB_SITE_REACH_SQUARED
                })
            {
                decision_phase!(
                    cache,
                    6,
                    super::trades::work(&mut v, &state.registry, state.tick, state.day_time)
                );
            }
            let scheduled = cache.activity_for(v.age, v.data.profession, state.day_time);
            decision_phase!(
                cache,
                5,
                if short_wander {
                    wander_to(
                        state,
                        slot,
                        &mut v,
                        if merchant_busy { None } else { target },
                        scheduled,
                    )
                } else {
                    move_to(
                        state,
                        &mut cache,
                        slot,
                        &mut v,
                        if merchant_busy { None } else { target },
                        exact_target,
                    )
                }
            );
        }
        super::doors::interact(state, slot, &mut v, changes);
        let appearance_changed =
            was_sleeping != v.sleeping || baby != (v.age < 0) || old_data != v.data;
        state.entities.row_mut(slot).villager = Some(v);
        if appearance_changed {
            super::sync_appearance(state, slot);
        }
        #[cfg(test)]
        {
            let elapsed = full_start.elapsed().as_nanos();
            full_ns += elapsed;
            if cache.measure_decisions {
                cache.decision_ns.push(elapsed);
                for (samples, elapsed) in
                    cache.decision_phases.iter_mut().zip(cache.decision_current)
                {
                    samples.push(elapsed);
                }
            }
        }
    }
    #[cfg(test)]
    let evaluated = brain_start.elapsed().as_micros();
    interact_gossip(state, &cache);
    courtship(state, &mut cache, changes);
    golems(state, &mut cache, changes);
    // Remove by id/descending slot only after every slot-based transaction.
    cache.removed_items.clear();
    for &slot in cache.items.values().flatten() {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if !body.alive && row.item.is_none() {
            cache.removed_items.push(slot);
        }
    }
    cache.removed_items.sort_unstable_by_key(|s| Reverse(s.0));
    for &slot in &cache.removed_items {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if matches!(body.kind, EntityKind::Item { .. }) && !body.alive && row.item.is_none() {
            let id = body.id;
            state.entities.remove(slot);
            changes.entities_removed.push(id);
        }
    }
    cache.motion_revision = Some(state.world.revision_now());
    #[cfg(test)]
    {
        cache.brain_us = brain_start.elapsed().as_micros();
        cache.phase_us = [
            refreshed,
            prepared - refreshed,
            evaluated - prepared,
            cache.brain_us - evaluated,
            acquired - prepared,
            full_ns / 1_000,
            evaluated - acquired - full_ns / 1_000,
        ];
    }
    state.villager_pois = cache;
    state
        .tick_budget
        .finish(state.dimension, crate::tick_budget::Consumer::Brain);
}

#[cfg(test)]
#[path = "brain_tests.rs"]
mod tests;
