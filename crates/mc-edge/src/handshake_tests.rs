//! Tests for [`crate::handshake`]: framing a handshake through a loopback
//! [`crate::connection::Connection`], version gating and transfer admission.

use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::packets::handshake::{Handshake, NextState as ProtoNextState};
use mc_protocol::packets::login::LoginDisconnect;
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

async fn send_handshake(conn: &mut Connection, next_state: ProtoNextState, address: &str) {
    conn.send_packet(&Handshake {
        protocol_version: PROTOCOL_VERSION,
        server_address: address.to_owned(),
        server_port: 25565,
        next_state,
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn status_handshake_routes_to_status() {
    let (mut client, mut server) = loopback_pair().await;
    send_handshake(&mut client, ProtoNextState::Status, "localhost").await;
    let outcome = super::read_handshake(&mut server, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.next, NextState::Status);
    assert_eq!(outcome.server_address, "localhost");
    assert!(!outcome.transfer);
}

#[tokio::test]
async fn login_handshake_routes_to_login() {
    let (mut client, mut server) = loopback_pair().await;
    send_handshake(&mut client, ProtoNextState::Login, "example.com").await;
    let outcome = super::read_handshake(&mut server, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.next, NextState::Login);
    assert!(!outcome.transfer);
}

#[tokio::test]
async fn transfer_handshake_is_login_only_when_accepted() {
    let (mut client, mut server) = loopback_pair().await;
    send_handshake(&mut client, ProtoNextState::Transfer, "example.com").await;
    let outcome = super::read_handshake(&mut server, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.next, NextState::Login);
    assert!(outcome.transfer);
    drop((client, server));

    let (mut client, mut server) = loopback_pair().await;
    send_handshake(&mut client, ProtoNextState::Transfer, "example.com").await;
    let server_task = tokio::spawn(async move {
        let outcome = super::read_handshake(&mut server, false).await.unwrap();
        assert!(outcome.is_none(), "disabled transfers are refused");
    });
    let disconnect = client
        .read_packet::<LoginDisconnect>(mc_protocol::State::Login)
        .await
        .unwrap();
    let reason: serde_json::Value = serde_json::from_str(&disconnect.reason_json).unwrap();
    assert_eq!(
        reason["translate"],
        "multiplayer.disconnect.transfers_disabled"
    );
    server_task.await.unwrap();
}

#[tokio::test]
async fn mismatched_status_protocol_still_routes_to_status() {
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

    let outcome = super::read_handshake(&mut server, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.next, NextState::Status);
}

#[tokio::test]
async fn mismatched_login_protocol_gets_vanilla_incompatible_disconnect() {
    let (mut client, mut server) = loopback_pair().await;
    client
        .send_packet(&Handshake {
            protocol_version: PROTOCOL_VERSION + 1,
            server_address: "localhost".to_owned(),
            server_port: 25565,
            next_state: ProtoNextState::Login,
        })
        .await
        .unwrap();
    let server_task = tokio::spawn(async move {
        assert!(
            super::read_handshake(&mut server, true)
                .await
                .unwrap()
                .is_none()
        );
    });

    let disconnect = client
        .read_packet::<LoginDisconnect>(mc_protocol::State::Login)
        .await
        .unwrap();
    let reason: serde_json::Value = serde_json::from_str(&disconnect.reason_json).unwrap();
    assert_eq!(reason["translate"], "multiplayer.disconnect.incompatible");
    assert_eq!(
        reason["with"],
        serde_json::json!([mc_protocol::TARGET_RELEASE])
    );
    server_task.await.unwrap();
}

#[tokio::test]
async fn mismatched_status_protocol_receives_supported_version_metadata() {
    let (mut client, mut server) = loopback_pair().await;
    let server_task = tokio::spawn(async move {
        let outcome = super::read_handshake(&mut server, true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.next, NextState::Status);
        crate::status::handle_status(&mut server, "motd", 20, 3)
            .await
            .unwrap();
    });
    client
        .send_packet(&Handshake {
            protocol_version: PROTOCOL_VERSION + 1,
            server_address: "localhost".to_owned(),
            server_port: 25565,
            next_state: ProtoNextState::Status,
        })
        .await
        .unwrap();
    client
        .send_packet(&mc_protocol::packets::status::StatusRequest)
        .await
        .unwrap();
    let response = client
        .read_packet::<mc_protocol::packets::status::StatusResponse>(mc_protocol::State::Status)
        .await
        .unwrap();
    let status: serde_json::Value = serde_json::from_str(&response.json).unwrap();
    assert_eq!(status["version"]["name"], mc_protocol::TARGET_RELEASE);
    assert_eq!(status["version"]["protocol"], PROTOCOL_VERSION);
    client
        .send_packet(&mc_protocol::packets::status::PingRequest { payload: 42 })
        .await
        .unwrap();
    let pong = client
        .read_packet::<mc_protocol::packets::status::PongResponse>(mc_protocol::State::Status)
        .await
        .unwrap();
    assert_eq!(pong.payload, 42);
    server_task.await.unwrap();
}
