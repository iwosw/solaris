//! Tick-owned biome spawning and category-aware despawning (ADR 0013).
//! Resident biome tables supply all weights and pack sizes. Land, cave and
//! aquatic admission use authoritative blocks, body bounds and lighting.

use mc_physics::BlockMaterial;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::blueprint::MobCategory;
use crate::boundary::{ChangeSet, EntityId};
use crate::entities::{EntityKind, EntitySlot};
use crate::tick::TickState;

#[path = "natural_spawn_hostile.rs"]
mod hostile;

/// Ticks between non-hostile spawn and grazing/daylight passes.
pub(crate) const CYCLE: u64 = 20;
/// Ticks between creature passes (vanilla spawns animals every 400 ticks).
const CREATURE_CYCLE: u64 = 400;
/// Natural categories in stable vanilla order; miscellaneous entities never spawn.
pub(crate) const CATEGORIES: [MobCategory; 7] = [
    MobCategory::Monster,
    MobCategory::Creature,
    MobCategory::Ambient,
    MobCategory::Axolotls,
    MobCategory::UndergroundWaterCreature,
    MobCategory::WaterCreature,
    MobCategory::WaterAmbient,
];
/// Spawn ring around a player, in blocks.
const SPAWN_MIN: f64 = 24.0;
const SPAWN_MAX: f64 = 128.0;
/// Vanilla waits 600 idle ticks before the 1/800 per-tick random despawn.
const DESPAWN_IDLE: u32 = 600;
const DESPAWN_ODDS: u64 = 800;
/// Item entities vanish after five minutes (vanilla `ItemEntity` lifespan).
const ITEM_LIFESPAN: u32 = 6_000;

#[derive(Debug, Default)]
struct SpawnChunk {
    counts: [usize; 7],
    entities: Vec<EntityId>,
    active: bool,
}

/// Rebuilt once after despawning, then updated by each append-only pack
/// admission. Chunk vectors retain capacity; departed chunks are discarded.
#[derive(Debug, Default)]
pub(crate) struct SpawnCensus {
    chunks: FxHashMap<(i32, i32), SpawnChunk>,
    eligible: FxHashSet<(i32, i32)>,
    global: [usize; 7],
    /// Actual player positions for the 128-block horizontal local cap.
    local_observers: Vec<[f64; 3]>,
    local_counts: Vec<[usize; 7]>,
    /// Reusable per-pass observer table (sorted by entity id) and the
    /// projected position slice the census/spawn passes consume. Owned here
    /// so the spawn pass allocates once and reuses capacity across ticks;
    /// `run` takes them out, fills them, and puts them back every pass.
    pub(crate) observer_table: Vec<(EntityId, [f64; 3])>,
    pub(crate) observer_positions: Vec<[f64; 3]>,
    max_half_width: f64,
    tick: Option<u64>,
    pub(crate) dimension_charges: crate::dimensions::spawning::Charges,
}

/// Local mob caps measure each chunk center against the player's actual X/Z.
fn local_in_range(observer: [f64; 3], cell: (i32, i32)) -> bool {
    let center = [f64::from(cell.0 * 16 + 8), f64::from(cell.1 * 16 + 8)];
    let dx = observer[0] - center[0];
    let dz = observer[2] - center[1];
    dx * dx + dz * dz <= SPAWN_MAX * SPAWN_MAX
}

impl SpawnCensus {
    fn begin(&mut self) {
        self.global.fill(0);
        self.eligible.clear();
        self.local_observers.clear();
        self.local_counts.clear();
        self.max_half_width = 0.0;
        self.dimension_charges.clear();
        for chunk in self.chunks.values_mut() {
            chunk.counts.fill(0);
            chunk.entities.clear();
            chunk.active = false;
        }
    }

    fn observe(
        &mut self,
        entity: EntityId,
        position: [f64; 3],
        half_width: f64,
        category: Option<usize>,
    ) {
        let cell = chunk_of(position);
        let chunk = self.chunks.entry(cell).or_default();
        chunk.active = true;
        chunk.entities.push(entity);
        self.max_half_width = self.max_half_width.max(half_width);
        if let Some(index) = category {
            chunk.counts[index] += 1;
            self.global[index] += 1;
            for (observer, counts) in self.local_observers.iter().zip(&mut self.local_counts) {
                if local_in_range(*observer, cell) {
                    counts[index] += 1;
                }
            }
        }
    }

    pub(crate) fn local_count(&self, observer: usize, category: usize) -> usize {
        self.local_counts[observer][category]
    }

    /// Any uncapped observer covering the chunk permits spawning.
    fn local_cap_reached(&self, position: [f64; 3], category: usize, cap: usize) -> bool {
        if self.local_observers.is_empty() {
            return false;
        }
        let cell = chunk_of(position);
        for (observer, counts) in self.local_observers.iter().zip(&self.local_counts) {
            if local_in_range(*observer, cell) && counts[category] < cap {
                return false;
            }
        }
        true
    }

