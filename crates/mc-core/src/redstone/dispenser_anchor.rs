//! Dispenser respawn-anchor charging: glowstone into a non-full anchor.

use crate::boundary::BlockPos;
use crate::tick::TickState;

/// Charge the anchor at `target` from dispenser slot `index`. Returns true
/// when the item was a glowstone consumed by a charge (or refused because the
/// anchor is already full); false when this behavior does not apply.
pub(super) fn charge(
    state: &mut TickState,
    pos: BlockPos,
    target: BlockPos,
    index: usize,
    writes: &mut Vec<(BlockPos, u32)>,
) -> bool {
    let Some(current) = state.world.block_at(target.x, target.y, target.z) else {
        return false;
    };
    let Some(charges) = crate::dimensions::rules::anchor_charge(state, current) else {
        return false;
    };
    if charges >= 4 {
        return false;
    }
    let Some(next) = crate::dimensions::rules::anchor_state(state, current, charges + 1) else {
        return false;
    };
    state.world.set_block(target.x, target.y, target.z, next);
    writes.push((target, next));
    let key = (pos.x, pos.y, pos.z);
    mc_data::inventory_semantics_26_1_2::decrement_stack(
        &mut state
            .block_entities
            .get_mut(&key)
            .expect("anchor-charge custody")
            .slots[index],
    );
    true
}
