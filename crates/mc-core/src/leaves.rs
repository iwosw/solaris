//! Natural leaf distance is a persisted block tick; decay uses tick-local loot.

use mc_world::BlockStateId;

use crate::boundary::BlockPos;
use crate::tick::TickState;

pub(crate) fn schedule(state: &mut TickState, at: BlockPos, id: u32) {
    let Some(block) = state.registry.blocks.by_id(BlockStateId(id)) else {
        return;
    };
    if !block.block.id.path().ends_with("_leaves") {
        return;
    }
    let name = block.block.id.clone();
    if !state.block_ticks.contains(crate::plants::old(at), &name, 0) {
        state
            .block_ticks
            .schedule(mc_world::ScheduledBlockTick::new(
                crate::plants::old(at),
                name,
                state.tick.saturating_add(1),
                0,
            ));
    }
}

/// `None` means an unloaded neighbour, never unsupported foliage.
pub(crate) fn distance(state: &TickState, at: BlockPos) -> Option<u8> {
    let mut distance = 7;
    for pos in crate::plants::neighbours(at) {
        let geometry = state.dimension.geometry();
        if !(geometry.min_y()..geometry.max_y()).contains(&pos.y) {
            continue;
        }
        let id = state.world.block_at(pos.x, pos.y, pos.z)?;
        distance = distance.min(
            mc_world::plant_rules_26_1_2::leaf_distance_from_state(
                &state.registry.blocks,
                BlockStateId(id),
            )
            .saturating_add(1),
        );
    }
    Some(distance.min(7))
}

pub(crate) fn scheduled(
    state: &mut TickState,
    at: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
) -> bool {
    let Some(id) = state.world.block_at(at.x, at.y, at.z) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(BlockStateId(id)) else {
        return false;
    };
    if !block.block.id.path().ends_with("_leaves") {
        return false;
    }
    let Some(distance) = distance(state, at) else {
        schedule(state, at, id);
        return true;
    };
    if crate::farming::prop(&state.registry.blocks, id, "distance")
        .and_then(|value| value.parse::<u8>().ok())
        == Some(distance)
    {
        return true;
    }
    if let Some(new) = crate::farming::with_prop(
        &state.registry.blocks,
        id,
        "distance",
        &distance.to_string(),
    ) {
        state.world.set_block(at.x, at.y, at.z, new);
        writes.retain(|(pos, _)| *pos != at);
        writes.push((at, new));
        for pos in crate::plants::neighbours(at) {
            if let Some(id) = state.world.block_at(pos.x, pos.y, pos.z) {
                schedule(state, pos, id);
            }
        }
    }
    true
}

pub(crate) fn decays(state: &TickState, at: BlockPos, id: u32) -> bool {
    crate::farming::prop(&state.registry.blocks, id, "persistent") == Some("false")
        && crate::farming::prop(&state.registry.blocks, id, "distance") == Some("7")
        // Guard against stale distance when a supporting log just appeared.
        && distance(state, at) == Some(7)
}
