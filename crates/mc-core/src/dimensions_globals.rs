//! Ephemeral global player roster and reliable broadcasts across local worlds.
use super::Dimension;
use crate::boundary::{ConnectionId, EntityId, PlayerChange, TickOutput};
use crate::commands::Weather;
use crate::tick::TickState;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterPlayer {
    pub connection: ConnectionId,
    pub entity: EntityId,
    pub uuid: uuid::Uuid,
    pub name: String,
    pub game_mode: u8,
}

#[derive(Debug)]
pub(crate) enum Broadcast {
    Chat(mc_nbt::Tag),
    TeamChat {
        members: std::collections::BTreeSet<String>,
        content: mc_nbt::Tag,
    },
    Scoreboard(crate::scoreboard::ScoreboardEvent),
    BossBar {
        players: Vec<uuid::Uuid>,
        id: uuid::Uuid,
        change: crate::boundary::BossBarChange,
    },
    Difficulty(u8),
    TickingState {
        rate: f32,
        frozen: bool,
    },
    TickingStep {
        steps: i32,
    },
    Weather(Weather, Option<i64>),
}

/// Connections are not durable gameplay state; rebuilding this cache on join
/// does not create another session owner or another persistent authority.
#[derive(Debug, Default)]
pub(crate) struct Globals {
    pub active: bool,
    pub roster: Arc<[RosterPlayer]>,
    /// Tick-local UUID custody, moved between worlds with the shared authorities.
    pub reserved_uuids: std::collections::HashSet<uuid::Uuid>,
    pub broadcasts: Vec<Broadcast>,
}

fn game_mode(world: &TickState, entity: EntityId) -> Option<u8> {
    world
        .players
        .get(&entity)
        .map(|player| player.game_mode.id())
}

fn queue_each(world: &mut TickState, mut change: impl FnMut() -> PlayerChange) {
    let connections = std::mem::take(&mut world.connections);
    for connection in connections.all_connections() {
        world.push_player(connection, change());
    }
    world.connections = connections;
}

fn relay(world: &mut TickState, event: &Broadcast) {
    match event {
        Broadcast::Chat(content) => queue_each(world, || PlayerChange::StyledChat {
            content: content.clone(),
        }),
        Broadcast::TeamChat { members, content } => {
            let recipients: Vec<_> = world
                .connections
                .all_connections()
                .filter(|connection| !world.session_locked(*connection))
                .filter(|connection| {
                    world
                        .connections
                        .entity_of(*connection)
                        .and_then(|entity| world.connections.name_of(entity))
                        .is_some_and(|name| members.contains(name))
                })
                .collect();
            for connection in recipients {
                world.push_player(
                    connection,
                    PlayerChange::StyledChat {
                        content: content.clone(),
                    },
                );
            }
        }
        Broadcast::Scoreboard(event) => queue_each(world, || PlayerChange::Scoreboard {
            event: event.clone(),
        }),
        Broadcast::BossBar {
            players,
            id,
            change,
        } => {
            for player in players {
                let connection = world
                    .connections
                    .all_connections()
                    .find(|connection| world.connections.uuid_of(*connection) == Some(*player));
                if let Some(connection) = connection {
                    world.push_player(
                        connection,
                        PlayerChange::BossBar {
                            id: *id,
                            change: change.clone(),
                        },
                    );
                }
            }
        }
        Broadcast::Difficulty(difficulty) => {
            queue_each(world, || PlayerChange::DifficultyChanged {
                difficulty: *difficulty,
            })
        }
        Broadcast::TickingState { rate, frozen } => {
            queue_each(world, || PlayerChange::TickingState {
                rate: *rate,
                frozen: *frozen,
            })
        }
        Broadcast::TickingStep { steps } => {
            queue_each(world, || PlayerChange::TickingStep { steps: *steps })
        }
        Broadcast::Weather(..) => unreachable!("weather belongs to the overworld"),
    }
}