    pub(crate) fn overlaps(
        &self,
        entities: &crate::entities::EntityStore,
        position: [f64; 3],
        half_width: f64,
        height: f64,
    ) -> bool {
        // Include the largest existing body, not just the candidate's chunk:
        // a dragon-sized body can cross several chunk boundaries.
        let extent = half_width + self.max_half_width;
        let min_x = ((position[0] - extent).floor() as i32).div_euclid(16);
        let max_x = ((position[0] + extent).floor() as i32).div_euclid(16);
        let min_z = ((position[2] - extent).floor() as i32).div_euclid(16);
        let max_z = ((position[2] + extent).floor() as i32).div_euclid(16);
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                let Some(chunk) = self.chunks.get(&(x, z)) else {
                    continue;
                };
                if chunk.entities.iter().any(|id| {
                    let Some(slot) = entities.slot_of(*id) else {
                        return false;
                    };
                    let row = entities.body(slot);
                    let other = entities.pos(slot);
                    (position[0] - other[0]).abs() < half_width + row.half_width
                        && (position[2] - other[2]).abs() < half_width + row.half_width
                        && position[1] < other[1] + row.height
                        && other[1] < position[1] + height
                }) {
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn nearby_entities(
        &self,
        position: [f64; 3],
        radius: f64,
    ) -> impl Iterator<Item = EntityId> + '_ {
        let min_x = ((position[0] - radius).floor() as i32).div_euclid(16);
        let max_x = ((position[0] + radius).floor() as i32).div_euclid(16);
        let min_z = ((position[2] - radius).floor() as i32).div_euclid(16);
        let max_z = ((position[2] + radius).floor() as i32).div_euclid(16);
        (min_x..=max_x).flat_map(move |x| {
            (min_z..=max_z)
                .filter_map(move |z| self.chunks.get(&(x, z)))
                .flat_map(|chunk| chunk.entities.iter().copied())
        })
    }
}

/// Vanilla monsters spawn on the surface from dusk to dawn.
#[must_use]
pub fn is_night(day_time: i64) -> bool {
    (13_000..23_000).contains(&day_time.rem_euclid(24_000))
}

/// Rows below which the age/despawn row scans stay inline: a pool hand-off
/// costs more than a small population's pass.
const MIN_PARALLEL_ROWS: usize = crate::contact_hazards::MIN_PARALLEL_ROWS;

/// Advance lifetime clocks once for every live entity before this tick's
/// admissions. Age is anchored by record ticks, never journaled on its own.
/// The per-row clock step is order-free (rows are independent); expired items
/// leave in reverse slot order, which keeps compaction from invalidating the
/// pending removals.
/// Returns the global-player admission list when expiry removed nothing, so the
/// despawn pass of the same tick can reuse it instead of re-admitting every row.
fn age_pass(
    state: &mut TickState,
    changes: &mut ChangeSet,
    min_parallel: usize,
) -> Option<Vec<EntitySlot>> {
    let mut slots = crate::tick::active_entity_slots(state, true);
    let mut admitted = std::mem::take(&mut state.serial_entity_marks);
    admitted.clear();
    admitted.resize(state.entities.len(), 0);
    for slot in &slots {
        admitted[slot.0 as usize] = 1;
    }
    let expired = state
        .entities
        .advance_admitted_ages(&admitted, ITEM_LIFESPAN, min_parallel);
    admitted.clear();
    state.serial_entity_marks = admitted;
    if expired.is_empty() {
        return Some(slots);
    }
    for &slot in expired.iter().rev() {
        changes.entities_removed.push(state.entities.body(slot).id);
        state.remove_entity(slot);
    }
    slots.clear();
    state.serial_entity_slots = slots;
    None
}

/// The pre-parallel age pass (serial, reverse slot order, one row at a time):
/// the reference half of the fused tick entry.
#[cfg(test)]
pub(crate) fn advance_ages_reference(state: &mut TickState, changes: &mut ChangeSet) {
    let mut slots = crate::tick::active_entity_slots(state, true);
    for &slot in slots.iter().rev() {
        let Some(age) = state.entities.advance_age(slot) else {
            continue;
        };
        let row = state.entities.body(slot);
        if matches!(row.kind, EntityKind::Item { .. }) && age >= ITEM_LIFESPAN {
            changes.entities_removed.push(row.id);
            state.remove_entity(slot);
        }
    }
    slots.clear();
    state.serial_entity_slots = slots;
}

/// Standalone age pass over the production code.
#[cfg(test)]
pub(crate) fn advance_ages(state: &mut TickState, changes: &mut ChangeSet) {
    if let Some(mut slots) = age_pass(state, changes, MIN_PARALLEL_ROWS) {
        slots.clear();
        state.serial_entity_slots = slots;
    }
}

/// Tick entry: the age pass, then the spawn/despawn pass over the same
/// admission list unless item expiry compacted the store in between.
pub(crate) fn advance_ages_and_run(state: &mut TickState, changes: &mut ChangeSet) {
    advance_ages_and_run_with(state, changes, MIN_PARALLEL_ROWS);
}

fn advance_ages_and_run_with(state: &mut TickState, changes: &mut ChangeSet, min_parallel: usize) {
    let admitted = age_pass(state, changes, min_parallel);
    run_with(state, changes, admitted, min_parallel);
}

/// Test entry with an explicit pool threshold (0 forces the pool path).
#[cfg(test)]
pub(crate) fn advance_ages_and_run_for_test(
    state: &mut TickState,
    changes: &mut ChangeSet,
    min_parallel: usize,
) {
    advance_ages_and_run_with(state, changes, min_parallel);
}

/// One spawn/despawn pass; runs before inputs so the slots inputs stage
/// for this tick are never moved by a removal here.
#[cfg(test)]
pub(crate) fn run(state: &mut TickState, changes: &mut ChangeSet) {
    run_with(state, changes, None, MIN_PARALLEL_ROWS);
}

/// `admitted` is a still-valid global-player admission list from [`age_pass`];
/// the extra player slots fail despawn's mob gate, so it equals the
/// despawn admission for every non-player row.
fn run_with(
    state: &mut TickState,
    changes: &mut ChangeSet,
    admitted: Option<Vec<EntitySlot>>,
    min_parallel: usize,
) {
    // Reuse the census-owned observer scratch across ticks: the only
    // per-observer borrow is the entity position read, so the table keeps
    // sorted-by-id order for the RNG/local-cap accounting below.
    let mut observers = std::mem::take(&mut state.spawn_census.observer_table);
    let mut positions = std::mem::take(&mut state.spawn_census.observer_positions);
    observers.clear();
    positions.clear();
    observers.extend(
        state
            .players
            .iter()
            .filter(|(_, player)| !player.spectator())
            .filter_map(|(id, _)| {
                state
                    .entities
                    .slot_of(*id)
                    .map(|slot| (*id, state.entities.pos(slot)))
            }),
    );
    let mut rng = Rng((state.world.seed() as u64) ^ state.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    // No observers pauses distance-based despawning, not Peaceful or item expiry.
    if observers.is_empty() {
        state.spawn_census.observer_table = observers;
        state.spawn_census.observer_positions = positions;
        despawn(
            state,
            &crate::target_grid::TargetGrid::linear(&[]),
            &mut rng,
            changes,
            admitted,
            min_parallel,
        );
        return;
    }
    observers.sort_unstable_by_key(|(id, _)| id.0);
    let grid = crate::target_grid::TargetGrid::for_consumers(&observers, state.entities.len());
    despawn(state, &grid, &mut rng, changes, admitted, min_parallel);

    // The disabled-spawn path only needs burn/graze: keep the position
    // projection (and its census/spawn consumers) out of it entirely.
    let cycle = state.tick.is_multiple_of(CYCLE);
    if !crate::command_rules::boolean(state, crate::command_rules::BoolRule::SPAWN_MOBS) {
        if cycle {
            burn(state, is_night(state.day_time), changes);
            graze(state, &mut rng, changes);
        }
        state.spawn_census.observer_table = observers;
        state.spawn_census.observer_positions = positions;
        return;
    }
    // Project positions once per pass from the same sorted table instead of
    // re-collecting a second allocation.
    positions.extend(observers.iter().map(|(_, pos)| *pos));

    let night = is_night(state.day_time);
    let spawn_monsters =
        crate::command_rules::boolean(state, crate::command_rules::BoolRule::SPAWN_MONSTERS);
    census(state, &positions);
    if spawn_monsters && state.difficulty != crate::survival::Difficulty::Peaceful {
        hostile::run(state, &positions, &mut rng, changes);
    }
    if !cycle {
        state.spawn_census.observer_table = observers;
        state.spawn_census.observer_positions = positions;
        return;
    }
    let eligible = state.spawn_census.eligible.len();
    for (observer, around) in positions.iter().enumerate() {
        for (index, category) in CATEGORIES.into_iter().enumerate() {
            if category == MobCategory::Monster
                || (category == MobCategory::Creature && !state.tick.is_multiple_of(CREATURE_CYCLE))
            {
                continue;
            }
            let cap = category.max_instances_per_chunk() as usize;
            let global_cap = cap * eligible / 289;
            let local = state.spawn_census.local_count(observer, index);
            let room = global_cap
                .saturating_sub(state.spawn_census.global[index])
                .min(cap.saturating_sub(local));
            if room != 0 {
                spawn_group(
                    state, &positions, *around, category, room, &mut rng, changes,
                );
            }
        }
    }

    burn(state, night, changes);
    graze(state, &mut rng, changes);
    state.spawn_census.observer_table = observers;
    state.spawn_census.observer_positions = positions;
}

pub(crate) fn category_of(state: &TickState, slot: EntitySlot) -> MobCategory {
    match state.entities.body(slot).kind {
        EntityKind::Mob { type_id } => state
            .registry
            .blueprint(type_id)
            .map_or(MobCategory::Misc, |blueprint| blueprint.category),
        _ => MobCategory::Misc,
    }
}

/// Vanilla `isSunBurnTick` mobs: the blueprint says whether it ignites by day.
fn burns_in_daylight(state: &TickState, slot: EntitySlot) -> bool {
    match state.entities.body(slot).kind {
        EntityKind::Mob { type_id } => state
            .registry
            .blueprint(type_id)
            .is_some_and(|blueprint| blueprint.capabilities.burns_in_daylight),
        _ => false,
    }
}

pub(crate) fn persistent(row: &crate::entities::ColdRow) -> bool {
    row.persistent
        || row.appearance.custom_name.is_some()
        || row.entity_life.tamed
        || row.entity_life.mounted
}

fn category_index(category: MobCategory) -> Option<usize> {
    match category {
        MobCategory::Monster => Some(0),
        MobCategory::Creature => Some(1),
        MobCategory::Ambient => Some(2),
        MobCategory::Axolotls => Some(3),
        MobCategory::UndergroundWaterCreature => Some(4),
        MobCategory::WaterCreature => Some(5),
        MobCategory::WaterAmbient => Some(6),
        MobCategory::Misc => None,
    }
}

fn chunk_of(position: [f64; 3]) -> (i32, i32) {
    (
        (position[0].floor() as i32).div_euclid(16),
        (position[2].floor() as i32).div_euclid(16),
    )
}

fn near_chunks(first: (i32, i32), second: (i32, i32)) -> bool {
    (i64::from(first.0) - i64::from(second.0)).abs() <= 8
        && (i64::from(first.1) - i64::from(second.1)).abs() <= 8
}

fn observe_slot(state: &mut TickState, slot: EntitySlot) {
    let row = state.entities.body(slot);
    if !row.alive {
        return;
    }
    let entity = row.id;
    let position = state.entities.pos(slot);
    let width = row.half_width;
    // Native rows outlive residency, unlike vanilla's loaded-entity census.
    // Keep their bodies indexed for overlap checks, but not in loaded mob caps.
    let cell = chunk_of(position);
    let category = (!persistent(state.entities.row(slot)) && state.world.has_chunk(cell.0, cell.1))
        .then(|| category_of(state, slot))
        .and_then(category_index);
    state
        .spawn_census
        .observe(entity, position, width, category);
    if category.is_some() {
        crate::dimensions::spawning::observe(state, slot);
    }
}

/// Build the one population census and all observer-local category counters.
/// Direct pack callers must rebuild after changes outside their spawn pass.
pub(crate) fn census(state: &mut TickState, players: &[[f64; 3]]) -> [usize; 7] {
    state.spawn_census.begin();
    // Build chunk totals first; do not multiply the population scan by players.
    for index in 0..state.entities.len() {
        observe_slot(state, EntitySlot(index as u32));
    }
    state.spawn_census.chunks.retain(|_, chunk| chunk.active);
    for player in players {
        let cell = chunk_of(*player);
        let mut counts = [0; 7];
        for x in cell.0 - 8..=cell.0 + 8 {
            for z in cell.1 - 8..=cell.1 + 8 {
                if state.world.is_active(x, z) {
                    state.spawn_census.eligible.insert((x, z));
                }
                if local_in_range(*player, (x, z))
                    && let Some(chunk) = state.spawn_census.chunks.get(&(x, z))
                {
                    for (count, population) in counts.iter_mut().zip(&chunk.counts) {
                        *count += *population;
                    }
                }
            }
        }
        state.spawn_census.local_observers.push(*player);
        state.spawn_census.local_counts.push(counts);
    }
    state.spawn_census.tick = Some(state.tick);
    state.spawn_census.global
}

fn nearest_sq(players: &[[f64; 3]], pos: [f64; 3]) -> f64 {
    players
        .iter()
        .map(|player| {
            let d = [player[0] - pos[0], player[1] - pos[1], player[2] - pos[2]];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
        })
        .fold(f64::INFINITY, f64::min)
}

fn despawn(
    state: &mut TickState,
    players: &crate::target_grid::TargetGrid<'_>,
    rng: &mut Rng,
    changes: &mut ChangeSet,
    admitted: Option<Vec<EntitySlot>>,
    min_parallel: usize,
) {
    let mut gone: Vec<EntitySlot> = Vec::new();
    let mut slots = admitted.unwrap_or_else(|| crate::tick::active_entity_slots(state, false));
    let peaceful_difficulty = state.difficulty == crate::survival::Difficulty::Peaceful;
    let no_players = players.table().is_empty();
    // Read-only filter first: almost every resident body has no despawn work,
    // so the pool drops them in parallel and the serial pass below (RNG draws,
    // clock writes, removals in slot order) sees only rows that may act.
    let acts = |slot: &EntitySlot| {
        let row = state.entities.row(*slot);
        match state.entities.body(*slot).kind {
            EntityKind::Item { .. } => row.age >= ITEM_LIFESPAN,
            EntityKind::Mob { type_id } => {
                // Vanilla EnderDragon.checkDespawn is a no-op, even on Peaceful.
                !crate::end_fight::dragon_type(type_id)
                    && state.entities.body(*slot).alive
                    && !no_work_before_category(row, peaceful_difficulty, no_players)
            }
            _ => false,
        }
    };
    let candidates: Vec<EntitySlot> = if slots.len() < min_parallel {
        slots.iter().copied().filter(|slot| acts(slot)).collect()
    } else {
        use rayon::prelude::*;
        slots
            .par_iter()
            .copied()
            .with_min_len(2048)
            .filter(|slot| acts(slot))
            .collect()
    };
    for &slot in &candidates {
        let row = state.entities.row(slot);
        if matches!(state.entities.body(slot).kind, EntityKind::Item { .. }) {
            if row.age >= ITEM_LIFESPAN {
                gone.push(slot);
            }
            continue;
        }
        if !state.entities.body(slot).alive
            || !matches!(state.entities.body(slot).kind, EntityKind::Mob { .. })
        {
            continue;
        }
        // Most resident bodies have no despawn work at all: outside Peaceful
        // (which removes monsters regardless of players) nobody to measure
        // against, or an explicitly retained body with a clear idle clock, is
        // done before the blueprint category, activity or distances are read.
        if no_work_before_category(row, peaceful_difficulty, no_players) {
            continue;
        }
        let category = category_of(state, slot);
        let peaceful = category == MobCategory::Monster && peaceful_difficulty;
        let retained = explicitly_retained(row) || category.is_persistent();
        if !peaceful && (no_players || retained && row.no_action_time == 0) {
            continue;
        }
        if peaceful {
            gone.push(slot);
            continue;
        }
        if players.table().is_empty() {
            continue;
        }
        if retained {
            if row.no_action_time != 0 {
                state.entities.set_no_action_time(slot, 0);
            }
            continue;
        }
        let pos = state.entities.pos(slot);
        let always = f64::from(category.despawn_distance());
        let near = f64::from(category.no_despawn_distance());
        let within = |radius: f64| {
            players.any(pos, radius, |(_, target)| {
                let distance_sq = (target[0] - pos[0]).powi(2)
                    + (target[1] - pos[1]).powi(2)
                    + (target[2] - pos[2]).powi(2);
                distance_sq <= radius * radius
            })
        };
        let nearby = within(near);
        let idle = if nearby {
            0
        } else {
            row.no_action_time.saturating_add(1)
        };
        if row.no_action_time != idle {
            state.entities.set_no_action_time(slot, idle);
        }
        if (!nearby && !within(always))
            || (idle > DESPAWN_IDLE && !nearby && rng.below(DESPAWN_ODDS) == 0)
        {
            gone.push(slot);
        }
    }
    // Removal swaps the last row into the hole: highest slots first.
    gone.sort_unstable_by_key(|slot| std::cmp::Reverse(slot.0));
    for slot in gone {
        changes.entities_removed.push(state.entities.body(slot).id);
        state.remove_entity(slot);
    }
    slots.clear();
    state.serial_entity_slots = slots;
}

fn category_key(category: MobCategory) -> &'static str {
    match category {
        MobCategory::Monster => "monster",
        MobCategory::Creature => "creature",
        MobCategory::Ambient => "ambient",
        MobCategory::Axolotls => "axolotls",
        MobCategory::UndergroundWaterCreature => "underground_water_creature",
        MobCategory::WaterCreature => "water_creature",
        MobCategory::WaterAmbient => "water_ambient",
        MobCategory::Misc => "misc",
    }
}

fn supported_entries<'a>(
    registry: &'a crate::registry::CoreRegistry,
    entries: &'a [mc_data::biomes::BiomeSpawnEntry],
) -> impl Iterator<Item = &'a mc_data::biomes::BiomeSpawnEntry> {
    entries.iter().filter(move |entry| {
        registry
            .entity_types
            .id_of(&entry.entity_type)
            .and_then(|id| registry.blueprint(id))
            .is_some_and(|blueprint| blueprint.category != MobCategory::Misc)
    })
}

