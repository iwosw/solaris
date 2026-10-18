//! Tests for [`crate::handshake`]: framing a handshake through a loopback
//! [`crate::connection::Connection`] and checking version gating.

use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::packets::handshake::{Handshake, NextState as ProtoNextState};
use tokio::net::TcpStream;

use super::NextState;
use crate::connection::Connection;

async fn loopback_pair() -> (Connection, Connection) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).await.unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let _ = client.set_nodelay(true);
    let _ = server.set_nodelay(true);
    (Connection::new(client), Connection::new(server))
}

#[tokio::test]
async fn status_handshake_routes_to_status() {
    let (mut client, mut server) = loopback_pair().await;
    client
        .send_packet(&Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "localhost".to_owned(),
            server_port: 25565,
            next_state: ProtoNextState::Status,
        })
        .await
        .unwrap();
    let (next, address) = super::read_handshake(&mut server).await.unwrap();
    assert_eq!(next, NextState::Status);
    assert_eq!(address, "localhost");
}

#[tokio::test]
async fn login_handshake_routes_to_login_and_transfer_too() {
    for wire in [ProtoNextState::Login, ProtoNextState::Transfer] {
        let (mut client, mut server) = loopback_pair().await;
        client
            .send_packet(&Handshake {
                protocol_version: PROTOCOL_VERSION,
                server_address: "example.com".to_owned(),
                server_port: 25565,
                next_state: wire,
            })
            .await
            .unwrap();
        let (next, _) = super::read_handshake(&mut server).await.unwrap();
        assert_eq!(next, NextState::Login);
    }
}

#[tokio::test]
async fn wrong_protocol_version_is_rejected() {
    let (mut client, mut server) = loopback_pair().await;
    client
        .send_packet(&Handshake {
            protocol_version: PROTOCOL_VERSION + 1,
            server_address: "localhost".to_owned(),
            server_port: 25565,
            next_state: ProtoNextState::Status,
        })
        .await
        .unwrap();
    assert!(super::read_handshake(&mut server).await.is_err());
}
