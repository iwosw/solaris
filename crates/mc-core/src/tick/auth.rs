//! The native session fence runs before gameplay, menus and plugin observation.
use super::{TickState, apply_player_effects};
use crate::boundary::{ChangeSet, ConnectionId, PlayerInput};

pub(super) fn refuse_input(state: &mut TickState, input: &PlayerInput) -> bool {
    let connection = input.connection();
    if !state.session_locked(connection) {
        return false;
    }
    match input {
        // Connection lifecycle and presentation acknowledgements are not gameplay.
        PlayerInput::Join { .. }
        | PlayerInput::Leave { .. }
        | PlayerInput::PluginClientReady { .. }
        | PlayerInput::PermissionLevel { .. }
        | PlayerInput::SetViewDistance { .. }
        | PlayerInput::SetLocale { .. }
        | PlayerInput::ConfirmTeleport { .. }
        | PlayerInput::BuildResync { .. }
        | PlayerInput::BuildPreviewFence { .. } => return false,
        PlayerInput::Command {
            command,
            permission_level,
            ..
        } => {
            let root = command
                .trim_start()
                .trim_start_matches('/')
                .split_whitespace()
                .next();
            if matches!(root, Some("register" | "login" | "changepassword")) {
                // route_command itself requires a declared plugin root. Never fall
                // through to native commands, echo text or log credential arguments.
                crate::plugin::route_command(state, connection, command, *permission_level);
            }
        }
        // Fence only a real displacement, once per teleport. The client answers every
        // teleport with accept + its pose, so correcting each input (or an echoed
        // pose) is a ping-pong that floods the connection until a queue overflows.
        PlayerInput::Move { position, .. } | PlayerInput::MoveVehicle { position, .. } => {
            if let Some(entity) = state.connections.entity_of(connection)
                && state
                    .players
                    .get(&entity)
                    .is_some_and(|player| player.awaiting_teleport.is_none())
                && let Some(slot) = state.entities.slot_of_any(entity)
            {
                let home = state.entities.pos(slot);
                let drift: f64 = (0..3).map(|i| (position[i] - home[i]).powi(2)).sum();
                // NaN drift compares false; a non-finite pose is fenced like any other.
                if drift > 1e-6 || !drift.is_finite() {
                    let [yaw, pitch, _] = state.entities.rot(slot);
                    state.correct_position(connection, home, yaw, pitch);
                }
            }
        }
        // Rotation/status carry no position, so nothing can diverge.
        PlayerInput::MoveRotation { .. } | PlayerInput::MoveStatus { .. } => {}
        _ => crate::build_precommit::resync_input(state, input),
    }
    true
}

/// Release one live lock and finish deferred restored cursor/grid custody only
/// after authentication. Unauthenticated disconnects never create overflow drops.
pub(crate) fn unlock_session(state: &mut TickState, connection: ConnectionId) {
    if !state.require_plugin_login || !state.locked_sessions.remove(&connection) {
        return;
    }
    let Some(entity) = state.connections.entity_of(connection) else {
        return;
    };
    let effects = {
        let rules = state.registry.item_rules();
        state.players.get_mut(&entity).map(|player| {
            let effects = player.close(&rules);
            // Any restored larger crafting menu is now empty; window zero
            // returns to its ordinary two-by-two input grid.
            player.inventory_menu.grid.truncate(4);
            effects
        })
    };
    if let Some(effects) = effects {
        let mut changes = ChangeSet::default();
        apply_player_effects(state, connection, entity, effects, &mut changes);
        state.plugin_spawned.extend(changes.entities_spawned);
    }
}
