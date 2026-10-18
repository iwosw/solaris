//! Client-driven elytra motion; the tick owns admission, pose and equipment wear.
use super::{TickState, update_player_crouching};
use crate::boundary::{ConnectionId, EntityId};
use crate::effects::EffectId;
use crate::entities::EntitySlot;
use std::sync::Arc;

const CHEST_SLOT: usize = 38;
const FALL_FLYING: u8 = 0x80;

fn usable(state: &TickState, entity: EntityId) -> bool {
    let Some(player) = state.players.get(&entity) else {
        return false;
    };
    let stack = &player.inventory.slots[CHEST_SLOT];
    let rules = state.registry.item_rules();
    !stack.is_empty()
        && rules
            .items
            .name_of(stack.item_id)
            .is_some_and(|name| name.as_str() == "minecraft:elytra")
        && rules
            .facts
            .facts_for_stack(stack, rules.items)
            .and_then(|facts| facts.max_damage)
            .is_some_and(|max| i64::from(max) - i64::from(stack.damage.unwrap_or(0)) > 1)
}

fn in_fluid(state: &TickState, slot: EntitySlot) -> bool {
    let [x, y, z] = state.entities.pos(slot);
    let row = state.entities.body(slot);
    // Check the body, not only its feet: a glider can enter water sideways.
    for bx in
        ((x - row.half_width + 1e-7).floor() as i32)..=((x + row.half_width - 1e-7).floor() as i32)
    {
        for by in ((y + 1e-7).floor() as i32)..=((y + row.height - 1e-7).floor() as i32) {
            for bz in ((z - row.half_width + 1e-7).floor() as i32)
                ..=((z + row.half_width - 1e-7).floor() as i32)
            {
                if state
                    .world
                    .block_at(bx, by, bz)
                    .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                    .is_some_and(|block| {
                        matches!(
                            block.block.id.path(),
                            "water"
                                | "lava"
                                | "bubble_column"
                                | "kelp"
                                | "kelp_plant"
                                | "seagrass"
                                | "tall_seagrass"
                        ) || block
                            .properties
                            .iter()
                            .any(|(key, value)| key == "waterlogged" && value == "true")
                    })
                {
                    return true;
                }
            }
        }
    }
    false
}

fn can_continue(state: &TickState, entity: EntityId, slot: EntitySlot) -> bool {
    state.entities.body(slot).alive
        && !state.entities.ground(slot)
        && state
            .players
            .get(&entity)
            .is_some_and(|p| !p.vitals.is_dead() && !p.spectator())
        && crate::mounts::vehicle(state, entity).is_none()
        && usable(state, entity)
        && !in_fluid(state, slot)
}

fn set(state: &mut TickState, entity: EntityId, slot: EntitySlot, flying: bool) {
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    player.fall_flying = flying;
    player.fall_flying_ticks = 0;
    let (body, row) = state.entities.parts_mut(slot);
    if flying || row.appearance.pose == 1 {
        body.height = if flying { 0.6 } else { 1.8 };
    }
    let appearance = Arc::make_mut(&mut row.appearance);
    if flying {
        appearance.shared_flags |= FALL_FLYING;
        appearance.pose = 1;
    } else {
        appearance.shared_flags &= !FALL_FLYING;
        if appearance.pose == 1 {
            appearance.pose = 0;
        }
    }
    let sneaking = appearance.shared_flags & super::SNEAKING_FLAG != 0;
    // Even a refused start publishes the authoritative false flag to self.
    state.appearance_changed.push(entity);
    if !flying {
        update_player_crouching(state, entity, slot, sneaking);
    }
}

pub(super) fn start(state: &mut TickState, connection: ConnectionId) {
    let Some(entity) = state.connections.entity_of(connection) else {
        return;
    };
    let Some(slot) = state.entities.slot_of(entity) else {
        return;
    };
    let Some(player) = state.players.get(&entity) else {
        return;
    };
    if player.fall_flying {
        refresh(state, entity);
        return;
    }
    let accepted = player.awaiting_teleport.is_none()
        && !player.active_effects.has(EffectId::Levitation)
        && !state.sleepers.contains_key(&entity)
        && can_continue(state, entity, slot);
    set(state, entity, slot, accepted);
}

pub(super) fn refresh(state: &mut TickState, entity: EntityId) {
    let Some(slot) = state.entities.slot_of(entity) else {
        return;
    };
    if state
        .players
        .get(&entity)
        .is_some_and(|player| player.fall_flying)
    {
        if !can_continue(state, entity, slot) {
            set(state, entity, slot, false);
        } else if state.entities.row(slot).appearance.shared_flags & FALL_FLYING == 0 {
            // Rejoin restores vanilla FallFlying, but entity presentation is fresh.
            set(state, entity, slot, true);
        }
    } else if state.entities.row(slot).appearance.shared_flags & FALL_FLYING != 0 {
        // Respawn resets player state before this tick's presentation refresh.
        set(state, entity, slot, false);
    }
}

pub(super) fn land(state: &mut TickState, entity: EntityId) {
    if state
        .players
        .get(&entity)
        .is_some_and(|player| player.fall_flying)
        && let Some(slot) = state.entities.slot_of(entity)
    {
        set(state, entity, slot, false);
    }
}

pub(super) fn tick(state: &mut TickState, entity: EntityId) {
    refresh(state, entity);
    let Some(player) = state
        .players
        .get_mut(&entity)
        .filter(|player| player.fall_flying)
    else {
        return;
    };
    player.fall_flying_ticks = player.fall_flying_ticks.wrapping_add(1);
    if !player.fall_flying_ticks.is_multiple_of(20) {
        return;
    }
    // Same Unbreaking, inventory synchronization and advancement hook as weapons.
    // A usable elytra has at least two durability, so wear cannot destroy it.
    crate::projectiles::weapons::wear(state, entity, CHEST_SLOT, 1);
    crate::projectiles::weapons::sync(state, entity);
    refresh(state, entity);
}
