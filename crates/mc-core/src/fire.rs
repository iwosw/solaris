//! Tick-owned world fire. Entity contact damage and retained burning belong
//! exclusively to contact_hazards; this module changes blocks and schedules work.

use mc_data::{Identifier, ItemStack};
use mc_world::{BlockRegistry, BlockState, BlockStateId, ScheduledBlockTick};

use crate::boundary::{BlockPos, ChangeSet, EntityId, Hand};
use crate::config::CoreError;
use crate::placement::Face;
use crate::tick::TickState;

const NEIGHBOURS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

fn property<'a>(block: &'a BlockState, key: &str) -> Option<&'a str> {
    block
        .properties
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value.as_str()))
}

#[derive(Debug, Clone, Copy, Default)]
struct FireFact {
    age: Option<u8>,
    fuel: bool,
    burn: bool,
    air: bool,
    water: bool,
    sturdy: bool,
    rain_blocking: bool,
    lit: Option<u32>,
}

/// Dense startup tables keep state-property parsing out of scheduled work.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub(crate) struct FireStates {
    facts: Vec<FireFact>,
    normal: [[u32; 16]; 32],
    soul: u32,
}

impl FireStates {
    pub(crate) fn resolve(blocks: &BlockRegistry) -> Result<Self, CoreError> {
        let id = |name: &str| Identifier::parse(name).expect("static block identifier");
        let normal = blocks
            .block(&id("minecraft:fire"))
            .ok_or_else(|| CoreError::Registry("missing minecraft:fire".into()))?;
        let soul = blocks
            .block(&id("minecraft:soul_fire"))
            .ok_or_else(|| CoreError::Registry("missing minecraft:soul_fire".into()))?
            .default
            .0;
        let mut states = [[u32::MAX; 16]; 32];
        let mut facts = vec![FireFact::default(); blocks.states().count()];
        for block in blocks.states() {
            let path = block.block.id.path();
            let age = (path == "fire")
                .then(|| property(block, "age").and_then(|age| age.parse::<u8>().ok()))
                .flatten();
            let mut shape = 0;
            for (bit, name) in ["up", "north", "east", "south", "west"]
                .into_iter()
                .enumerate()
            {
                if property(block, name) == Some("true") {
                    shape |= 1 << bit;
                }
            }
            if let Some(age) = age {
                states[shape as usize][age as usize] = block.id.0;
            }
            let wet = path == "water" || property(block, "waterlogged") == Some("true");
            let (ignite_odds, burn_odds) =
                mc_data::fire_flammability::odds(block.block.id.as_str());
            let fuel = !wet && ignite_odds != 0;
            let lightable = matches!(path, "campfire" | "soul_campfire" | "candle")
                || path.ends_with("_candle")
                || path.ends_with("_candle_cake")
                || path == "candle_cake";
            let lit = (lightable && !wet && property(block, "lit") == Some("false"))
                .then(|| {
                    block.block.states.iter().find_map(|next| {
                        let next_block = blocks.by_id(*next)?;
                        (property(next_block, "lit") == Some("true")
                            && block.properties.iter().all(|(key, value)| {
                                key == "lit" || property(next_block, key) == Some(value.as_str())
                            }))
                        .then_some(next.0)
                    })
                })
                .flatten();
            facts[block.id.0 as usize] = FireFact {
                age,
                fuel,
                burn: !wet && burn_odds != 0,
                air: matches!(path, "air" | "cave_air" | "void_air"),
                water: wet,
                rain_blocking: wet
                    || mc_data::collision_shapes::vanilla_collision_shapes()
                        .get_for_state(block.id.0, &block.block.id, &block.properties)
                        .is_some_and(|shape| !shape.is_empty()),
                lit,
                sturdy: mc_data::block_facts::has_full_sturdy_face(
                    block.id.0,
                    &block.block.id,
                    &block.properties,
                    mc_data::block_facts::SturdyFace::Up,
                ),
            };
        }
        if states.iter().flatten().any(|state| *state == u32::MAX) {
            return Err(CoreError::Registry(format!(
                "incomplete {} age/attachment states",
                normal.id
            )));
        }
        Ok(Self {
            facts,
            normal: states,
            soul,
        })
    }

    fn fact(&self, id: u32) -> FireFact {
        self.facts.get(id as usize).copied().unwrap_or_default()
    }
}

fn offset(pos: BlockPos, delta: [i32; 3]) -> Option<BlockPos> {
    Some(BlockPos {
        x: pos.x.checked_add(delta[0])?,
        y: pos.y.checked_add(delta[1])?,
        z: pos.z.checked_add(delta[2])?,
    })
}