fn broadcasts(state: &mut TickState, source: Dimension, pending: &mut Vec<Broadcast>) {
    for event in pending.drain(..) {
        if let Broadcast::Weather(weather, duration) = event {
            crate::weather::set_command(state, weather, duration);
            let raining = weather != Weather::Clear;
            let rain_level = state.rain_level;
            let thunder_level = state.thunder_level;
            queue_each(state, || PlayerChange::WeatherChanged {
                raining,
                rain_level,
                thunder_level,
            });
            continue;
        }
        if state.dimension != source {
            relay(state, &event);
        }
        for (&dimension, world) in &mut state.other_dimensions {
            if dimension != source {
                relay(world, &event);
            }
        }
    }
}

pub(super) fn roster(state: &mut TickState) {
    let worlds =
        || std::iter::once(&*state).chain(state.other_dimensions.values().map(|world| &**world));
    let count: usize = worlds()
        .map(|world| {
            world
                .connections
                .all_connections()
                .filter(|connection| !world.session_locked(*connection))
                .count()
        })
        .sum();
    let changed = count != state.dimension_globals.roster.len()
        || state.dimension_globals.roster.iter().any(|player| {
            !worlds().any(|world| {
                world.connections.entity_of(player.connection) == Some(player.entity)
                    && !world.session_locked(player.connection)
                    && world.connections.uuid_of(player.connection) == Some(player.uuid)
                    && world.connections.name_of(player.entity) == Some(player.name.as_str())
                    && game_mode(world, player.entity) == Some(player.game_mode)
            })
        });
    if !changed {
        return;
    }
    let mut players = Vec::with_capacity(count);
    for world in worlds() {
        for connection in world.connections.all_connections() {
            if world.session_locked(connection) {
                continue;
            }
            let entity = world
                .connections
                .entity_of(connection)
                .expect("joined entity");
            players.push(RosterPlayer {
                connection,
                entity,
                uuid: world.connections.uuid_of(connection).expect("joined UUID"),
                name: world
                    .connections
                    .name_of(entity)
                    .expect("joined name")
                    .to_owned(),
                game_mode: game_mode(world, entity).expect("joined player"),
            });
        }
    }
    players.sort_unstable_by_key(|player| player.connection);
    let removed: Arc<[uuid::Uuid]> = state
        .dimension_globals
        .roster
        .iter()
        .filter(|old| !players.iter().any(|player| player.uuid == old.uuid))
        .map(|player| player.uuid)
        .collect();
    let players: Arc<[RosterPlayer]> = players.into();
    state.dimension_globals.roster = Arc::clone(&players);
    queue_each(state, || PlayerChange::Roster {
        players: Arc::clone(&players),
        removed: Arc::clone(&removed),
    });
    for world in state.other_dimensions.values_mut() {
        world.dimension_globals.roster = Arc::clone(&players);
        queue_each(world, || PlayerChange::Roster {
            players: Arc::clone(&players),
            removed: Arc::clone(&removed),
        });
    }
}

pub(crate) fn finish(
    state: &mut TickState,
    output: &mut TickOutput,
    drains: &mut Vec<(ConnectionId, Vec<PlayerChange>)>,
) {
    let mut pending = std::mem::take(&mut state.dimension_globals.broadcasts);
    broadcasts(state, state.dimension, &mut pending);
    state.dimension_globals.broadcasts = pending;
    for dimension in Dimension::ALL {
        let Some(world) = state.other_dimensions.get_mut(&dimension) else {
            continue;
        };
        let mut pending = std::mem::take(&mut world.dimension_globals.broadcasts);
        broadcasts(state, dimension, &mut pending);
        state
            .other_dimensions
            .get_mut(&dimension)
            .expect("broadcast world")
            .dimension_globals
            .broadcasts = pending;
    }
    roster(state);
    let snapshot = Arc::get_mut(&mut output.snapshot).expect("unpublished root snapshot");
    snapshot.raining = state.weather.0 != Weather::Clear;
    snapshot.rain_level = state.rain_level;
    snapshot.thunder_level = state.thunder_level;
    drain(state, drains);
    for world in state.other_dimensions.values_mut() {
        drain(world, drains);
    }
}

fn drain(world: &mut TickState, drains: &mut Vec<(ConnectionId, Vec<PlayerChange>)>) {
    for (connection, queue) in std::mem::take(&mut world.outbox) {
        if !queue.is_empty() {
            drains.push((connection, queue));
        }
    }
}
