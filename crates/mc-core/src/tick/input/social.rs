use crate::boundary::{BlockPos, ConnectionId, PlayerChange};
use crate::tick::TickState;

/// Vanilla's TickThrottler(20, 200), checked before fan-out rather than after it.
/// The counter travels with the player across respawns/dimensions, but not rejoins.
pub(in crate::tick) fn admit_communication(
    state: &mut TickState,
    connection: ConnectionId,
) -> bool {
    if state
        .disconnects
        .iter()
        .any(|(sealed, _)| *sealed == connection)
    {
        return false;
    }
    let Some(entity) = state.connections.entity_of(connection) else {
        return false;
    };
    let Some(player) = state.players.get_mut(&entity) else {
        return false;
    };
    player.chat_spam_ticks += 20;
    if player.chat_spam_ticks < 200 {
        return true;
    }
    // Seal exactly once, including when the sender's queue is already full.
    // Later queued communication must not broadcast while Leave is pending.
    state
        .outbox
        .entry(connection)
        .or_default()
        .push(PlayerChange::Disconnect {
            reason: "disconnect.spam".to_owned(),
        });
    state.disconnects.push((connection, state.tick));
    false
}

pub(in crate::tick) fn seen_advancements(
    state: &mut TickState,
    (connection, tab): (&ConnectionId, &Option<String>),
) {
    if let Some(entity) = state.connections.entity_of(*connection) {
        crate::advancements::seen(state, entity, tab.as_deref());
    }
}

pub(in crate::tick) fn request_stats(state: &mut TickState, connection: &ConnectionId) {
    if let Some(entity) = state.connections.by_connection.get(connection).copied()
        && let Some(player) = state.players.get(&entity)
    {
        let stats = player.statistics.wire_entries();
        state.push_player(*connection, PlayerChange::Statistics { stats });
    }
}

pub(in crate::tick) fn sign_update(
    state: &mut TickState,
    (connection, position, front, lines): (&ConnectionId, &BlockPos, &bool, &Vec<String>),
) {
    crate::signs::update(state, *connection, *position, *front, lines);
}

pub(in crate::tick) fn chat(
    state: &mut TickState,
    (connection, message): (&ConnectionId, &String),
) {
    // Reliable fan-out at commit: every joined connection gets
    // the broadcast in its own reliable outbox (never skipped).
    // No ChangeSet copy: the edge plans both, so a copy would
    // deliver every message twice.
    let Some(entity) = state.connections.entity_of(*connection) else {
        return;
    };
    let content = crate::scoreboard::chat_component(state, entity, message);
    let recipients: Vec<ConnectionId> = state.connections.all_connections().collect();
    for recipient in recipients {
        state.push_player(
            recipient,
            PlayerChange::StyledChat {
                content: content.clone(),
            },
        );
    }
    if state.dimension_globals.active {
        state
            .dimension_globals
            .broadcasts
            .push(crate::dimensions::globals::Broadcast::Chat(content));
    }
}