fn at(state: &TickState, pos: BlockPos) -> Option<&BlockState> {
    state
        .registry
        .blocks
        .by_id(BlockStateId(state.world.block_at(pos.x, pos.y, pos.z)?))
}

fn fact(state: &TickState, pos: BlockPos) -> FireFact {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .map_or(FireFact::default(), |id| {
            state.registry.fire_states.fact(id)
        })
}

fn soul_support(state: &TickState, pos: BlockPos) -> bool {
    offset(pos, [0, -1, 0])
        .and_then(|below| at(state, below))
        .is_some_and(|block| matches!(block.block.id.path(), "soul_sand" | "soul_soil"))
}

fn supported(state: &TickState, pos: BlockPos) -> bool {
    offset(pos, [0, -1, 0]).is_some_and(|below| fact(state, below).sturdy)
        || NEIGHBOURS
            .into_iter()
            .filter_map(|delta| offset(pos, delta))
            .any(|next| fact(state, next).fuel)
}

fn eternal(state: &TickState, pos: BlockPos) -> bool {
    offset(pos, [0, -1, 0])
        .and_then(|below| at(state, below))
        .is_some_and(|block| block.block.id.path() == "netherrack")
}

fn normal_state(state: &TickState, pos: BlockPos, age: u8) -> u32 {
    let below = offset(pos, [0, -1, 0])
        .map(|below| fact(state, below))
        .unwrap_or_default();
    let mut shape = 0;
    if !below.sturdy && !below.fuel {
        for (bit, delta) in [[0, 1, 0], [0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]]
            .into_iter()
            .enumerate()
        {
            if offset(pos, delta).is_some_and(|next| fact(state, next).fuel) {
                shape |= 1 << bit;
            }
        }
    }
    state.registry.fire_states.normal[shape][age as usize]
}

fn seed(state: &TickState, pos: BlockPos) -> u64 {
    state.world.seed() as u64
        ^ state.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos.x as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (pos.y as i64 as u64).rotate_left(21)
        ^ (pos.z as i64 as u64).rotate_left(42)
}

fn schedule(state: &mut TickState, pos: BlockPos) {
    let Some(block) = at(state, pos) else {
        return;
    };
    if !matches!(block.block.id.path(), "fire" | "soul_fire") {
        return;
    }
    if state.block_ticks.contains(pos, &block.block.id, 0) {
        return;
    }
    let block = block.block.id.clone();
    let deadline = state.tick.saturating_add(30 + seed(state, pos) % 10);
    state
        .block_ticks
        .schedule(ScheduledBlockTick::new(pos, block, deadline, 0));
}

/// Only mutates the loaded target and its durable tick queue. The caller owns
/// adding the successful post-image to its input/committing block-write list.
pub(crate) fn ignite_block(state: &mut TickState, pos: BlockPos) -> bool {
    let mut portal_writes = Vec::new();
    if crate::portals::nether::ignite(state, pos, &mut portal_writes) {
        state.merge_block_writes.extend(portal_writes);
        return true;
    }
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let next = if let Some(lit) = state.registry.fire_states.fact(current).lit {
        lit
    } else {
        let Some(block) = at(state, pos) else {
            return false;
        };
        if !matches!(block.block.id.path(), "air" | "cave_air" | "void_air") {
            return false;
        }
        if soul_support(state, pos) {
            state.registry.fire_states.soul
        } else if supported(state, pos) {
            normal_state(state, pos, 0)
        } else {
            return false;
        }
    };
    if current == next {
        return false;
    }
    state.world.set_block(pos.x, pos.y, pos.z, next);
    schedule(state, pos);
    true
}

/// Written cells and their neighbours wake existing fire, never absent chunks.
pub(crate) fn changed(state: &mut TickState, pos: BlockPos) {
    schedule(state, pos);
}

fn lava_target(state: &TickState, pos: BlockPos, roll: u64) -> Option<BlockPos> {
    if at(state, pos)?.block.id.path() != "lava" {
        return None;
    }
    let start = roll as usize % NEIGHBOURS.len();
    for index in 0..NEIGHBOURS.len() {
        let Some(target) = offset(pos, NEIGHBOURS[(start + index) % NEIGHBOURS.len()]) else {
            continue;
        };
        if !at(state, target)
            .is_some_and(|block| matches!(block.block.id.path(), "air" | "cave_air" | "void_air"))
        {
            continue;
        }
        if NEIGHBOURS
            .into_iter()
            .filter_map(|delta| offset(target, delta))
            .any(|fuel| fuel != pos && fact(state, fuel).fuel)
        {
            return Some(target);
        }
    }
    None
}

