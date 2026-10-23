//! Connection-local digging admission shared by native input and build hooks.
use super::{TickState, within_block_reach};
use crate::boundary::{BlockPos, DigAction, EntityId};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DigProgress {
    position: BlockPos,
    block: u32,
    dimension: crate::dimensions::Dimension,
    progress: f32,
    tick: u64,
}

fn valid_target(state: &TickState, entity: EntityId, dig: DigProgress) -> bool {
    dig.dimension == state.dimension
        && state
            .world
            .block_at(dig.position.x, dig.position.y, dig.position.z)
            == Some(dig.block)
        && within_block_reach(state, entity, dig.position)
        && state
            .players
            .get(&entity)
            .is_some_and(|player| player.vitals.health > 0.0)
        && crate::plugin_player::allows_block(state, entity, dig.position)
        && !crate::plugin_inventories::has_reservations_for_block(state, dig.position)
}

/// Each server tick contributes once, using that tick's tool and effects. A
/// Finish packet (or a burst of Starts) cannot manufacture elapsed progress.
pub(super) fn advance(state: &mut TickState) {
    for slot in state.entities.player_slots() {
        let entity = state.entities.body(slot).id;
        let Some(mut dig) = state
            .players
            .get(&entity)
            .and_then(|player| player.dig_progress)
        else {
            continue;
        };
        let next = if valid_target(state, entity, dig) {
            let rate = crate::effects::mining_progress(state, entity, dig.block);
            if rate <= 0.0 || rate.is_nan() {
                None
            } else {
                if dig.tick < state.tick {
                    dig.progress = (dig.progress + rate).min(1.0);
                    dig.tick = state.tick;
                }
                Some(dig)
            }
        } else {
            None
        };
        state
            .players
            .get_mut(&entity)
            .expect("mining player")
            .dig_progress = next;
    }
}

pub(super) fn allows(
    state: &TickState,
    entity: EntityId,
    position: BlockPos,
    block: u32,
    action: DigAction,
) -> bool {
    let Some(player) = state.players.get(&entity) else {
        return false;
    };
    if action == DigAction::Cancel {
        return false;
    }
    if player.creative() {
        return true;
    }
    let rate = crate::effects::mining_progress(state, entity, block);
    // Zero hardness produces +infinity (instant), while missing/negative
    // hardness produces zero. Neither can be bypassed by stored progress.
    if rate <= 0.0 || rate.is_nan() {
        return false;
    }
    match action {
        DigAction::Start => rate >= 1.0,
        DigAction::Finish => player.dig_progress.is_some_and(|dig| {
            dig.position == position && dig.block == block && valid_target(state, entity, dig)
                // Vanilla accepts a finish at 70% to tolerate client prediction.
                && dig.progress >= 0.7
        }),
        DigAction::Cancel => false,
    }
}

pub(super) fn start(state: &mut TickState, entity: EntityId, position: BlockPos, block: u32) {
    let rate = crate::effects::mining_progress(state, entity, block);
    let next = (rate > 0.0).then_some(DigProgress {
        position,
        block,
        dimension: state.dimension,
        progress: rate.min(1.0),
        tick: state.tick,
    });
    if let Some(player) = state.players.get_mut(&entity) {
        player.dig_progress = next;
    }
}

pub(super) fn cancel(state: &mut TickState, entity: EntityId) {
    if let Some(player) = state.players.get_mut(&entity) {
        player.dig_progress = None;
    }
}
