//! Trial potion triggers at admitted damage and the once-only death transition.
use crate::boundary::{BlockPos, EntityId};
use crate::effects::EffectId;
use crate::entities::EntitySlot;
use crate::tick::TickState;

fn has(state: &TickState, slot: EntitySlot, effect: EffectId) -> bool {
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    state
        .players
        .get(&body.id)
        .map_or(&row.active_effects, |p| &p.active_effects)
        .has(effect)
}
fn draw(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut value = *seed;
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}
fn seed(state: &TickState, slot: EntitySlot) -> u64 {
    let uuid = state.entities.row(slot).uuid.as_u128();
    uuid as u64 ^ (uuid >> 64) as u64 ^ state.tick.rotate_left(23)
}
fn spawn(
    state: &mut TickState,
    name: &str,
    pos: [f64; 3],
    size: Option<u8>,
    spawned: &mut Vec<EntityId>,
) {
    let id = mc_data::Identifier::parse(name).expect("vanilla entity");
    let Some(type_id) = state.registry.entity_types.id_of(&id) else {
        return;
    };
    let Some(blueprint) = state.registry.blueprint(type_id) else {
        return;
    };
    let id = state.spawn_mob(type_id, pos, blueprint.params.max_health);
    if let Some(size) = size {
        let slot = state.entities.slot_of(id).expect("trigger spawn");
        crate::entity_loot::set_size(state, slot, size, true);
    }
    spawned.push(id);
}

pub(crate) fn accepted_hit(state: &mut TickState, slot: EntitySlot, spawned: &mut Vec<EntityId>) {
    if !has(state, slot, EffectId::Infested) {
        return;
    }
    let mut random = seed(state, slot);
    if !draw(&mut random).is_multiple_of(10) {
        return;
    }
    let mut pos = state.entities.pos(slot);
    pos[1] += state.entities.body(slot).height * 0.5;
    let count = 1 + draw(&mut random) % 2;
    for _ in 0..count {
        spawn(state, "minecraft:silverfish", pos, None, spawned);
    }
}

pub(crate) fn death(state: &mut TickState, id: EntityId, spawned: &mut Vec<EntityId>) {
    let Some(slot) = state.entities.slot_of_any(id) else {
        return;
    };
    let pos = state.entities.pos(slot);
    let oozing = has(state, slot, EffectId::Oozing);
    let weaving = has(state, slot, EffectId::Weaving);
    let wind = has(state, slot, EffectId::WindCharged);
    let mut random = seed(state, slot);
    if oozing {
        let nearby = state
            .entities
            .slots()
            .filter(|other| {
                let body = state.entities.body(*other);
                let row = state.entities.row(*other);
                body.alive
                    && row.loot.slime_size.is_some()
                    && state
                        .entities
                        .pos(*other)
                        .into_iter()
                        .zip(pos)
                        .all(|(a, b)| (a - b).abs() <= 2.0)
            })
            .count();
        let cap = state
            .gamerules
            .get("max_entity_cramming")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(24);
        let count = if cap == 0 {
            2
        } else {
            cap.saturating_sub(nearby).min(2)
        };
        for index in 0..count {
            spawn(
                state,
                "minecraft:slime",
                [pos[0] + (index as f64 - 0.5) * 0.4, pos[1] + 0.5, pos[2]],
                Some(2),
                spawned,
            );
        }
    }
    if weaving
        && (state.players.contains_key(&id)
            || crate::command_rules::boolean(state, crate::command_rules::BoolRule::MOB_GRIEFING))
    {
        cobwebs(state, pos, &mut random);
    }
    if wind {
        let center = [
            pos[0],
            pos[1] + state.entities.body(slot).height * 0.5,
            pos[2],
        ];
        let power = 3.0 + (draw(&mut random) >> 40) as f32 / 16_777_216.0 * 2.0;
        crate::projectiles::payloads::wind_burst(state, center, power);
    }
}

fn cobwebs(state: &mut TickState, pos: [f64; 3], random: &mut u64) {
    let id = mc_data::Identifier::parse("minecraft:cobweb").expect("vanilla block");
    let Some(web) = state
        .registry
        .blocks
        .block(&id)
        .map(|block| block.default.0)
    else {
        return;
    };
    let origin = [
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    ];
    let count = 2 + draw(random) % 2;
    let mut placed = 0;
    for _ in 0..15 {
        let cell: [i32; 3] =
            std::array::from_fn(|axis| origin[axis] + (draw(random) % 3) as i32 - 1);
        let Some(current) = state.world.block_at(cell[0], cell[1], cell[2]) else {
            continue;
        };
        let Some(below) = state.world.block_at(cell[0], cell[1] - 1, cell[2]) else {
            continue;
        };
        if !crate::tick::replaceable(state, current)
            || !state
                .registry
                .collision_boxes
                .get(below as usize)
                .is_some_and(|boxes| {
                    boxes
                        .iter()
                        .any(|b| b.as_blocks() == [0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
                })
        {
            continue;
        }
        let pos = BlockPos {
            x: cell[0],
            y: cell[1],
            z: cell[2],
        };
        state.write_block(pos, web);
        placed += 1;
        if placed >= count {
            break;
        }
    }
}