/// Seed-independent old B08 admission: stable lava only keeps ticking when
/// an air cell beside it has fuel. This does not consult the transient roll.
pub(crate) fn has_lava_fuel(state: &TickState, pos: BlockPos) -> bool {
    lava_target(state, pos, 0).is_some()
}

/// The native fluid owner calls this for due lava and owns requeueing lava.
/// Successful ignition publishes the fire post-image in this block phase.
pub(crate) fn lava_tick(
    state: &mut TickState,
    pos: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
) -> bool {
    let roll = seed(state, pos);
    if roll == 0 || !roll.is_multiple_of(3) || !spread_enabled(state, pos) {
        return false;
    }
    let Some(target) = lava_target(state, pos, roll) else {
        return false;
    };
    if !spread_enabled(state, target) || raining_at(state, target) || !ignite_block(state, target) {
        return false;
    }
    let next = state
        .world
        .block_at(target.x, target.y, target.z)
        .expect("ignited loaded lava neighbour");
    write(state, writes, target, next);
    true
}

fn raining_at(state: &TickState, pos: BlockPos) -> bool {
    if !state.dimension.has_weather() || state.rain_level <= 0.2 {
        return false;
    }
    // Full sky light can arrive sideways beneath a roof; direct exposure must
    // consult the actual column, including this tick's overlay writes.
    (pos.y + 1..state.dimension.geometry().max_y()).all(|y| {
        state
            .world
            .block_at(pos.x, y, pos.z)
            .is_some_and(|id| !state.registry.fire_states.fact(id).rain_blocking)
    })
}

fn spread_enabled(state: &TickState, pos: BlockPos) -> bool {
    let radius = crate::command_rules::integer(state, "fire_spread_radius_around_player");
    if radius == -1 {
        return true;
    }
    let radius_squared = f64::from(radius) * f64::from(radius);
    state.players.iter().any(|(entity, player)| {
        if player.spectator() {
            return false;
        }
        let Some(slot) = state.entities.slot_of(*entity) else {
            return false;
        };
        let point = state.entities.pos(slot);
        let dx = point[0] - f64::from(pos.x);
        let dy = point[1] - f64::from(pos.y);
        let dz = point[2] - f64::from(pos.z);
        dx * dx + dy * dy + dz * dz < radius_squared
    })
}

fn write(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>, pos: BlockPos, next: u32) {
    if state.world.block_at(pos.x, pos.y, pos.z) != Some(next) {
        state.world.set_block(pos.x, pos.y, pos.z, next);
    }
    writes.retain(|(cell, _)| *cell != pos);
    writes.push((pos, next));
}

/// Shared scheduler validates the queued identifier before entering this hook.
/// Age and combustion retain the bounded seeded gameplay rule; air spread
/// also requires a flammable neighbour. The native scheduler owns deadlines.
pub(crate) fn scheduled_tick(
    state: &mut TickState,
    pos: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
    _spawned: &mut Vec<EntityId>,
) {
    let Some(block) = at(state, pos) else {
        return;
    };
    let soul = block.block.id.path() == "soul_fire";
    let current = state.registry.fire_states.fact(block.id.0);
    let air = state.registry.states.air;
    if !soul && current.age.is_none() {
        return;
    }
    if (soul && !soul_support(state, pos)) || (!soul && !supported(state, pos)) {
        write(state, writes, pos, air);
        return;
    }
    schedule(state, pos);
    if soul || !spread_enabled(state, pos) {
        return;
    }
    let permanent = eternal(state, pos);
    let wet = NEIGHBOURS
        .into_iter()
        .filter_map(|delta| offset(pos, delta))
        .any(|next| fact(state, next).water);
    let rainy = raining_at(state, pos)
        || [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]]
            .into_iter()
            .filter_map(|delta| offset(pos, delta))
            .any(|next| raining_at(state, next));
    if wet || (!permanent && rainy) {
        write(state, writes, pos, air);
        return;
    }
    let age = current.age.expect("normal fire has age");
    if age == 15 && !permanent {
        write(state, writes, pos, air);
        return;
    }
    let next = normal_state(state, pos, age.saturating_add(1).min(15));
    write(state, writes, pos, next);
    let roll = seed(state, pos);
    if !roll.is_multiple_of(3) {
        return;
    }
    let start = roll as usize % NEIGHBOURS.len();
    for index in 0..NEIGHBOURS.len() {
        let Some(target) = offset(pos, NEIGHBOURS[(start + index) % NEIGHBOURS.len()]) else {
            continue;
        };
        if !fact(state, target).burn || !spread_enabled(state, target) || raining_at(state, target)
        {
            continue;
        }
        if at(state, target).is_some_and(|block| block.block.id.path() == "tnt") {
            let _ = crate::tnt::prime_block(state, target, 80);
        } else {
            let next = normal_state(state, target, 0);
            crate::plugin_inventories::invalidate_block(state, target);
            write(state, writes, target, next);
            schedule(state, target);
        }
        break;
    }
    // Burning a solid fuel cell and igniting air beside fuel are separate
    // vanilla operations. Without this pass fire cannot travel over a platform.
    for index in 0..NEIGHBOURS.len() {
        let Some(target) = offset(pos, NEIGHBOURS[(start + index) % NEIGHBOURS.len()]) else {
            continue;
        };
        if !fact(state, target).air
            || !spread_enabled(state, target)
            || raining_at(state, target)
            || !NEIGHBOURS
                .into_iter()
                .filter_map(|delta| offset(target, delta))
                .any(|fuel| fact(state, fuel).fuel)
        {
            continue;
        }
        let next = normal_state(state, target, 0);
        write(state, writes, target, next);
        schedule(state, target);
        break;
    }
}

