//! Beds, vanilla `BedBlock.useWithoutItem` + `Player.startSleepInBed`: a
//! player lies down in a free bed at night (or in a thunderstorm) with no
//! monster near. A valid bed also sets respawn during daytime. Once the
//! configured quorum has slept long enough, the night is skipped and rain
//! clears. Waking and respawning share the bed's safe-exit search.

use std::sync::Arc;

use crate::blueprint::MobCategory;
use crate::boundary::{BlockPos, ConnectionId, EntityDataValue, EntityId, PlayerChange};
use crate::commands::Weather;
use crate::tick::TickState;

/// Vanilla `Pose.SLEEPING` ordinal.
const SLEEPING_POSE: u8 = 2;
/// `LivingEntity.SLEEPING_POS_ID` entity data index; the edge's
/// `mc_protocol` `LIVING_ENTITY_DATA_SLEEPING_POS_INDEX` is the wire evidence.
pub const SLEEPING_POS_DATA_INDEX: u8 = 14;
/// Ticks a player must have slept before the night may be skipped.
pub(crate) const SLEEP_TICKS: u32 = 100;
/// Day times (ticks into the day) at which beds work (vanilla `BedRule`).
const NIGHT: std::ops::Range<i64> = 12_542..23_460;
/// Monsters this close to a bed keep its sleeper awake.
const MONSTER_RANGE: [f64; 2] = [8.0, 5.0];
/// Vanilla bed height above its block's floor, and the sleeper's offset in it.
const BED_TOP: f64 = 0.5625;
const SLEEPER_HEIGHT: f64 = 0.6875;

/// A player lying in a bed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sleeper {
    /// Head cell of the bed.
    pub bed: BlockPos,
    /// Ticks slept so far, capped at [`SLEEP_TICKS`].
    pub ticks: u32,
    /// Health when lying down: a hit wakes the sleeper.
    health: f32,
}

/// Whether the clock or the weather allows sleeping.
pub(crate) fn can_sleep_now(state: &TickState) -> bool {
    NIGHT.contains(&state.day_time.rem_euclid(24_000)) || state.weather.0 == Weather::Thunder
}

fn say(state: &mut TickState, connection: ConnectionId, message: &str) {
    state.push_player(
        connection,
        PlayerChange::Chat {
            message: message.to_owned(),
        },
    );
}

/// Whether `pos` holds a bed block.
fn is_bed(state: &TickState, pos: BlockPos) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|current| state.registry.blocks.by_id(mc_world::BlockStateId(current)))
        .is_some_and(|block| block.block.id.path().ends_with("_bed"))
}

/// A monster whose box meets the box around `bed` that keeps sleepers up.
/// Vanilla exempts slimes/magma cubes entirely and zombified piglins unless
/// angered by a player; endermen still block sleep.
fn monster_near(state: &TickState, bed: BlockPos) -> bool {
    let centre = [f64::from(bed.x) + 0.5, f64::from(bed.z) + 0.5];
    let floor = f64::from(bed.y);
    state.entities.slots().any(|slot| {
        let row = state.entities.body(slot);
        if !row.alive || crate::natural_spawn::category_of(state, slot) != MobCategory::Monster {
            return false;
        }
        if mob_exempt_from_bed_check(state, slot) {
            return false;
        }
        let at = state.entities.pos(slot);
        (at[0] - centre[0]).abs() < MONSTER_RANGE[0] + row.half_width
            && (at[2] - centre[1]).abs() < MONSTER_RANGE[0] + row.half_width
            && at[1] + row.height > floor - MONSTER_RANGE[1]
            && at[1] < floor + MONSTER_RANGE[1]
    })
}

/// Vanilla BedBlock monster exceptions: slimes and magma cubes never block
/// sleep; zombified piglins block only while hostile to a player.
fn mob_exempt_from_bed_check(state: &TickState, slot: crate::entities::EntitySlot) -> bool {
    let crate::entities::EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return false;
    };
    let Some(blueprint) = state.registry.blueprint(type_id) else {
        return false;
    };
    match blueprint.name.as_str() {
        "minecraft:slime" | "minecraft:magma_cube" => true,
        "minecraft:zombified_piglin" => !mob_hostile_to_players(state, slot),
        _ => false,
    }
}

/// Whether the mob is currently hostile to a player (its combat credit or
/// recent player damage). Neutral mobs at peace do not block sleep.
fn mob_hostile_to_players(state: &TickState, slot: crate::entities::EntitySlot) -> bool {
    let row = state.entities.row(slot);
    // Vanilla player-kill credit persists 100 ticks after a player hit: a
    // piglin hurt by a player in that window counts as angered.
    if row
        .last_player_hit
        .is_some_and(|(_, at)| state.tick.saturating_sub(at) <= 100)
    {
        return true;
    }
    row.last_hurt > 0.0
}

