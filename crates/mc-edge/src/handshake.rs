//! Handshake state: one serverbound packet, then Status or Login.
//!
//! Reads the initial handshake. Packet IDs and layouts come only from
//! `mc-protocol`.

use std::sync::Arc;

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
    /// A 26.1.2 (protocol 775) client served through `crate::legacy`.
    pub legacy: bool,
}

/// Reads the single handshake packet and returns the selected next state.
///
/// A mismatched Status handshake still reaches the Status exchange, which
/// advertises the supported release and protocol in the server list. A
/// mismatched Login handshake receives vanilla's
/// `multiplayer.disconnect.incompatible` reason and returns `Ok(None)`.
/// Protocol 775 is supported when `legacy` (the 26.1.2 profile) is loaded:
/// a Login then installs the per-session translator on `conn`.
/// `Transfer` (wire value 3) is routed through Login, but only when the server
/// accepts transfers (vanilla `accepts-transfers`); otherwise the client
/// receives `multiplayer.disconnect.transfers_disabled` and `Ok(None)` is
/// returned.
pub async fn read_handshake(
    conn: &mut Connection,
    accepts_transfers: bool,
    legacy: Option<&Arc<crate::legacy::Profile>>,
) -> anyhow::Result<Option<HandshakeOutcome>> {
    let handshake = conn
        .read_packet::<Handshake>(mc_protocol::State::Handshake)
        .await?;
    let transfer = handshake.next_state == ProtoNextState::Transfer;
    let next = match handshake.next_state {
        ProtoNextState::Status => NextState::Status,
        ProtoNextState::Login | ProtoNextState::Transfer => NextState::Login,
    };
    let legacy = legacy.filter(|_| {
        handshake.protocol_version == crate::legacy::PROTOCOL
            && PROTOCOL_VERSION != crate::legacy::PROTOCOL
    });
    if handshake.protocol_version != PROTOCOL_VERSION
        && legacy.is_none()
        && next == NextState::Login
    {
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
    if let Some(profile) = legacy
        && next == NextState::Login
    {
        conn.set_legacy(crate::legacy::Session::new(Arc::clone(profile)));
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
        legacy: legacy.is_some(),
    }))
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod tests;