/// Serial native input admission, success-only debit/wear for either hand.
/// Returns true for an ignition item even when the target refuses ignition.
pub(crate) fn use_on(
    state: &mut TickState,
    entity: EntityId,
    hand: Hand,
    clicked: BlockPos,
    face: Face,
    _changes: &mut ChangeSet,
) -> bool {
    let Some(player) = state.players.get(&entity) else {
        return false;
    };
    let slot = crate::tick::held_slot_index(player, hand);
    let held = &player.inventory.slots[slot];
    if held.is_empty() {
        return false;
    }
    let Some(name) = state.registry.items.name_of(held.item_id) else {
        return false;
    };
    let tool = name.as_str() == "minecraft:flint_and_steel";
    if !tool && name.as_str() != "minecraft:fire_charge" {
        return false;
    }
    if player.spectator() || player.vitals.is_dead() || player.item_on_cooldown(held.item_id) {
        return true;
    }
    let wear = if tool && !player.creative() {
        let max = state
            .registry
            .item_facts
            .facts_for_stack(held, &state.registry.items)
            .and_then(|facts| facts.max_damage)
            .unwrap_or(64);
        let damage = held.damage.unwrap_or(0).saturating_add(1);
        Some((damage, damage as u32 >= max))
    } else {
        None
    };
    if let Some((_, broken)) = wear {
        if crate::plugin_inventories::below_player_debit_floors(
            state,
            entity,
            &[],
            broken.then_some(slot),
        ) {
            return true;
        }
    } else if !tool && crate::tick::held_debit_below_floor(state, entity, hand, 1) {
        return true;
    }
    let target = if at(state, clicked).is_some_and(|block| block.block.id.path() == "tnt") {
        if crate::tnt::prime_block(state, clicked, 80).is_none() {
            return true;
        }
        None
    } else {
        let target = if fact(state, clicked).lit.is_some() {
            clicked
        } else {
            let Some(target) = offset(clicked, face.offset()) else {
                return true;
            };
            target
        };
        if !ignite_block(state, target) {
            return true;
        }
        Some(target)
    };
    if let Some(target) = target {
        let next = state
            .world
            .block_at(target.x, target.y, target.z)
            .expect("ignited loaded block");
        state.merge_block_writes.retain(|(cell, _)| *cell != target);
        state.merge_block_writes.push((target, next));
    }
    let before = state.players[&entity].inventory.slots[slot].clone();
    let player = state.players.get_mut(&entity).expect("admitted player");
    let connection = player.connection;
    if tool {
        player.statistics.increment(
            mc_data::statistics::StatType::Used,
            player.inventory.slots[slot].item_id,
            1,
        );
        if let Some((damage, broken)) = wear {
            let held = &mut player.inventory.slots[slot];
            if broken {
                player
                    .statistics
                    .increment(mc_data::statistics::StatType::Broken, held.item_id, 1);
                *held = ItemStack::EMPTY;
            } else {
                held.damage = Some(damage);
            }
            let change = player.inventory_contents();
            state.push_player(connection, change);
        }
    } else {
        let effects = player.consume_held(hand, 1);
        for change in effects.changes {
            state.push_player(connection, change);
        }
    }
    crate::advancements::accepted_block(
        state,
        entity,
        "minecraft:item_used_on_block",
        clicked,
        None,
        Some(before),
    );
    true
}

#[cfg(test)]
#[path = "fire_tests.rs"]
mod tests;
