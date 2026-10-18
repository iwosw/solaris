use crate::boundary::{BlockPos, ConnectionId, PlayerChange};
use crate::tick::TickState;

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
