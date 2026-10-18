//! Server command source: permission level four, without a player entity.
use std::sync::{Arc, Mutex};

use crate::boundary::{ChangeSet, ConnectionId, EntityId, PlayerChange};
use crate::tick::TickState;

/// Reserved server source; edge connection allocation starts at one.
pub const CONNECTION: ConnectionId = ConnectionId(0);
/// Not an entity: vanilla self-targets deliberately resolve to no entities.
pub(crate) const EXECUTOR: EntityId = EntityId(-1);

/// One console line and an acknowledgement after its tick-owned execution.
#[derive(Debug, Clone)]
pub struct ConsoleCommand {
    command: String,
    completed: Arc<Mutex<Option<tokio::sync::oneshot::Sender<u64>>>>,
}

impl PartialEq for ConsoleCommand {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.completed, &other.completed)
    }
}

impl ConsoleCommand {
    pub fn new(command: String) -> (Self, tokio::sync::oneshot::Receiver<u64>) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        (
            Self {
                command,
                completed: Arc::new(Mutex::new(Some(sender))),
            },
            receiver,
        )
    }

    pub(crate) fn execute(&self, state: &mut TickState, changes: &mut ChangeSet) {
        execute(state, CONNECTION, &self.command, 4, changes);
        if let Ok(mut completed) = self.completed.lock()
            && let Some(sender) = completed.take()
        {
            let _ = sender.send(state.tick + 1);
        }
    }
}

/// Native command origin. The console uses spawn and zero rotation, never a
/// fabricated player, inventory, UUID, or connection registration.
pub(crate) fn pose(state: &TickState, executor: EntityId) -> Option<([f64; 3], [f32; 2])> {
    if executor == EXECUTOR {
        return Some((state.world.world_spawn_position(), [0.0; 2]));
    }
    let slot = state.entities.slot_of_any(executor)?;
    let [yaw, pitch, _] = state.entities.rot(slot);
    Some((state.entities.pos(slot), [yaw, pitch]))
}

/// Shared player/console path, including plugin ownership, native parsing and
/// dimension-coordinated commands. Only the trusted console input grants a
/// source without an entity.
pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    command: &str,
    permission_level: u8,
    changes: &mut ChangeSet,
) {
    let entity = if connection == CONNECTION {
        EXECUTOR
    } else {
        let Some(entity) = state.connections.entity_of(connection) else {
            return;
        };
        if state.entities.slot_of(entity).is_none() {
            return;
        }
        entity
    };
    state
        .plugin_players
        .observe_permission(connection, permission_level);
    if crate::plugin::route_command(state, connection, command, permission_level) {
        return;
    }
    if let Some(parsed) = crate::dimensions::travel::parse_execution(command, permission_level >= 2)
    {
        match parsed {
            Ok((dimension, command)) => {
                state
                    .dimension_requests
                    .push(crate::dimensions::travel::Request {
                        connection,
                        executor: entity,
                        dimension,
                        command,
                    })
            }
            Err(error) => state.push_player(
                connection,
                PlayerChange::Chat {
                    message: error.message(),
                },
            ),
        }
        return;
    }
    let command = match crate::commands::parse(command, permission_level >= 2) {
        Ok(command) => command,
        Err(error) => {
            state.push_player(
                connection,
                PlayerChange::Chat {
                    message: error.message(),
                },
            );
            return;
        }
    };
    if matches!(command, crate::commands::Command::Teleport { .. })
        || (state.dimension_globals.active && crate::dimensions::commands::is_global(&command))
    {
        state
            .dimension_requests
            .push(crate::dimensions::travel::Request {
                connection,
                executor: entity,
                dimension: state.dimension,
                command,
            });
        return;
    }
    crate::command_execution::execute(state, connection, entity, &command, changes);
}

#[cfg(test)]
#[path = "command_console_tests.rs"]
mod tests;
