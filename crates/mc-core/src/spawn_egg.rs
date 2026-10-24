//! SpawnEggItem.useOn admission, using 26.1.2 report-authored entity_data defaults.

use crate::boundary::{BlockPos, EntityId, Hand};
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use mc_nbt::Tag;

fn entity_type(state: &TickState, stack: &ItemStack, default: &Identifier) -> Option<u32> {
    for component in &stack.extended().preserved_components {
        if component.name == "!minecraft:entity_data" {
            return None;
        }
        if component.name != "minecraft:entity_data" {
            continue;
        }
        let mut bytes = component.value.as_slice();
        let Tag::Compound(fields) = mc_nbt::read_network_value(&mut bytes).ok()? else {
            return None;
        };
        if !bytes.is_empty() {
            return None;
        }
        let (_, Tag::String(name)) = fields.iter().find(|(key, _)| key == "id")? else {
            return None;
        };
        return state
            .registry
            .entity_types
            .id_of(&Identifier::parse(name).ok()?);
    }
    state.registry.entity_types.id_of(default)
}

/// Vanilla starts one block above the target and clips a downward movement:
/// one block normally, two for an upward face adjacent to the clicked block.
fn position(state: &TickState, target: BlockPos, lower: bool, width: f64) -> Option<[f64; 3]> {
    let x = f64::from(target.x) + 0.5;
    let z = f64::from(target.z) + 0.5;
    let start = f64::from(target.y) + 1.0;
    let mut floor = f64::from(target.y) - if lower { 1.0 } else { 0.0 };
    for bx in (x - width).floor() as i32..=(x + width).floor() as i32 {
        for by in (target.y - i32::from(lower))..=target.y {
            for bz in (z - width).floor() as i32..=(z + width).floor() as i32 {
                let id = state.world.block_at(bx, by, bz)?;
                for shape in state.registry.collision_boxes.get(id as usize)?.iter() {
                    let [a, _, c, d, top, f] = shape.as_blocks();
                    let top = f64::from(by) + top;
                    if x + width > f64::from(bx) + a
                        && x - width < f64::from(bx) + d
                        && z + width > f64::from(bz) + c
                        && z - width < f64::from(bz) + f
                        && top <= start
                    {
                        floor = floor.max(top);
                    }
                }
            }
        }
    }
    Some([x, floor, z])
}

/// Only shapes intersecting the actual body matter, not adjacent pen walls.
pub(crate) fn clear_body(state: &TickState, pos: [f64; 3], width: f64, height: f64) -> bool {
    let min = [pos[0] - width, pos[1] + 1e-6, pos[2] - width];
    let max = [pos[0] + width, pos[1] + height - 1e-6, pos[2] + width];
    for x in min[0].floor() as i32..=max[0].floor() as i32 {
        for y in (min[1].floor() as i32 - 1)..=max[1].floor() as i32 {
            for z in min[2].floor() as i32..=max[2].floor() as i32 {
                let Some(shapes) = state
                    .world
                    .block_at(x, y, z)
                    .and_then(|id| state.registry.collision_boxes.get(id as usize))
                else {
                    return false;
                };
                for shape in shapes.iter() {
                    let b = shape.as_blocks();
                    if min[0] < f64::from(x) + b[3] - 1e-6
                        && max[0] > f64::from(x) + b[0] + 1e-6
                        && min[1] < f64::from(y) + b[4] - 1e-6
                        && max[1] > f64::from(y) + b[1] + 1e-6
                        && min[2] < f64::from(z) + b[5] - 1e-6
                        && max[2] > f64::from(z) + b[2] + 1e-6
                    {
                        return false;
                    }
                }
            }
        }
    }
    true
}

pub(crate) fn use_on(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    clicked: BlockPos,
    face: u8,
) -> bool {
    let stack_snapshot = {
        let Some(player) = state.players.get(&actor) else {
            return false;
        };
        if player.spectator() || player.vitals.is_dead() {
            return false;
        }
        let index = if hand == Hand::Main {
            usize::from(player.inventory.selected)
        } else {
            crate::inventory::OFF_HAND
        };
        let stack = &player.inventory.slots[index];
        if stack.is_empty() {
            return false;
        }
        stack.clone()
    };
    let stack = &stack_snapshot;
    let Some(default) = state
        .registry
        .item_facts
        .facts_for_stack(stack, &state.registry.items)
        .and_then(|facts| facts.spawn_entity.as_ref())
    else {
        return false;
    };
    let Some(type_id) = entity_type(state, stack, default) else {
        return true;
    };
    let Some(blueprint) = state.registry.blueprint(type_id) else {
        return true;
    };
    if crate::trial_spawner::block(state, clicked, "spawner").is_some() {
        let configured = crate::monster_spawner::set_entity(state, clicked, type_id);
        if !configured {
            return true;
        }
        // SpawnEggItem.useOn shrinks directly for spawners, even in creative.
        // (Held-debit floors do not apply: vanilla shrinks the egg here, so a
        // direct shrink keeps survival/creative identical.)
        let mut remaining = stack_snapshot;
        remaining.count -= 1;
        if remaining.count == 0 {
            remaining = ItemStack::EMPTY;
        }
        let changes = state.players.get_mut(&actor).expect("egg player").replace_held(hand, remaining);
        if let Some(connection) = state.connections.entity_connection(actor) {
            for change in changes {
                state.push_player(connection, change);
            }
        }
        crate::vibration::emit(state, [f64::from(clicked.x) + 0.5, f64::from(clicked.y) + 0.5, f64::from(clicked.z) + 0.5],
            crate::vibration::GameEvent::BlockChange, Some(actor));
        return true;
    }
    if state.difficulty == crate::survival::Difficulty::Peaceful
        && blueprint.category == crate::blueprint::MobCategory::Monster
    {
        return true;
    }
    let (width, height, health) = (
        blueprint.half_width,
        blueprint.height,
        blueprint.params.max_health,
    );
    let Some(shapes) = state
        .world
        .block_at(clicked.x, clicked.y, clicked.z)
        .and_then(|id| state.registry.collision_boxes.get(id as usize))
    else {
        return true;
    };
    let target = if shapes.is_empty() {
        clicked
    } else {
        let [dx, dy, dz] = crate::placement::Face::from_code(face).offset();
        BlockPos {
            x: clicked.x + dx,
            y: clicked.y + dy,
            z: clicked.z + dz,
        }
    };
    let Some(pos) = position(state, target, target != clicked && face == 1, width) else {
        return true;
    };
    if !clear_body(state, pos, width, height) {
        return true;
    }
    if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
        return true;
    }
    let custom_name = stack.custom_name.as_deref().map(str::to_owned);
    let id = state.spawn_mob(type_id, pos, health);
    if let Some(name) = custom_name {
        let slot = state.entities.slot_of(id).expect("spawned egg mob");
        std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name =
            Some(name);
    }
    state.hostile.spawned.push(id);
    let effects = state
        .players
        .get_mut(&actor)
        .expect("egg player")
        .consume_held(hand, 1);
    if let Some(connection) = state.connections.entity_connection(actor) {
        for change in effects.changes {
            state.push_player(connection, change);
        }
    }
    crate::vibration::emit(
        state,
        pos,
        crate::vibration::GameEvent::EntityPlace,
        Some(actor),
    );
    true
}

#[cfg(test)]
#[path = "spawn_egg_tests.rs"]
mod tests;
