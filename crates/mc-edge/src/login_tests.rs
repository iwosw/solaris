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
                velocity: None,
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
                velocity: None,
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
                velocity: None,
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
                    compression_threshold: 256,
                    velocity: None,
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

fn forwarding_message(secret: &str, name: &str, uuid: Uuid, address: &str) -> Vec<u8> {
    use mc_protocol::codec::WriteMc;
    let mut body = Vec::new();
    body.write_varint(4);
    body.write_string(address, 255).unwrap();
    body.write_uuid(uuid);
    body.write_string(name, 16).unwrap();
    body.write_varint(1);
    body.write_string("textures", 32_767).unwrap();
    body.write_string("{\"value\":\"skin\"}", 32_767).unwrap();
    body.write_bool(false);
    let mut signed = crate::proxy::hmac_sha256(secret.as_bytes(), &body).to_vec();
    signed.extend_from_slice(&body);
    signed
}

fn velocity_opts(secret: &str) -> LoginOpts {
    LoginOpts {
        online_mode: false,
        compression_threshold: 256,
        velocity: Some(crate::proxy::VelocityForwarding::new(secret).unwrap()),
    }
}

#[tokio::test]
async fn velocity_login_forwards_signed_profile() {
    let (mut client, mut server) = loopback_pair().await;
    let forwarded_uuid = Uuid::from_u128(0xABCD_EF01_2345_6789_ABCD_EF01_2345_6789);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen_by_server = std::sync::Arc::clone(&seen);
    let server_task = tokio::spawn(async move {
        super::handle_login_admitted(
            &mut server,
            &velocity_opts("s3cret"),
            None,
            Some("10.0.0.1".parse().unwrap()),
            |uuid, ip| {
                *seen_by_server.lock().unwrap() = Some((uuid, ip));
                None
            },
        )
        .await
        .unwrap()
        .expect("forwarded login succeeds")
    });
    client
        .send_packet(&LoginStart {
            name: "Proxied".into(),
            player_uuid: Uuid::new_v4(),
        })
        .await
        .unwrap();
    let request = client
        .read_packet::<mc_protocol::packets::login::LoginPluginRequest>(mc_protocol::State::Login)
        .await
        .unwrap();
    assert_eq!(request.message_id, 0);
    assert_eq!(request.channel.as_str(), crate::proxy::CHANNEL);
    assert_eq!(request.data, vec![crate::proxy::MAX_FORWARDING_VERSION]);
    client
        .send_packet(&mc_protocol::packets::login::LoginPluginResponse {
            message_id: 0,
            successful: true,
            data: forwarding_message("s3cret", "Proxied", forwarded_uuid, "198.51.100.9"),
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
    assert_eq!(success.uuid, forwarded_uuid);
    assert_eq!(success.name, "Proxied");
    assert_eq!(success.properties.len(), 1);
    assert_eq!(success.properties[0].name, "textures");
    client.send_packet(&LoginAcknowledged).await.unwrap();

    let outcome = server_task.await.unwrap();
    assert_eq!(outcome.uuid, forwarded_uuid);
    assert_eq!(
        outcome.remote_address,
        Some("198.51.100.9".parse().unwrap())
    );
    let (admitted_uuid, admitted_ip) = seen.lock().unwrap().unwrap();
    assert_eq!(admitted_uuid, forwarded_uuid);
    assert_eq!(
        admitted_ip,
        Some("198.51.100.9".parse().unwrap()),
        "admission sees the forwarded client address, not the proxy"
    );
}

async fn refused_velocity_login(
    response: mc_protocol::packets::login::LoginPluginResponse,
) -> (Option<super::LoginOutcome>, String) {
    let (mut client, mut server) = loopback_pair().await;
    let server_task = tokio::spawn(async move {
        super::handle_login_admitted(
            &mut server,
            &velocity_opts("s3cret"),
            None,
            Some("10.0.0.1".parse().unwrap()),
            |_, _| None,
        )
        .await
        .unwrap()
    });
    client
        .send_packet(&LoginStart {
            name: "Proxied".into(),
            player_uuid: Uuid::new_v4(),
        })
        .await
        .unwrap();
    let _request = client
        .read_packet::<mc_protocol::packets::login::LoginPluginRequest>(mc_protocol::State::Login)
        .await
        .unwrap();
    client.send_packet(&response).await.unwrap();
    let disconnect = client
        .read_packet::<LoginDisconnect>(mc_protocol::State::Login)
        .await
        .unwrap();
    let outcome = server_task.await.unwrap();
    (outcome, disconnect.reason_json)
}

#[tokio::test]
async fn velocity_login_refuses_an_unanswered_plugin_request() {
    let (outcome, reason) =
        refused_velocity_login(mc_protocol::packets::login::LoginPluginResponse {
            message_id: 0,
            successful: false,
            data: Vec::new(),
        })
        .await;
    assert!(outcome.is_none());
    assert!(reason.contains("Velocity"), "reason: {reason}");
}

#[tokio::test]
async fn velocity_login_refuses_a_wrong_signature() {
    let (outcome, reason) =
        refused_velocity_login(mc_protocol::packets::login::LoginPluginResponse {
            message_id: 0,
            successful: true,
            data: forwarding_message("wrong-secret", "Proxied", Uuid::new_v4(), "198.51.100.9"),
        })
        .await;
    assert!(outcome.is_none());
    assert!(reason.contains("Velocity"), "reason: {reason}");
}

#[tokio::test]
async fn velocity_login_refuses_an_unknown_transaction_id() {
    let (outcome, reason) =
        refused_velocity_login(mc_protocol::packets::login::LoginPluginResponse {
            message_id: 7,
            successful: true,
            data: forwarding_message("s3cret", "Proxied", Uuid::new_v4(), "198.51.100.9"),
        })
        .await;
    assert!(outcome.is_none());
    assert!(reason.contains("Velocity"), "reason: {reason}");
}

#[tokio::test]
async fn velocity_login_refuses_a_forwarded_name_mismatch() {
    let (outcome, reason) =
        refused_velocity_login(mc_protocol::packets::login::LoginPluginResponse {
            message_id: 0,
            successful: true,
            data: forwarding_message("s3cret", "SomeoneElse", Uuid::new_v4(), "198.51.100.9"),
        })
        .await;
    assert!(outcome.is_none());
    assert!(reason.contains("Velocity"), "reason: {reason}");
}

#[tokio::test]
async fn velocity_with_online_mode_is_a_logic_error() {
    let (mut client, mut server) = loopback_pair().await;
    let server_task = tokio::spawn(async move {
        let mut opts = velocity_opts("s3cret");
        opts.online_mode = true;
        super::handle_login(&mut server, &opts).await
    });
    client
        .send_packet(&LoginStart {
            name: "Proxied".into(),
            player_uuid: Uuid::new_v4(),
        })
        .await
        .unwrap();
    assert!(server_task.await.unwrap().is_err());
}