/// Weighted supported entry in biome order; absent/zero-weight tables fail closed.
pub(crate) fn choose_spawn<'a>(
    registry: &'a crate::registry::CoreRegistry,
    entries: &'a [mc_data::biomes::BiomeSpawnEntry],
    mut roll: u64,
) -> Option<&'a mc_data::biomes::BiomeSpawnEntry> {
    for entry in supported_entries(registry, entries) {
        let weight = u64::from(entry.weight);
        if roll < weight {
            return Some(entry);
        }
        roll -= weight;
    }
    None
}

fn entries_at(
    state: &TickState,
    x: i32,
    y: i32,
    z: i32,
    category: MobCategory,
) -> &[mc_data::biomes::BiomeSpawnEntry] {
    state.world.biome_at(x, y, z).map_or(&[], |biome| {
        state
            .registry
            .biome_spawns
            .exact_entries(biome, category_key(category))
    })
}

fn spawn_column(around: [f64; 3], category: MobCategory, rng: &mut Rng) -> [i32; 2] {
    let angle = rng.unit() * std::f64::consts::TAU;
    let distance = if category == MobCategory::Monster {
        (SPAWN_MIN * SPAWN_MIN + rng.unit() * (SPAWN_MAX * SPAWN_MAX - SPAWN_MIN * SPAWN_MIN))
            .sqrt()
    } else {
        SPAWN_MIN + rng.unit() * (64.0 - SPAWN_MIN)
    };
    [
        mc_physics::mth::floor_i32(around[0] + f64::from(mc_physics::mth::cos(angle)) * distance),
        mc_physics::mth::floor_i32(around[2] + f64::from(mc_physics::mth::sin(angle)) * distance),
    ]
}

