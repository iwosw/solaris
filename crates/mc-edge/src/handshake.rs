//! Handshake state: one serverbound packet, then Status or Login.
//!
//! Reads the initial handshake. Packet IDs and layouts come only from
//! `mc-protocol`.

use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::packets::handshake::{Handshake, NextState as ProtoNextState};

use crate::connection::Connection;

/// Where the connection goes after the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextState {
    Status,
    Login,
}

/// Reads the single handshake packet, verifies the protocol version, and
/// returns the next state plus the server address the client connected to.
///
/// A mismatched protocol version is a disconnect: this returns `Err` and
/// the caller closes TCP. `Transfer` (wire value 3) is treated as Login,
/// matching Login handling until transfer support is wired.
pub async fn read_handshake(conn: &mut Connection) -> anyhow::Result<(NextState, String)> {
    let handshake = conn
        .read_packet::<Handshake>(mc_protocol::State::Handshake)
        .await?;
    if handshake.protocol_version != PROTOCOL_VERSION {
        anyhow::bail!(
            "unsupported protocol version {} (expected {PROTOCOL_VERSION})",
            handshake.protocol_version
        );
    }
    let next = match handshake.next_state {
        ProtoNextState::Status => NextState::Status,
        ProtoNextState::Login | ProtoNextState::Transfer => NextState::Login,
    };
    Ok((next, handshake.server_address))
}

#[cfg(test)]
#[path = "handshake_tests.rs"]
mod tests;
