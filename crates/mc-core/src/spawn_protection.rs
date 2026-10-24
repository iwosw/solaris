//! Vanilla Overworld spawn protection; the edge supplies the persisted op list.
use crate::boundary::{BlockPos, EntityId};
use crate::tick::TickState;

pub(crate) fn protected(state: &TickState, actor: EntityId, pos: BlockPos) -> bool {
    if state.dimension != crate::dimensions::Dimension::Overworld
        || state.spawn_protection == 0 || state.spawn_operators.is_empty() {
        return false;
    }
    if state.entities.slot_of(actor).is_some_and(|slot| state.spawn_operators.contains(&state.entities.row(slot).uuid)) {
        return false;
    }
    let [x,_,z] = state.world.world_spawn();
    i64::from(pos.x).abs_diff(i64::from(x)).max(i64::from(pos.z).abs_diff(i64::from(z))) <= u64::from(state.spawn_protection)
}