fn aquatic(category: MobCategory) -> bool {
    matches!(
        category,
        MobCategory::Axolotls
            | MobCategory::UndergroundWaterCreature
            | MobCategory::WaterCreature
            | MobCategory::WaterAmbient
    )
}

/// Uniform hostile Y attempts; other habitats retain their cave/water sampler.
fn spawn_height(
    state: &TickState,
    x: i32,
    z: i32,
    category: MobCategory,
    rng: &mut Rng,
) -> Option<i32> {
    if category == MobCategory::Monster {
        return hostile::height(state, x, z, rng);
    }
    if category == MobCategory::Misc
        || (category == MobCategory::Creature && state.dimension.beds_work())
    {
        return spawn_floor(state, x, z, category);
    }
    // Resolve membership once for the full-height reservoir scan, not once per
    // candidate floor. This borrow ends with the read-only scan; later passes
    // still observe direct mutations of TagsData::registries.
    let leaves = leaves_for_support(state, category);
    let mut picked = None;
    let mut seen = 0;
    let geometry = state.dimension.geometry();
    let ceiling = if state.dimension == crate::dimensions::Dimension::Nether {
        128
    } else {
        geometry.max_y()
    };
    for y in (geometry.min_y() + 1)..(ceiling - 2) {
        let feet = state
            .registry
            .materials
            .classify(state.world.block_at(x, y, z)?);
        let above = state
            .registry
            .materials
            .classify(state.world.block_at(x, y + 1, z)?);
        let water = feet == BlockMaterial::Water && above == BlockMaterial::Water;
        let land = feet == BlockMaterial::Air
            && above == BlockMaterial::Air
            && valid_support(state, x, y - 1, z, category, leaves);
        let valid = if aquatic(category) {
            water
                && (!matches!(
                    category,
                    MobCategory::Axolotls | MobCategory::UndergroundWaterCreature
                ) || y < 63)
        } else if category == MobCategory::Ambient {
            land && y < 63
        } else {
            land || water
        };
        if valid {
            seen += 1;
            if rng.below(seen) == 0 {
                picked = Some(y);
            }
        }
    }
    picked
}

