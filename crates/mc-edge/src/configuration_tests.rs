//! Tests for [`crate::configuration`]: pack advertisement, known-packs
//! negotiation, registry/tag/brand/finish ordering, and ignore tolerance.

use mc_protocol::TARGET_RELEASE;
use mc_protocol::packets::Packet as _;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration,
    ServerboundClientInformation, ServerboundKnownPacks,
};
use mc_protocol::packets::{ChatVisibility, ClientInformation, MainHand, ParticleStatus};
use tokio::net::TcpStream;

use crate::connection::Connection;

async fn loopback_pair() -> (Connection, Connection) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).await.unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (Connection::new(client), Connection::new(server))
}

fn information(view_distance: i8) -> ClientInformation {
    ClientInformation {
        language: "en_us".to_owned(),
        view_distance,
        chat_visibility: ChatVisibility::Full,
        chat_colors: true,
        model_customisation: 0,
        main_hand: MainHand::Right,
        text_filtering_enabled: false,
        allows_listing: true,
        particle_status: ParticleStatus::All,
    }
}

fn test_data() -> mc_data::VanillaData {
    mc_data::testing::stub()
}

fn test_tags() -> mc_data::tags::TagsData {
    mc_data::tags::TagsData::default()
}

#[tokio::test]
async fn full_configuration_with_matching_pack() {
    let (mut client, mut server) = loopback_pair().await;
    let data = test_data();
    let registry_count = data.registry_count();
    let server_task = tokio::spawn(async move {
        let tags = test_tags();
        super::handle_configuration(&mut server, &data, &tags, None)
            .await
            .unwrap()
    });

    // UpdateEnabledFeatures.
    let _ = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    // ClientboundKnownPacks: must advertise minecraft:core:<release>.
    let frame = client
        .read_frame_in(mc_protocol::State::Configuration)
        .await
        .unwrap();
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let mut body = frame.body.clone();
    let packs = ClientboundKnownPacks::decode(&mut body).expect("known packs decode");
    assert!(
        packs.packs.iter().any(|p| {
            p.namespace == "minecraft" && p.id == "core" && p.version == TARGET_RELEASE
        })
    );

    // Tolerated packet before KnownPacks: client information.
    client
        .send_packet(&ServerboundClientInformation {
            information: information(6),
        })
        .await
        .unwrap();
    client
        .send_packet(&ServerboundKnownPacks {
            packs: packs.packs.clone(),
        })
        .await
        .unwrap();

    // Drain RegistryData + UpdateTags + Brand + FinishConfiguration.
    let mut saw_finish = false;
    for _ in 0..(registry_count + 8) {
        let frame = client
            .read_frame_in(mc_protocol::State::Configuration)
            .await
            .unwrap();
        if frame.id == FinishConfiguration::ID {
            saw_finish = true;
            break;
        }
    }
    assert!(saw_finish, "server must send FinishConfiguration");

    // Tolerated packet while waiting for ack, then ack.
    client
        .send_packet(&ServerboundClientInformation {
            information: information(9),
        })
        .await
        .unwrap();
    client
        .send_packet(&AcknowledgeFinishConfiguration)
        .await
        .unwrap();

    let outcome = server_task.await.unwrap();
    assert_eq!(outcome.client_view_distance, 9);
}

#[tokio::test]
async fn missing_pack_without_sidecar_payloads_fails() {
    let (mut client, mut server) = loopback_pair().await;
    // `testing::stub()` has no sidecar payloads, so a client that reports
    // no packs must be rejected.
    let data = test_data();
    assert!(!data.has_full_registry_payloads());
    let server_task = tokio::spawn(async move {
        let tags = test_tags();
        super::handle_configuration(&mut server, &data, &tags, None).await
    });
    // Skip features + known packs advertisements.
    for _ in 0..2 {
        let _ = client
            .read_frame_in(mc_protocol::State::Configuration)
            .await
            .unwrap();
    }
    client
        .send_packet(&ServerboundKnownPacks { packs: Vec::new() })
        .await
        .unwrap();
    let result = server_task.await.unwrap();
    assert!(result.is_err());
}
