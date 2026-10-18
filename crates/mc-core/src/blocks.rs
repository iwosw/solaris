//! Settled neighbor updates and native falling entities on the tick owner.
use crate::boundary::{BlockPos, EntityId};
use crate::tick::TickState;

pub(crate) fn run(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
    removed: &mut Vec<EntityId>,
) {
    crate::falling::run(state, writes, spawned, removed);
    // Stair transitions also react to breaks, fluid wash and piston edits.
    // shape_updates stages its changes until all required reads succeed.
    if crate::placement::shape_updates(&state.registry.blocks, writes, &|pos| {
        state.world.block_at(pos.x, pos.y, pos.z)
    })
    .is_some()
    {
        for (pos, block) in writes.iter() {
            state.world.set_block(pos.x, pos.y, pos.z, *block);
        }
    }
    let settled: Vec<_> = writes.iter().map(|(pos, _)| *pos).collect();
    for pos in settled {
        crate::plants::neighbor_updates(state, pos, writes, spawned);
    }
    crate::tick::pop_unsupported(state, writes, spawned);
    crate::falling::start(state, writes, spawned);
    crate::signs::invalidate(state, writes);
}