fn leaves_for_support(state: &TickState, category: MobCategory) -> &[i32] {
    if category == MobCategory::Creature {
        // Creature support uses grass/fallback surfaces (or Nether lava), not tags.
        &[]
    } else {
        state
            .registry
            .tags
            .raw_ids("minecraft:block", "minecraft:leaves")
            .unwrap_or(&[])
    }
}

fn valid_support(
    state: &TickState,
    x: i32,
    y: i32,
    z: i32,
    category: MobCategory,
    leaves: &[i32],
) -> bool {
    let Some(floor) = state.world.block_at(x, y, z) else {
        return false;
    };
    if category == MobCategory::Creature && state.dimension == crate::dimensions::Dimension::Nether
    {
        return state.registry.materials.classify(floor) == BlockMaterial::Lava;
    }
    if state.registry.materials.classify(floor) != BlockMaterial::Solid {
        return false;
    }
    if category == MobCategory::Creature {
        return floor == state.registry.states.grass_block
            || state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(floor))
                .is_some_and(|block| {
                    mc_data::block_semantics_26_1_2::passive_herd_fallback_surface_name(
                        block.block.id.as_str(),
                    )
                });
    }
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(floor))
        .is_some_and(|block| {
            let name = block.block.id.path();
            !leaves.contains(&block.block.raw_id)
                && !matches!(name, "glass" | "tinted_glass")
                && !name.ends_with("_stained_glass")
                && mc_data::block_facts::has_full_sturdy_face(
                    floor,
                    &block.block.id,
                    &block.properties,
                    mc_data::block_facts::SturdyFace::Up,
                )
        })
}

