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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DelayedDig {
    progress: DigProgress,
    finish: Option<PendingFinish>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PendingFinish {
    Native { sequence: i32 },
    Authorized(crate::build_precommit::AuthorizedBuild),
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

/// Credit Start's first tick immediately, then each subsequent server tick once.
fn advanced(state: &TickState, entity: EntityId, mut dig: DigProgress) -> Option<DigProgress> {
    if !valid_target(state, entity, dig) {
        return None;
    }
    let rate = crate::effects::mining_progress(state, entity, dig.block);
    if rate <= 0.0 || rate.is_nan() {
        return None;
    }
    if dig.tick < state.tick {
        dig.progress = (dig.progress + rate).min(1.0);
        dig.tick = state.tick;
        tracing::trace!(
            target: "solaris::mining", phase = "tick", tick = state.tick,
            actor = ?entity, position = ?dig.position, block = dig.block,
            progress = dig.progress, rate,
            on_ground = state.entities.slot_of(entity).is_some_and(|slot| state.entities.ground(slot)),
        );
    }
    Some(dig)
}

pub(super) fn advance(state: &mut TickState) {
    for slot in state.entities.player_slots() {
        let entity = state.entities.body(slot).id;
        let Some(player) = state.players.get(&entity) else {
            continue;
        };
        let current = player
            .dig_progress
            .and_then(|dig| advanced(state, entity, dig));
        let delayed = player
            .delayed_dig
            .as_ref()
            .and_then(|dig| advanced(state, entity, dig.progress));
        let player = state.players.get_mut(&entity).expect("mining player");
        player.dig_progress = current;
        if let Some(progress) = delayed {
            player
                .delayed_dig
                .as_mut()
                .expect("delayed mining")
                .progress = progress;
        } else {
            player.delayed_dig = None;
        }
    }
}

/// A separate, single delayed slot survives Start on another block. Unlike an
/// immediate Finish's 70% tolerance, vanilla delayed destruction waits for 100%.
pub(super) fn take_finished(state: &mut TickState, inputs: &mut Vec<crate::boundary::PlayerInput>) {
    for slot in state.entities.player_slots() {
        let entity = state.entities.body(slot).id;
        let Some(connection) = state.connections.entity_connection(entity) else {
            continue;
        };
        let Some(delayed) = state
            .players
            .get_mut(&entity)
            .and_then(|player| player.delayed_dig.as_mut())
        else {
            continue;
        };
        if delayed.progress.progress < 1.0 {
            continue;
        }
        let Some(finish) = delayed.finish.take() else {
            continue;
        };
        inputs.push(match finish {
            PendingFinish::Native { sequence } => crate::boundary::PlayerInput::Dig {
                connection,
                position: delayed.progress.position,
                action: DigAction::Finish,
                sequence,
            },
            PendingFinish::Authorized(authorization) => {
                crate::boundary::PlayerInput::AuthorizedBuild { authorization }
            }
        });
    }
}

fn matching(
    state: &TickState,
    entity: EntityId,
    position: BlockPos,
    block: u32,
) -> Option<DigProgress> {
    let player = state.players.get(&entity)?;
    [
        player.dig_progress,
        player.delayed_dig.as_ref().map(|dig| dig.progress),
    ]
    .into_iter()
    .flatten()
    .filter(|dig| {
        dig.position == position && dig.block == block && valid_target(state, entity, *dig)
    })
    .max_by(|a, b| a.progress.total_cmp(&b.progress))
}

pub(crate) fn can_finish(
    state: &TickState,
    entity: EntityId,
    position: BlockPos,
    block: u32,
) -> bool {
    crate::effects::mining_progress(state, entity, block) > 0.0
        && matching(state, entity, position, block).is_some()
}

pub(crate) fn delay(
    state: &mut TickState,
    entity: EntityId,
    position: BlockPos,
    block: u32,
    finish: impl FnOnce() -> PendingFinish,
) -> bool {
    if !can_finish(state, entity, position, block)
        || allows(state, entity, position, block, DigAction::Finish)
    {
        return false;
    }
    let progress = matching(state, entity, position, block).expect("valid pending finish");
    let player = state.players.get_mut(&entity).expect("mining player");
    if player.delayed_dig.is_none() {
        player.delayed_dig = Some(DelayedDig {
            progress,
            finish: Some(finish()),
        });
        player.dig_progress = None;
    }
    true
}

pub(super) fn consume(state: &mut TickState, entity: EntityId, position: BlockPos, block: u32) {
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    let matches = |dig: DigProgress| {
        dig.position == position && dig.block == block && dig.dimension == state.dimension
    };
    if player.dig_progress.is_some_and(matches) {
        player.dig_progress = None;
    }
    if player
        .delayed_dig
        .as_ref()
        .is_some_and(|dig| matches(dig.progress))
    {
        player.delayed_dig = None;
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
        DigAction::Finish => matching(state, entity, position, block)
            // Vanilla accepts an immediate finish at 70% for client prediction.
            .is_some_and(|dig| dig.progress >= 0.7),
        DigAction::Cancel => false,
    }
}

pub(super) fn trace_finish(state: &TickState, entity: EntityId, position: BlockPos, block: u32) {
    tracing::trace!(
        target: "solaris::mining", phase = "finish", tick = state.tick,
        actor = ?entity, ?position, block,
        progress = ?matching(state, entity, position, block).map(|dig| dig.progress),
        rate = crate::effects::mining_progress(state, entity, block),
        on_ground = state.entities.slot_of(entity).is_some_and(|slot| state.entities.ground(slot)),
    );
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
    tracing::trace!(
        target: "solaris::mining", phase = "start", tick = state.tick,
        actor = ?entity, ?position, block, progress = rate.min(1.0), rate,
        on_ground = state.entities.slot_of(entity).is_some_and(|slot| state.entities.ground(slot)),
    );
    if let Some(player) = state.players.get_mut(&entity) {
        player.dig_progress = next;
    }
}

pub(super) fn cancel(state: &mut TickState, entity: EntityId) {
    if let Some(player) = state.players.get_mut(&entity) {
        player.dig_progress = None;
    }
}
