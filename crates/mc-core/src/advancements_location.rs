//! Fresh periodic location evidence with startup criterion dispatch.
use super::*;

pub(super) fn periodic(state: &mut TickState, entity: EntityId, selected: &mut Vec<usize>) {
    let tree = state.registry.advancements.clone();
    if tree.trigger_index.is_empty() {
        trigger(state, entity, "minecraft:location", json!({}));
        return;
    }
    let Some(criteria) = tree.trigger_index.get("minecraft:location") else {
        return;
    };
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return;
    };
    let position = state.entities.pos(slot);
    let [x, _, z] = position.map(|axis| axis.floor() as i32);
    let stepping_pos = crate::boundary::BlockPos {
        x,
        y: (position[1] - 1.0e-5).floor() as i32,
        z,
    };
    // Always re-read live location and blocks, even for a stationary player.
    // Only block-id metadata inside these snapshots is immutable and cached.
    let location = location_snapshot(state, entity);
    let stepping_on = block_location(state, stepping_pos, None);
    let mut evidence =
        json!({"location":location,"player":{"location":location,"stepping_on":stepping_on}});
    tree.candidate_indices(
        &EventContext {
            trigger: "minecraft:location",
            event: &evidence,
            inventory: &[],
        },
        selected,
    );
    let player = &state.players[&entity];
    let mut pending = false;
    let mut full_context = false;
    for &index in selected.iter() {
        let (id, name) = &criteria[index];
        if player
            .advancement_progress
            .get(id)
            .and_then(|entry| entry.get("criteria"))
            .and_then(|criteria| criteria.get(name))
            .is_none()
        {
            pending = true;
            full_context |= tree.location_needs_full_context(index);
        }
    }
    if !pending {
        return;
    }
    if full_context {
        // Equipment, effects, flags, passengers and other mutable facts are
        // captured live only for candidates that actually constrain them.
        evidence.as_object_mut().unwrap().remove("player");
    }
    trigger(state, entity, "minecraft:location", evidence);
}