fn admitted(
    state: &TickState,
    blueprint: &crate::blueprint::MobBlueprint,
    pos: [f64; 3],
    rng: &mut Rng,
) -> bool {
    let x = pos[0].floor() as i32;
    let y = pos[1].floor() as i32;
    let z = pos[2].floor() as i32;
    let water = state
        .world
        .block_at(x, y, z)
        .is_some_and(|block| state.registry.materials.classify(block) == BlockMaterial::Water);
    let swimmer = matches!(blueprint.movement, crate::blueprint::MovementClass::Swimmer);
    let amphibious = blueprint.movement == crate::blueprint::MovementClass::Amphibious;
    let invalid_habitat = if water {
        !(swimmer || amphibious)
    } else {
        swimmer
            || !valid_support(
                state,
                x,
                y - 1,
                z,
                blueprint.category,
                leaves_for_support(state, blueprint.category),
            )
    };
    if invalid_habitat {
        return false;
    }
    let required = if water {
        BlockMaterial::Water
    } else {
        BlockMaterial::Air
    };
    let min_x = (pos[0] - blueprint.half_width + f64::EPSILON).floor() as i32;
    let max_x = (pos[0] + blueprint.half_width - f64::EPSILON).floor() as i32;
    let max_y = (pos[1] + blueprint.height - f64::EPSILON).floor() as i32;
    let min_z = (pos[2] - blueprint.half_width + f64::EPSILON).floor() as i32;
    let max_z = (pos[2] + blueprint.half_width - f64::EPSILON).floor() as i32;
    for bx in min_x..=max_x {
        for bz in min_z..=max_z {
            for by in y..=max_y {
                if !state
                    .world
                    .block_at(bx, by, bz)
                    .is_some_and(|block| state.registry.materials.classify(block) == required)
                {
                    return false;
                }
            }
        }
    }
    let Some((sky, block)) = state.world.light_at(x, y, z) else {
        return false;
    };
    let sky = if state.dimension.has_weather() {
        sky
    } else {
        0
    };
    let brightness = if state.thunder_level * state.rain_level > 0.9 {
        block.max(sky.saturating_sub(10))
    } else {
        state.brightness(x, y, z)
    };
    match blueprint.category {
        MobCategory::Monster => {
            if state.difficulty == crate::survival::Difficulty::Peaceful
                || !crate::command_rules::boolean(
                    state,
                    crate::command_rules::BoolRule::SPAWN_MONSTERS,
                )
                || match state.dimension {
                    crate::dimensions::Dimension::Overworld
                        if blueprint.capabilities.slime_spawn_rules =>
                    {
                        !hostile::slime_allowed(state, [x, y, z], brightness, rng)
                    }
                    crate::dimensions::Dimension::Overworld => {
                        block != 0
                            || u64::from(sky) > rng.below(32)
                            || u64::from(brightness) > rng.below(8)
                    }
                    crate::dimensions::Dimension::Nether => {
                        !blueprint.capabilities.ignores_nether_spawn_light && block > 11
                    }
                    crate::dimensions::Dimension::End => block > 7,
                }
            {
                return false;
            }
        }
        MobCategory::Creature if state.dimension.has_weather() && state.brightness(x, y, z) < 9 => {
            return false;
        }
        MobCategory::Ambient if state.brightness(x, y, z) > 7 => return false,
        _ => {}
    }
    !state
        .spawn_census
        .overlaps(&state.entities, pos, blueprint.half_width, blueprint.height)
}

/// Spawn one biome-selected pack against this tick's census and local counters.
pub(crate) fn spawn_group(
    state: &mut TickState,
    players: &[[f64; 3]],
    around: [f64; 3],
    category: MobCategory,
    room: usize,
    rng: &mut Rng,
    changes: &mut ChangeSet,
) -> usize {
    let column = spawn_column(around, category, rng);
    spawn_group_in_column(state, players, column, category, room, rng, changes)
}