/// Publish the sleeping position (and pose) of `entity`.
fn set_lying(state: &mut TickState, entity: EntityId, bed: Option<BlockPos>) {
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return;
    };
    state
        .entities
        .set_half_width(slot, if bed.is_some() { 0.1 } else { 0.3 });
    state.entities.body_mut(slot).height = if bed.is_some() { 0.2 } else { 1.8 };
    let row = state.entities.row_mut(slot);
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.pose = if bed.is_some() { SLEEPING_POSE } else { 0 };
    if bed.is_some() {
        appearance.shared_flags &= !crate::tick::SNEAKING_FLAG;
    }
    let value = EntityDataValue::OptionalBlockPos(bed.map(|bed| [bed.x, bed.y, bed.z]));
    match appearance
        .extra
        .iter_mut()
        .find(|(index, _)| *index == SLEEPING_POS_DATA_INDEX)
    {
        Some(entry) => entry.1 = value,
        None => appearance.extra.push((SLEEPING_POS_DATA_INDEX, value)),
    }
    state.appearance_changed.push(entity);
}

/// A hand use of the block at `clicked`. `false` when it is not a bed;
/// otherwise the use is consumed: the player lies down or is told why not.
pub(crate) fn use_bed(
    state: &mut TickState,
    entity: EntityId,
    connection: ConnectionId,
    clicked: BlockPos,
) -> bool {
    let Some(current) = state.world.block_at(clicked.x, clicked.y, clicked.z) else {
        return false;
    };
    if !is_bed(state, clicked) {
        return false;
    }
    // Either half sleeps in the head cell.
    let Some((_, head)) = crate::placement::other_half(&state.registry.blocks, clicked, current)
    else {
        return false;
    };
    let Some(player) = state.players.get(&entity) else {
        return true;
    };
    if player.spectator() || state.sleepers.contains_key(&entity) {
        return true;
    }
    let creative = player.creative();
    let Some(slot) = state.entities.slot_of(entity) else {
        return true;
    };
    if !is_bed(state, head) || state.sleepers.values().any(|other| other.bed == head) {
        say(state, connection, "This bed is occupied");
        return true;
    }
    let [yaw, _, _] = state.entities.rot(slot);
    // Vanilla sleeping distance: within 3 blocks of either half. Packet
    // reach (4.5+1) can still target the bed from farther away.
    if !creative {
        let at = state.entities.pos(slot);
        let near_half = |half: BlockPos| {
            let dx = (f64::from(half.x) + 0.5 - at[0]).abs();
            let dy = (f64::from(half.y) + 0.5 - at[1]).abs();
            let dz = (f64::from(half.z) + 0.5 - at[2]).abs();
            (dx * dx + dy * dy + dz * dz).sqrt() <= 3.0
        };
        if !near_half(clicked) && !near_half(head) {
            say(state, connection, "You are too far away");
            return true;
        }
        // Vanilla refuses entry when no wake-up space exists, instead of
        // leaving the player sealed inside on exit.
        if bed_exit(state, head, yaw).is_none() {
            say(state, connection, "This bed is obstructed");
            return true;
        }
    }
    if let Some(player) = state.players.get_mut(&entity) {
        player.respawn_point = Some([head.x, head.y, head.z]);
        player.respawn_dimension = state.dimension;
        player.respawn_forced = false;
        player.respawn_yaw = yaw;
        player.respawn_pitch = 0.0;
    }
    if !can_sleep_now(state) {
        say(state, connection, "You can sleep only at night");
        return true;
    }
    if !creative && monster_near(state, head) {
        say(
            state,
            connection,
            "You may not rest now; there are monsters nearby",
        );
        return true;
    }
    let lying = [
        f64::from(head.x) + 0.5,
        f64::from(head.y) + SLEEPER_HEIGHT,
        f64::from(head.z) + 0.5,
    ];
    let [yaw, pitch, _] = state.entities.rot(slot);
    let health = state.entities.row(slot).health;
    state.entities.set_pos(slot, lying);
    state.entities.set_vel(slot, [0.0; 3]);
    state.sleepers.insert(
        entity,
        Sleeper {
            bed: head,
            ticks: 0,
            health,
        },
    );
    if let Some(player) = state.players.get_mut(&entity) {
        // The spawn was set even if sleeping would have been refused above.
        player
            .statistics
            .increment_custom(mc_data::statistics::CustomStat::SleepInBed, 1);
        player
            .statistics
            .set_custom(mc_data::statistics::CustomStat::TimeSinceRest, 0);
    }
    set_lying(state, entity, Some(head));
    crate::advancements::trigger(
        state,
        entity,
        "minecraft:slept_in_bed",
        serde_json::json!({}),
    );
    state.correct_position(connection, lying, yaw, pitch);
    true
}

