//! Tests for [`crate::status`]: payload shape and the full
//! request/response/ping/pong exchange over loopback.

use mc_protocol::packets::status::{PingRequest, PongResponse, StatusRequest, StatusResponse};
use mc_protocol::{PROTOCOL_VERSION, TARGET_RELEASE};
use tokio::net::TcpStream;

use crate::connection::Connection;

async fn loopback_pair() -> (Connection, Connection) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).await.unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (Connection::new(client), Connection::new(server))
}

#[test]
fn status_json_has_vanilla_shape() {
    let json = super::build_status_json("motd", 20, 3);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["version"]["name"], TARGET_RELEASE);
    assert_eq!(value["version"]["protocol"], PROTOCOL_VERSION);
    assert_eq!(value["players"]["max"], 20);
    assert_eq!(value["players"]["online"], 3);
    assert_eq!(value["description"]["text"], "motd");
    assert_eq!(value["enforcesSecureChat"], false);
}

#[tokio::test]
async fn full_status_exchange_echoes_ping() {
    let (mut client, mut server) = loopback_pair().await;
    let server_task = tokio::spawn(async move {
        super::handle_status(&mut server, "hi", 10, 1)
            .await
            .unwrap();
    });
    client.send_packet(&StatusRequest).await.unwrap();
    let response = client
        .read_packet::<StatusResponse>(mc_protocol::State::Status)
        .await
        .unwrap();
    assert!(response.json.contains("hi"));
    client
        .send_packet(&PingRequest { payload: 12345 })
        .await
        .unwrap();
    let pong = client
        .read_packet::<PongResponse>(mc_protocol::State::Status)
        .await
        .unwrap();
    assert_eq!(pong.payload, 12345);
    server_task.await.unwrap();
}