fn spawn_group_in_column(
    state: &mut TickState,
    players: &[[f64; 3]],
    column: [i32; 2],
    category: MobCategory,
    room: usize,
    rng: &mut Rng,
    changes: &mut ChangeSet,
) -> usize {
    if room == 0 || category == MobCategory::Misc {
        return 0;
    }
    debug_assert_eq!(state.spawn_census.tick, Some(state.tick));
    debug_assert_eq!(state.spawn_census.local_counts.len(), players.len());
    let Some(height) = spawn_height(state, column[0], column[1], category, rng) else {
        return 0;
    };
    let entries = entries_at(state, column[0], height, column[1], category);
    let total: u64 = supported_entries(&state.registry, entries)
        .map(|entry| u64::from(entry.weight))
        .sum();
    if total == 0 {
        return 0;
    }
    let Some(entry) = choose_spawn(&state.registry, entries, rng.below(total)) else {
        return 0;
    };
    let Some(type_id) = state.registry.entity_types.id_of(&entry.entity_type) else {
        return 0;
    };
    let registry = std::sync::Arc::clone(&state.registry);
    let Some(blueprint) = registry.blueprint(type_id) else {
        return 0;
    };
    let health = blueprint.params.max_health;
    let name = blueprint.name.as_str();
    let min = entry.min_count.min(entry.max_count).max(1);
    let max = entry.max_count.max(min);
    let size = u64::from(min) + rng.below(u64::from(max - min) + 1);
    let mut spawned = 0;
    for _ in 0..size {
        if spawned == room {
            break;
        }
        let x = column[0] + rng.below(7) as i32 - 3;
        let z = column[1] + rng.below(7) as i32 - 3;
        let y = if category == MobCategory::Creature && state.dimension.beds_work() {
            let Some(y) = spawn_floor(state, x, z, category) else {
                continue;
            };
            y
        } else {
            height
        };
        let pos = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
        let distance = nearest_sq(players, pos);
        if distance <= SPAWN_MIN * SPAWN_MIN
            || (!players.is_empty() && distance > f64::from(category.despawn_distance()).powi(2))
            || !entries_at(state, x, y, z, category)
                .iter()
                .any(|entry| entry.entity_type.as_str() == name)
        {
            continue;
        }
        let cap = category.max_instances_per_chunk() as usize;
        if state.spawn_census.local_cap_reached(
            pos,
            category_index(category).expect("natural category"),
            cap,
        ) {
            continue;
        }
        if !admitted(
            state,
            state
                .registry
                .blueprint(type_id)
                .expect("selected blueprint"),
            pos,
            rng,
        ) {
            continue;
        }
        if !crate::dimensions::spawning::admitted(state, name, pos) {
            continue;
        }
        let id = state.spawn_mob(type_id, pos, health);
        let slot = state.entities.slot_of(id).expect("new spawn");
        crate::mob_equipment::natural(state, slot);
        observe_slot(state, slot);
        changes.entities_spawned.push(id);
        spawned += 1;
    }
    spawned
}

/// Feet height of the column's surface spawn spot: the topmost non-air
/// block is a valid floor (grass for creatures, any solid block but leaves
/// for monsters, any solid block for players) with two air blocks above.
/// `None` off resident chunks.
pub(crate) fn spawn_floor(state: &TickState, x: i32, z: i32, category: MobCategory) -> Option<i32> {
    let materials = &state.registry.materials;
    let geometry = state.dimension.geometry();
    let mut y = geometry.max_y() - 1;
    let floor = loop {
        let block = state.world.block_at(x, y, z)?;
        if materials.classify(block) != BlockMaterial::Air {
            break block;
        }
        y -= 1;
        if y < geometry.min_y() {
            return None;
        }
    };
    let solid = materials.classify(floor) == BlockMaterial::Solid;
    let valid = match category {
        MobCategory::Creature => valid_support(state, x, y, z, category, &[]),
        MobCategory::Misc => solid,
        MobCategory::Monster | MobCategory::Ambient => valid_support(
            state,
            x,
            y,
            z,
            category,
            leaves_for_support(state, category),
        ),
        MobCategory::Axolotls
        | MobCategory::UndergroundWaterCreature
        | MobCategory::WaterCreature
        | MobCategory::WaterAmbient => false,
    };
    // The topmost non-air block has only air above it inside the world.
    (valid && y + 2 < geometry.max_y()).then_some(y + 1)
}

/// Choose a dry resident spawn column within the configured radius. Small
/// squares use a coprime walk; large rules examine resident columns only, never
/// billions of unloaded positions. This runs on join/respawn, not every tick.
pub(crate) fn player_spawn(state: &TickState, salt: u64) -> [f64; 3] {
    let spawn = state.world.world_spawn();
    let radius = i64::from(
        crate::command_rules::integer(state, crate::command_rules::IntRule::RESPAWN_RADIUS).max(0),
    );
    let seed = (state.world.seed() as u64) ^ salt.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    if radius <= 31 {
        let side = (2 * radius + 1) as u64;
        let area = side * side;
        let offset = Rng(seed).below(area);
        // Preserve the ordinary seed walk where 17 is coprime. For a multiple
        // of 17, side+1 is always coprime to side².
        let step = if side.is_multiple_of(17) {
            side + 1
        } else {
            17
        };
        for i in 0..area {
            let index = (offset + step * i) % area;
            let x = i64::from(spawn[0]) + (index % side) as i64 - radius;
            let z = i64::from(spawn[2]) + (index / side) as i64 - radius;
            if let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z))
                && let Some(y) = spawn_floor(state, x, z, MobCategory::Misc)
            {
                return [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
            }
        }
    } else {
        let mut best: Option<((u64, i32, i32), i32)> = None;
        for chunk in state.world.chunks() {
            let x0 = i64::from(chunk.pos.x) * 16;
            let z0 = i64::from(chunk.pos.z) * 16;
            let min_x = x0
                .max(i64::from(spawn[0]) - radius)
                .max(i64::from(i32::MIN));
            let max_x = (x0 + 15)
                .min(i64::from(spawn[0]) + radius)
                .min(i64::from(i32::MAX));
            let min_z = z0
                .max(i64::from(spawn[2]) - radius)
                .max(i64::from(i32::MIN));
            let max_z = (z0 + 15)
                .min(i64::from(spawn[2]) + radius)
                .min(i64::from(i32::MAX));
            for x in min_x..=max_x {
                for z in min_z..=max_z {
                    let (x, z) = (x as i32, z as i32);
                    let key = (
                        Rng(seed ^ u64::from(x as u32) ^ (u64::from(z as u32) << 32)).next(),
                        x,
                        z,
                    );
                    if best.is_none_or(|(old, _)| key < old)
                        && let Some(y) = spawn_floor(state, x, z, MobCategory::Misc)
                    {
                        best = Some((key, y));
                    }
                }
            }
        }
        if let Some(((_, x, z), y)) = best {
            return [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
        }
    }
    state.world.world_spawn_position()
}