/// Get `entity` out of its bed (leave-bed command, morning, hit, death).
pub(crate) fn wake(state: &mut TickState, entity: EntityId) {
    if let Some(sleeper) = state.sleepers.remove(&entity) {
        if let Some(slot) = state.entities.slot_of(entity) {
            let [yaw, pitch, _] = state.entities.rot(slot);
            if let Some(exit) = bed_exit(state, sleeper.bed, yaw) {
                state.entities.set_pos(slot, exit);
                state.entities.set_vel(slot, [0.0; 3]);
                if let Some(connection) = state.connections.entity_connection(entity) {
                    state.correct_position(connection, exit, yaw, pitch);
                }
            }
        }
        set_lying(state, entity, None);
    }
}

/// Once per tick: count sleep time, wake those whose bed is gone, whose
/// night is over or who were hurt, and skip the night when the gamerule's
/// percentage of active non-spectator players sleeps soundly.
pub(crate) fn run(state: &mut TickState) {
    if state.sleepers.is_empty() {
        return;
    }
    let can_sleep = can_sleep_now(state);
    let mut sleepers: Vec<EntityId> = state.sleepers.keys().copied().collect();
    sleepers.sort_unstable();
    for entity in sleepers {
        let Some(sleeper) = state.sleepers.get_mut(&entity) else {
            continue;
        };
        sleeper.ticks = (sleeper.ticks + 1).min(SLEEP_TICKS);
        let (bed, health) = (sleeper.bed, sleeper.health);
        let hurt = state
            .entities
            .slot_of(entity)
            .is_none_or(|slot| state.entities.row(slot).health < health);
        if hurt || !can_sleep || !is_bed(state, bed) {
            wake(state, entity);
        }
    }
    let mut active = 0_u64;
    let mut deep_sleepers = 0_u64;
    for (id, player) in &state.players {
        if player.spectator() || state.entities.slot_of(*id).is_none() {
            continue;
        }
        active += 1;
        if state
            .sleepers
            .get(id)
            .is_some_and(|sleeper| sleeper.ticks >= SLEEP_TICKS)
        {
            deep_sleepers += 1;
        }
    }
    let percentage = crate::command_rules::integer(state, "players_sleeping_percentage") as u64;
    let required = (u128::from(active) * u128::from(percentage))
        .div_ceil(100)
        .max(1);
    if u128::from(deep_sleepers) >= required {
        if crate::command_rules::boolean(state, "advance_time") {
            state.day_time = (state.day_time.div_euclid(24_000) + 1) * 24_000;
        }
        if crate::command_rules::boolean(state, "advance_weather")
            && state.weather.0 != Weather::Clear
        {
            state.weather = (Weather::Clear, 0);
            state.thundering = false;
            state.thunder_remaining_ticks = 0;
            state.clear_remaining_ticks = 0;
        }
        let sleepers: Vec<EntityId> = state.sleepers.keys().copied().collect();
        for entity in sleepers {
            wake(state, entity);
        }
    }
}

/// Forced command points allow a safe non-bed cell; ordinary bed points
/// remain valid only while the bed and an exit stand.
pub(crate) fn respawn_position(state: &TickState, entity: EntityId) -> Option<[f64; 3]> {
    let player = state.players.get(&entity)?;
    respawn_position_for(state, player)
}

pub(crate) fn respawn_position_for(
    state: &TickState,
    player: &crate::player::PlayerState,
) -> Option<[f64; 3]> {
    let [x, y, z] = player.respawn_point?;
    if player.respawn_dimension != state.dimension {
        return None;
    }
    if let Some(position) = crate::dimensions::rules::anchor_spawn(state, BlockPos { x, y, z }) {
        return Some(position);
    }
    if !state.dimension.beds_work() && is_bed(state, BlockPos { x, y, z }) {
        return None;
    }
    if !is_bed(state, BlockPos { x, y, z }) {
        let safe = |y| {
            state.world.block_at(x, y, z).is_some_and(|block| {
                state.registry.materials.classify(block) != mc_physics::BlockMaterial::Solid
                    && state
                        .registry
                        .blocks
                        .by_id(mc_world::BlockStateId(block))
                        .is_some_and(|state| !state.has_fluid())
            })
        };
        return (player.respawn_forced && safe(y) && safe(y + 1)).then_some([
            f64::from(x) + 0.5,
            f64::from(y) + 0.1,
            f64::from(z) + 0.5,
        ]);
    }
    bed_exit(state, BlockPos { x, y, z }, player.respawn_yaw)
}

