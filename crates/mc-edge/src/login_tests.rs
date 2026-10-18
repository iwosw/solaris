//! Tests for [`crate::login`]: name validation, offline UUID pins, the
//! offline login exchange, and rejection paths over loopback.

use mc_protocol::packets::login::{LoginAcknowledged, LoginDisconnect, LoginStart, LoginSuccess};
use tokio::net::TcpStream;
use uuid::Uuid;

use super::{LoginOpts, validate_login_name};
use crate::connection::Connection;
use crate::crypto::offline_uuid;

async fn loopback_pair() -> (Connection, Connection) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).await.unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (Connection::new(client), Connection::new(server))
}

#[test]
fn name_validation_boundaries() {
    for name in ["abc", "A_1", "abcdefghijklmnop", "Player_123"] {
        assert!(validate_login_name(name).is_ok(), "name {name:?}");
    }
    for name in [
        "",
        "ab",
        "abcdefghijklmnopq",
        "has space",
        "semi;colon",
        "ünicode",
    ] {
        assert!(validate_login_name(name).is_err(), "name {name:?}");
    }
}

#[test]
fn offline_uuid_pins_hold() {
    assert_eq!(
        offline_uuid("Notch"),
        Uuid::parse_str("b50ad385-829d-3141-a216-7e7d7539ba7f").unwrap()
    );
    assert_eq!(
        offline_uuid("Player"),
        Uuid::parse_str("a01e3843-e521-3998-958a-f459800e4d11").unwrap()
    );
}

#[tokio::test]
async fn offline_login_round_trip() {
    let (mut client, mut server) = loopback_pair().await;
    let server_task = tokio::spawn(async move {
        super::handle_login(
            &mut server,
            &LoginOpts {
                online_mode: false,
                compression_threshold: 256,
            },
        )
        .await
        .unwrap()
        .expect("offline login succeeds")
    });
    client
        .send_packet(&LoginStart {
            name: "TestPlayer".to_owned(),
            player_uuid: Uuid::nil(),
        })
        .await
        .unwrap();
    let _compression = client
        .read_packet::<mc_protocol::packets::login::SetCompression>(mc_protocol::State::Login)
        .await
        .unwrap();
    client.set_compression(mc_protocol::frame::Compression::Threshold(256));
    let success = client
        .read_packet::<LoginSuccess>(mc_protocol::State::Login)
        .await
        .unwrap();
    assert_eq!(success.name, "TestPlayer");
    assert_eq!(success.uuid, offline_uuid("TestPlayer"));
    client.send_packet(&LoginAcknowledged).await.unwrap();
    let outcome = server_task.await.unwrap();
    assert_eq!(outcome.name, "TestPlayer");
    assert_eq!(outcome.uuid, offline_uuid("TestPlayer"));
    assert!(outcome.properties.is_empty());
}

#[tokio::test]
async fn invalid_name_is_rejected_with_disconnect() {
    let (mut client, mut server) = loopback_pair().await;
    let server_task = tokio::spawn(async move {
        let outcome = super::handle_login(
            &mut server,
            &LoginOpts {
                online_mode: false,
                compression_threshold: 256,
            },
        )
        .await
        .unwrap();
        assert!(outcome.is_none(), "invalid name must be rejected");
    });
    client
        .send_packet(&LoginStart {
            name: "bad name!".to_owned(),
            player_uuid: Uuid::nil(),
        })
        .await
        .unwrap();
    let disconnect = client
        .read_packet::<LoginDisconnect>(mc_protocol::State::Login)
        .await
        .unwrap();
    assert!(disconnect.reason_json.contains("Invalid username"));
    server_task.await.unwrap();
}

#[tokio::test]
async fn online_without_auth_context_errors() {
    let (mut client, mut server) = loopback_pair().await;
    let server_task = tokio::spawn(async move {
        super::handle_login_with_auth(
            &mut server,
            &LoginOpts {
                online_mode: true,
                compression_threshold: 256,
            },
            None,
            None,
        )
        .await
    });
    client
        .send_packet(&LoginStart {
            name: "TestPlayer".to_owned(),
            player_uuid: Uuid::nil(),
        })
        .await
        .unwrap();
    let result = server_task.await.unwrap();
    assert!(result.is_err(), "online without context must fail");
}

#[tokio::test]
async fn banned_uuid_refused_before_login_success() {
    let (mut client, mut server) = loopback_pair().await;
    let dir = std::env::temp_dir().join(format!("edge-ban-login-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let admin = crate::admin::Admin::open(&dir).unwrap();
    admin
        .ban_player(
            offline_uuid("BannedPlayer"),
            "BannedPlayer",
            "Console",
            "Test ban",
            true,
        )
        .unwrap();
    let server_task = tokio::spawn(async move {
        assert!(
            super::handle_login_admitted(
                &mut server,
                &LoginOpts {
                    online_mode: false,
                    compression_threshold: 256
                },
                None,
                Some("127.0.0.1".parse().unwrap()),
                |uuid, ip| admin.admission(uuid, ip, false),
            )
            .await
            .unwrap()
            .is_none()
        );
    });
    client
        .send_packet(&LoginStart {
            name: "BannedPlayer".into(),
            player_uuid: Uuid::new_v4(),
        })
        .await
        .unwrap();
    let disconnect = client
        .read_packet::<LoginDisconnect>(mc_protocol::State::Login)
        .await
        .unwrap();
    let reason: serde_json::Value = serde_json::from_str(&disconnect.reason_json).unwrap();
    assert_eq!(reason["translate"], "multiplayer.disconnect.banned.reason");
    assert_eq!(reason["with"][0], "Test ban");
    server_task.await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
