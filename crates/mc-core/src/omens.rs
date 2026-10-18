//! Trial omens; village/raid admission remains in the raid owner.
use crate::boundary::EntityId;
use crate::effects::{ActiveEffect, EffectId};
use crate::tick::TickState;

/// Conversion is vanilla's 18,000 ticks per Bad Omen level. Consumption occurs
/// when the spawner accepts the player, never merely on range detection.
pub(crate) fn activate_trial(state: &mut TickState, entity: EntityId) -> bool {
    let Some(player) = state.players.get_mut(&entity) else {
        return false;
    };
    let mut updates = Vec::new();
    if let Some(bad) = player.active_effects.get(EffectId::BadOmen) {
        let duration = 18_000 * (i32::from(bad.amplifier) + 1);
        updates.extend(player.clear_effects(Some(EffectId::BadOmen)));
        updates.extend(player.give_effect(ActiveEffect::new(EffectId::TrialOmen, duration, 0)));
    }
    let ominous = player.active_effects.has(EffectId::TrialOmen);
    let connection = player.connection;
    for update in updates {
        state.push_player(connection, update);
    }
    if ominous {
        crate::effects::sync_appearance(state, entity);
    }
    ominous
}
pub(crate) fn consume_trial(state: &mut TickState, entity: EntityId) {
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    let connection = player.connection;
    let updates = player.clear_effects(Some(EffectId::TrialOmen));
    for update in updates {
        state.push_player(connection, update);
    }
    crate::effects::sync_appearance(state, entity);
}
