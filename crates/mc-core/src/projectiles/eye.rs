//! Eye guidance uses the same off-tick saved/generator truth as `/locate`.
//! Only completion on the native tick may create an entity or debit inventory.

use mc_data::ItemStack;
use mc_data::inventory_semantics_26_1_2::{can_stack, decrement_stack};
use mc_data::statistics::StatType;

use super::{ProjectileData, ProjectileKind, ProjectileState, weapons};
use crate::boundary::{ConnectionId, EntityId, Hand};
use crate::dimensions::Dimension;
use crate::tick::TickState;

#[derive(Debug)]
pub(crate) struct EyeUse {
    pub actor: EntityId,
    pub connection: ConnectionId,
    pub hand: Hand,
    pub origin: [f64; 3],
    pub dimension: Dimension,
    pub stack: ItemStack,
}

pub(super) fn use_item(state: &mut TickState, actor: EntityId, hand: Hand) {
    if state.dimension != Dimension::Overworld {
        return;
    }
    let Some(slot) = weapons::held_slot(state, actor, hand) else { return };
    let player = &state.players[&actor];
    if player.spectator() || player.vitals.is_dead() {
        return;
    }
    if !player.creative() && !weapons::costs_allowed(state, actor, &[(slot, 1)], slot, None) {
        return;
    }
    let Some((origin, _)) = weapons::geometry(state, actor, 0.0) else { return };
    let mut stack = player.inventory.slots[slot].clone();
    stack.count = 1;
    let request = EyeUse { actor, connection: player.connection, hand, origin, dimension: state.dimension, stack };
    crate::command_locate::submit_eye(state, request);
}

pub(crate) fn complete(state: &mut TickState, request: EyeUse, destination: (i32, i32, i32)) {
    if state.dimension != request.dimension
        || state.connections.entity_of(request.connection) != Some(request.actor)
    {
        return;
    }
    let Some(slot) = weapons::held_slot(state, request.actor, request.hand) else { return };
    let player = &state.players[&request.actor];
    if player.spectator() || player.vitals.is_dead()
        || !can_stack(&player.inventory.slots[slot], &request.stack)
        || player.inventory.slots[slot].is_empty()
        || (!player.creative() && !weapons::costs_allowed(state, request.actor, &[(slot, 1)], slot, None))
    {
        return;
    }
    let Some((origin, _)) = weapons::geometry(state, request.actor, 0.0) else { return };
    // A delayed search must not throw from a departed location or point at a
    // stale nearest result after the player teleports elsewhere.
    if origin.iter().zip(request.origin).any(|(now, before)| (now - before).abs() > 8.0) {
        return;
    }
    let owner_slot = state.entities.slot_of(request.actor).expect("live player");
    let owner = state.entities.row(owner_slot).uuid;
    let target = flight_target(origin, destination);
    let roll = crate::farming::hash(&[state.tick, request.actor.0 as u64, state.projectiles.spawned.len() as u64]);
    let mut record = ProjectileState::new(ProjectileKind::EyeOfEnder, Some(owner), request.stack.clone());
    record.data = ProjectileData::Eye { origin, target, drop_item: !roll.is_multiple_of(5) };
    if super::launch(state, record, origin, [0.0; 3]).is_none() {
        return;
    }
    let player = state.players.get_mut(&request.actor).expect("live player");
    player.statistics.increment(StatType::Used, request.stack.item_id, 1);
    if !player.creative() {
        decrement_stack(&mut player.inventory.slots[slot]);
    }
    weapons::sync(state, request.actor);
}

fn flight_target(origin: [f64; 3], destination: (i32, i32, i32)) -> [f64; 3] {
    let target = [f64::from(destination.0), f64::from(destination.1), f64::from(destination.2)];
    let dx = target[0] - origin[0];
    let dz = target[2] - origin[2];
    let distance = dx.hypot(dz);
    if distance > 12.0 {
        [origin[0] + dx * 12.0 / distance, origin[1] + 8.0, origin[2] + dz * 12.0 / distance]
    } else {
        target
    }
}

pub(super) fn step(state: &mut TickState, id: EntityId, record: &ProjectileState) -> bool {
    let Some(slot) = state.entities.slot_of(id) else { return true };
    let ProjectileData::Eye { origin, target, drop_item } = record.data else {
        // A vanilla imported/summoned eye has no retained destination.
        return record.age > 80;
    };
    if record.age > 80 {
        let position = state.entities.pos(slot);
        if drop_item {
            let item = crate::tick::spawn_item_entity(state, position, record.stack.clone());
            state.projectiles.spawned.push(item);
        } else {
            // Local 26.1.2 EyeOfEnder javap: levelEvent 2003, data 0.
            super::payloads::level_event(state, position, 2003, 0);
        }
        return true;
    }
    // Smooth two-second rise/travel followed by a two-second hover. This is
    // wiki-level guidance, not a copy of vanilla's oscillating integrator.
    let time = (f64::from(record.age) / 40.0).min(1.0);
    let progress = time * time * (3.0 - 2.0 * time);
    let position = std::array::from_fn(|axis| origin[axis] + (target[axis] - origin[axis]) * progress);
    let old = state.entities.pos(slot);
    super::set_flight(state, slot, std::array::from_fn(|axis| position[axis] - old[axis]));
    state.entities.set_pos(slot, position);
    false
}