/// Both bed halves, all perimeter cells and the lower floor tier. The search is
/// bounded to 36 cells and ordered toward the player's saved facing.
fn bed_exit(state: &TickState, head: BlockPos, yaw: f32) -> Option<[f64; 3]> {
    let current = state.world.block_at(head.x, head.y, head.z)?;
    let foot = crate::placement::other_half(&state.registry.blocks, head, current)
        .map_or(head, |(other, _)| other);
    let offsets = [
        [1, 0],
        [1, 1],
        [0, 1],
        [-1, 1],
        [-1, 0],
        [-1, -1],
        [0, -1],
        [1, -1],
    ];
    let first = (((f64::from(yaw).to_radians() + std::f64::consts::FRAC_PI_2)
        / std::f64::consts::FRAC_PI_4)
        .round() as i32)
        .rem_euclid(8) as usize;
    // Vanilla fits the player AABB in the exit cells (DismountHelper):
    // partial blocks like carpet do not obstruct; only intersecting solid
    // shapes and hazards block the exit.
    let free = |x, y, z| {
        let Some(id) = state.world.block_at(x, y, z) else {
            return false;
        };
        if state.registry.hazard_flags.get(id as usize) != Some(&0) {
            return false;
        }
        if state.registry.materials.classify(id) == mc_physics::BlockMaterial::Air {
            return true;
        }
        if state.registry.materials.classify(id) == mc_physics::BlockMaterial::Solid {
            return false;
        }
        let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
            return false;
        };
        !mc_data::collision_shapes::vanilla_collision_shapes()
            .get_for_state(id, &block.block.id, &block.properties)
            .is_some_and(|shape| {
                shape.iter().any(|collision_box| {
                    let units =
                        mc_data::collision_shapes::COLLISION_UNITS_PER_BLOCK as f64;
                    let [_, min_y, _, _, max_y, _] = collision_box.coordinates();
                    // Carpet-thin shapes (<=1/16) never intersect the player
                    // AABB standing at this cell's floor.
                    f64::from(max_y) / units > 0.0625
                        && f64::from(min_y) / units < 1.8
                })
            })
    };
    for dy in [0, -1] {
        for anchor in [head, foot] {
            for index in 0..8 {
                let [dx, dz] = offsets[(first + index) % 8];
                let (x, y, z) = (anchor.x + dx, head.y + dy, anchor.z + dz);
                if free(x, y, z)
                    && free(x, y + 1, z)
                    && state.world.block_at(x, y - 1, z).is_some_and(|id| {
                        state.registry.materials.classify(id) == mc_physics::BlockMaterial::Solid
                            && state.registry.hazard_flags.get(id as usize) == Some(&0)
                    })
                {
                    return Some([f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5]);
                }
            }
        }
    }
    [head, foot]
        .into_iter()
        .find(|pos| free(pos.x, pos.y + 1, pos.z) && free(pos.x, pos.y + 2, pos.z))
        .map(|pos| {
            [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + BED_TOP,
                f64::from(pos.z) + 0.5,
            ]
        })
}

pub(crate) fn respawn_rotation(state: &TickState, entity: EntityId) -> [f32; 2] {
    if respawn_position(state, entity).is_some() {
        let player = &state.players[&entity];
        [player.respawn_yaw, player.respawn_pitch]
    } else {
        state.world.world_spawn_rotation()
    }
}

/// Respawn waits for its off-tick loads rather than treating an unloaded
/// personal point as an invalid bed and silently falling back.
pub(crate) fn respawn_chunks_loaded(state: &TickState, entity: EntityId) -> bool {
    let loaded = |spawn: [i32; 3]| {
        let x = spawn[0].div_euclid(16);
        let z = spawn[2].div_euclid(16);
        (-1..=1).all(|dx| (-1..=1).all(|dz| state.world.has_chunk(x + dx, z + dz)))
    };
    loaded(state.world.world_spawn())
        && state
            .players
            .get(&entity)
            .and_then(|player| player.respawn_point)
            .is_none_or(loaded)
}

#[cfg(test)]
#[path = "sleep_tests.rs"]
mod tests;
