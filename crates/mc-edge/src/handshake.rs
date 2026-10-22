//! Handshake state: one serverbound packet, then Status or Login.
//!
//! Reads the initial handshake. Packet IDs and layouts come only from
//! `mc-protocol`.

use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::packets::handshake::{Handshake, NextState as ProtoNextState};
use mc_protocol::packets::login::LoginDisconnect;

use crate::connection::Connection;

/// Where the connection goes after the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextState {
    Status,
    Login,
}

/// Accepted handshake intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeOutcome {
    pub next: NextState,
    /// Server address the client connected to (`host` or `host\0data`).
    pub server_address: String,
    /// The client arrived through a `Transfer` from another server.
    pub transfer: bool,
}

/// Reads the single handshake packet and returns the selected next state.
///
/// A mismatched Status handshake still reaches the Status exchange, which
/// advertises the supported release and protocol in the server list. A
/// mismatched Login handshake receives vanilla's
/// `multiplayer.disconnect.incompatible` reason and returns `Ok(None)`.
/// `Transfer` (wire value 3) is routed through Login, but only when the server
/// accepts transfers (vanilla `accepts-transfers`); otherwise the client
/// receives `multiplayer.disconnect.transfers_disabled` and `Ok(None)` is
/// returned.
pub async fn read_handshake(
    conn: &mut Connection,
    accepts_transfers: bool,
) -> anyhow::Result<Option<HandshakeOutcome>> {
    let handshake = conn
        .read_packet::<Handshake>(mc_protocol::State::Handshake)
        .await?;
    let transfer = handshake.next_state == ProtoNextState::Transfer;
    let next = match handshake.next_state {
        ProtoNextState::Status => NextState::Status,
        ProtoNextState::Login | ProtoNextState::Transfer => NextState::Login,
    };
    if handshake.protocol_version != PROTOCOL_VERSION && next == NextState::Login {
        conn.send_packet(&LoginDisconnect {
            reason_json: serde_json::json!({
                "translate": "multiplayer.disconnect.incompatible",
                "with": [mc_protocol::TARGET_RELEASE],
            })
            .to_string(),
        })
        .await?;
        return Ok(None);
    }
    if transfer && !accepts_transfers {
        conn.send_packet(&LoginDisconnect {
            reason_json:
                serde_json::json!({"translate":"multiplayer.disconnect.transfers_disabled"})
                    .to_string(),
        })
        .await?;
        return Ok(None);
    }
    Ok(Some(HandshakeOutcome {
        next,
        server_address: handshake.server_address,
        transfer,
    }))
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod tests;
