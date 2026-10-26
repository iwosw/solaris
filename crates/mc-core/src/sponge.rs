//! Placement/neighbor-driven sponge absorption (26.1.2 SpongeBlock).
use crate::boundary::{BlockPos, EntityId};
use crate::tick::TickState;
use mc_data::Identifier;
use mc_world::BlockStateId;

const STEPS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];
fn offset(pos: BlockPos, step: [i32; 3]) -> BlockPos {
    BlockPos {
        x: pos.x + step[0],
        y: pos.y + step[1],
        z: pos.z + step[2],
    }
}
pub(crate) fn after_blocks(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    if writes.is_empty() {
        return;
    }
    let Some(dry) = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:sponge").unwrap())
        .map(|b| b.default.0)
    else {
        return;
    };
    let wet = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:wet_sponge").unwrap())
        .expect("wet sponge")
        .default
        .0;
    // Include appended removals: another dry sponge touching the drained pool
    // gets its ordinary neighbor notification, without scanning loaded chunks.
    let mut index = 0;
    while index < writes.len() {
        let pos = writes[index].0;
        index += 1;
        for step in [[0, 0, 0]].into_iter().chain(STEPS) {
            let cell = offset(pos, step);
            if state.world.block_at(cell.x, cell.y, cell.z) == Some(dry)
                && absorb(state, cell, writes, spawned)
            {
                state.world.set_block(cell.x, cell.y, cell.z, wet);
                writes.push((cell, wet));
            }
        }
    }
}
fn absorb(
    state: &mut TickState,
    origin: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) -> bool {
    // Vanilla breadthFirstTraversal includes the sponge in its 65 accepted
    // nodes: at most 64 water cells and Manhattan distance at most six.
    let mut queue = [(origin, 0_u8); 65];
    let mut read = 0;
    let mut end = 1;
    while read < end && end < queue.len() {
        let (pos, depth) = queue[read];
        read += 1;
        if depth == 6 {
            continue;
        }
        for step in STEPS {
            if end == queue.len() {
                break;
            }
            let next = offset(pos, step);
            let Some(current) = state.world.block_at(next.x, next.y, next.z) else {
                continue;
            };
            let Some(block) = state.registry.blocks.by_id(BlockStateId(current)) else {
                continue;
            };
            let plant = matches!(
                block.block.id.path(),
                "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass"
            );
            let replacement = if plant
                || matches!(
                    crate::fluids::fluid_at(&state.registry.states, current),
                    Some((crate::fluids::Fluid::Water, _))
                ) {
                state.registry.states.air
            } else if let Some(dry) =
                crate::fluids::waterlogged(state, current, false).filter(|&dry| dry != current)
            {
                dry
            } else {
                continue;
            };
            if crate::plugin_player::protects_cell(state, next) {
                continue;
            }
            if plant && !state.pop_block_loot(next, current, spawned) {
                continue;
            }
            state.world.set_block(next.x, next.y, next.z, replacement);
            writes.push((next, replacement));
            queue[end] = (next, depth + 1);
            end += 1;
        }
    }
    end > 1
}
