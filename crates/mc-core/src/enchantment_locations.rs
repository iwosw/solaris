//! Terrain-dependent equipment effects through canonical block intents and durable ticks.
use crate::boundary::{BlockPos, EntityId};
use crate::enchantments::{level, random};
use crate::intent::{Footprint, Intent, IntentKind};
use crate::tick::TickState;

pub(crate) fn run(state: &mut TickState, intents: &mut Vec<Intent>) {
    // Player equipment is authoritative here. Enumerate its owners rather than
    // probing the player map once for every resident mob; retain dense-slot
    // order for frost-walker intents and soul-speed wear/outbox publication.
    let mut slots = std::mem::take(&mut state.serial_entity_slots);
    slots.clear();
    slots.extend(
        state
            .players
            .keys()
            .filter_map(|id| state.entities.slot_of(*id)),
    );
    slots.sort_unstable_by_key(|slot| slot.0);
    run_slots(state, intents, slots.iter().copied());
    slots.clear();
    state.serial_entity_slots = slots;
}

#[cfg(test)]
pub(crate) fn run_all_rows(state: &mut TickState, intents: &mut Vec<Intent>) {
    let rows = state.entities.len() as u32;
    run_slots(state, intents, (0..rows).map(crate::entities::EntitySlot));
}

fn run_slots(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    slots: impl Iterator<Item = crate::entities::EntitySlot>,
) {
    for slot in slots {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        let actor = body.id;
        let Some(player) = state.players.get(&actor) else {
            continue;
        };
        if !body.alive || player.spectator() {
            continue;
        }
        let boots = &player.inventory.slots[36];
        let soul_level = level(boots, "soul_speed");
        let frost_level = level(boots, "frost_walker");
        let previously_active = player.soul_speed_active;
        let ground = state.entities.ground(slot);
        let pos = state.entities.pos(slot);
        let mounted = row.entity_life.mount.vehicle.is_some();
        let flying = row.appearance.pose == 1;
        let below = BlockPos {
            x: pos[0].floor() as i32,
            y: pos[1].floor() as i32 - 1,
            z: pos[2].floor() as i32,
        };
        let soul_block = state
            .world
            .block_at(below.x, below.y, below.z)
            .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
            .is_some_and(|block| {
                state.registry.tags.contains_raw_id(
                    "minecraft:block",
                    "minecraft:soul_speed_blocks",
                    block.block.raw_id,
                )
            });
        let soul_active =
            soul_level > 0 && !mounted && !flying && (soul_block || previously_active && !ground);
        let player = state.players.get_mut(&actor).expect("equipment owner");
        if soul_active != player.soul_speed_active {
            player.soul_speed_active = soul_active;
            player.attribute_equipment = None;
        }
        if soul_active
            && soul_block
            && ground
            && !player.creative()
            && random(&mut player.enchantment_seed) < 0.04
        {
            crate::projectiles::weapons::wear(state, actor, 36, 1);
            crate::projectiles::weapons::sync(state, actor);
        }
        if frost_level > 0 && ground && !mounted && !flying {
            freeze(state, actor, pos, below.y, frost_level, intents);
        }
    }
}

fn freeze(
    state: &mut TickState,
    actor: EntityId,
    pos: [f64; 3],
    y: i32,
    level: u32,
    intents: &mut Vec<Intent>,
) {
    let radius = level.saturating_add(2).min(16) as i32;
    let frost_id = mc_data::Identifier::parse("minecraft:frosted_ice").expect("vanilla id");
    let Some(frost) = state
        .registry
        .blocks
        .by_name_and_props(&frost_id, &[("age".into(), "0".into())])
    else {
        return;
    };
    let air = state.registry.states.air;
    let water = state.registry.states.water;
    let cx = pos[0].floor() as i32;
    let cz = pos[2].floor() as i32;
    for x in cx - radius..=cx + radius {
        for z in cz - radius..=cz + radius {
            let distance =
                (f64::from(x) + 0.5 - pos[0]).powi(2) + (f64::from(z) + 0.5 - pos[2]).powi(2);
            if distance > f64::from(radius * radius) {
                continue;
            }
            let cell = BlockPos { x, y, z };
            let above = BlockPos { x, y: y + 1, z };
            if state.world.block_at(x, y, z) != Some(water)
                || state.world.block_at(x, y + 1, z) != Some(air)
            {
                continue;
            }
            let seq = state.next_seq(actor);
            intents.push(Intent {
                actor,
                seq,
                kind: IntentKind::PlaceBlock {
                    pos: cell,
                    state: frost.0,
                },
                footprint: Footprint {
                    entity_reads: vec![actor],
                    block_expect: vec![(cell, water), (above, air)],
                    block_write: vec![(cell, frost.0)],
                    ..Default::default()
                },
            });
        }
    }
}

pub(crate) fn changed(state: &mut TickState, pos: BlockPos) {
    let frosted = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| block.block.id.path() == "frosted_ice");
    static ID: std::sync::LazyLock<mc_data::Identifier> = std::sync::LazyLock::new(|| {
        mc_data::Identifier::parse("minecraft:frosted_ice").expect("vanilla id")
    });
    if frosted && !state.block_ticks.contains(crate::plants::old(pos), &ID, 0) {
        let delay = 60 + (state.tick ^ pos.x as u64 ^ (pos.z as u64).rotate_left(17)) % 61;
        state
            .block_ticks
            .schedule(mc_world::ScheduledBlockTick::new(
                pos,
                ID.clone(),
                state.tick.saturating_add(delay),
                0,
            ));
    }
}

pub(crate) fn scheduled(state: &mut TickState, pos: BlockPos, writes: &mut Vec<(BlockPos, u32)>) {
    let Some(id) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
        return;
    };
    if block.block.id.path() != "frosted_ice" {
        return;
    }
    let age = block
        .properties
        .iter()
        .find(|(name, _)| name == "age")
        .and_then(|(_, value)| value.parse::<u8>().ok())
        .unwrap_or(0);
    let next = if age >= 3 {
        state.registry.states.water
    } else {
        let Some(next) = state
            .registry
            .blocks
            .by_name_and_props(&block.block.id, &[("age".into(), (age + 1).to_string())])
        else {
            return;
        };
        next.0
    };
    state.world.set_block(pos.x, pos.y, pos.z, next);
    writes.push((pos, next));
    changed(state, pos);
}