/// Refresh daylight ignition; the shared fire owner deals damage and extinguishes.
fn burn(state: &mut TickState, night: bool, changes: &mut ChangeSet) {
    if night || state.rain_level > 0.0 {
        return;
    }
    for index in 0..state.entities.len() as u32 {
        let slot = EntitySlot(index);
        // Daylight burn is live simulation: freeze resident-inactive.
        let bpos = state.entities.pos(slot);
        if !state.world.is_active(
            (bpos[0].floor() as i32).div_euclid(16),
            (bpos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let row = state.entities.row(slot);
        if state.entities.body(slot).alive
            && burns_in_daylight(state, slot)
            && open_sky(
                state,
                state.entities.pos(slot),
                state.entities.body(slot).height,
            )
        {
            let id = state.entities.body(slot).id;
            // Mob.burnUndead protects with any nonempty head item, including
            // nondamageable pumpkins. A damageable head item takes nextInt(2)
            // wear; even its breaking pass is protected (javap-daylight.log).
            if let Some(head) = row.appearance.equipment[5]
                .as_ref()
                .filter(|stack| !stack.is_empty())
            {
                let maximum = head.extended().max_damage.or_else(|| {
                    state
                        .registry
                        .item_facts
                        .facts_for_stack(head, &state.registry.items)
                        .and_then(|facts| facts.max_damage)
                });
                let wear = Rng((state.world.seed() as u64)
                    ^ state.tick.wrapping_mul(0xBF58_476D_1CE4_E5B9)
                    ^ id.0 as u64)
                .below(2) as i32;
                if let Some(maximum) = maximum.filter(|maximum| *maximum != 0)
                    && wear != 0
                {
                    let head =
                        &mut std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance)
                            .equipment[5];
                    let stack = head.as_mut().expect("protected by nonempty head item");
                    let damage = stack.damage.unwrap_or(0).saturating_add(wear);
                    if u32::try_from(damage).is_ok_and(|damage| damage >= maximum) {
                        *head = None;
                    } else {
                        *stack = std::mem::take(stack).with_damage(damage);
                    }
                    changes.entities_appearance_changed.push(id);
                }
                continue;
            }
            crate::contact_hazards::ignite(state, id, 160);
        }
    }
}

/// Sheared sheep graze (vanilla `EatBlockGoal`, about once per 1000 ticks):
/// the grass block under one turns to dirt and its wool grows back.
fn graze(state: &mut TickState, rng: &mut Rng, changes: &mut ChangeSet) {
    let mob_griefing =
        crate::command_rules::mob_griefing(state, crate::command_rules::MobGrief::SheepGrazing);
    for index in 0..state.entities.len() as u32 {
        let slot = EntitySlot(index);
        // Graze/block-grief is live simulation: freeze resident-inactive.
        let gpos = state.entities.pos(slot);
        if !state.world.is_active(
            (gpos[0].floor() as i32).div_euclid(16),
            (gpos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let row = state.entities.row(slot);
        let wool = crate::tick::wool_of(&row.appearance);
        let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
            continue;
        };
        if !state.entities.body(slot).alive
            || wool & 0x10 == 0
            || !state
                .registry
                .blueprint(type_id)
                .is_some_and(|blueprint| blueprint.wool)
            || rng.below(1000 / CYCLE) != 0
        {
            continue;
        }
        let id = state.entities.body(slot).id;
        let pos = state.entities.pos(slot);
        let below = crate::boundary::BlockPos {
            x: pos[0].floor() as i32,
            y: pos[1].floor() as i32 - 1,
            z: pos[2].floor() as i32,
        };
        if state.world.block_at(below.x, below.y, below.z)
            != Some(state.registry.states.grass_block)
        {
            continue;
        }
        if mob_griefing {
            let dirt = state.registry.states.dirt;
            state.world.set_block(below.x, below.y, below.z, dirt);
            changes.blocks.push(crate::boundary::BlockChange {
                position: below,
                state: dirt,
            });
        }
        let row = state.entities.row_mut(slot);
        crate::tick::set_wool(std::sync::Arc::make_mut(&mut row.appearance), wool & 0x0F);
        state.appearance_changed.push(id);
    }
}

/// Sunlight at the head, with no water at the feet.
fn open_sky(state: &TickState, pos: [f64; 3], height: f64) -> bool {
    let materials = &state.registry.materials;
    let x = pos[0].floor() as i32;
    let z = pos[2].floor() as i32;
    let feet = pos[1].floor() as i32;
    if state
        .world
        .block_at(x, feet, z)
        .is_none_or(|block| materials.classify(block) == BlockMaterial::Water)
    {
        return false;
    }
    let head = (pos[1] + height).floor() as i32;
    state
        .world
        .light_at(x, head, z)
        .is_some_and(|(sky, _)| sky == 15)
}

/// SplitMix64: deterministic per (world seed, tick).
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    pub(crate) fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
#[path = "natural_spawn_parity_tests.rs"]
mod parity_tests;

fn explicitly_retained(row: &crate::entities::ColdRow) -> bool {
    persistent(row)
        || matches!(
            row.special_mob.as_deref(),
            Some(crate::special_mobs::Retained::Warden(_))
        )
}

/// Strict subset of despawn's "nothing to do" rows that needs no blueprint.
pub(crate) fn no_work_before_category(
    row: &crate::entities::ColdRow,
    peaceful_difficulty: bool,
    no_players: bool,
) -> bool {
    !peaceful_difficulty && (no_players || row.no_action_time == 0 && explicitly_retained(row))
}
