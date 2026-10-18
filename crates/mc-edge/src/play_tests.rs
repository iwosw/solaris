//! Tests for [`crate::play::send_join_burst`]: the Play-entry packet order
//! the vanilla loading gate depends on.
//!
//! Background: vanilla 26.1.2 `LevelLoadTracker` starts in `WaitingForServer`
//! (whose `tick()` is a no-op with no timeout) and advances to
//! `WaitingForPlayerChunk` **only** when `ClientPacketListener.handleGameEvent`
//! sees `LEVEL_CHUNKS_LOAD_START` (game event 13). A burst that drops or
//! misplaces that event leaves the java client on `LevelLoadingScreen`
//! forever: no moves, no `PlayerLoaded`, harness timeout. The native client
//! has no such gate, so only java catches the regression.

use std::sync::Arc;

use mc_core::boundary::{ChangeSet, EntityId, TickOutput, WorldSnapshot};
use mc_protocol::packets::Packet;

use super::{GameEvent, JoinBurst};

async fn loopback_pair() -> (crate::connection::Connection, crate::connection::Connection) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (
        crate::connection::Connection::new(client),
        crate::connection::Connection::new(server),
    )
}

fn empty_output() -> TickOutput {
    TickOutput {
        tick: 1,
        snapshot: Arc::new(WorldSnapshot {
            day_time: 100,
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    }
}

/// Read burst frames until the closing time sync: the burst ends with
/// container + health + a second `SetTime`, so the second `0x71` bounds
/// the read without depending on the chunk count (zero here).
async fn read_burst_ids(conn: &mut crate::connection::Connection) -> Vec<(i32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut time_seen = 0u32;
    // Hard cap: the static burst is ~20 packets; never spin forever.
    for _ in 0..64 {
        let frame = conn.read_frame_play().await.expect("burst frame decodes");
        if frame.id == 0x71 {
            time_seen += 1;
        }
        out.push((frame.id, frame.body.to_vec()));
        if time_seen >= 2 {
            break;
        }
    }
    out
}

#[tokio::test]
async fn join_burst_carries_load_start_before_center() {
    let (mut client, mut server) = loopback_pair().await;
    let data = mc_data::solaris_required_data();
    let out = empty_output();
    let recipes = crate::recipes::RecipePackets {
        update: mc_protocol::packets::play::ClientboundUpdateRecipes {
            item_sets: Vec::new(),
            stonecutter_recipes: Vec::new(),
        },
        book: mc_protocol::packets::play::ClientboundRecipeBookAdd {
            entries: Vec::new(),
            replace: true,
        },
        advancements: Default::default(),
    };
    let server_task = tokio::spawn(async move {
        super::send_join_burst(JoinBurst {
            conn: &mut server,
            data: &data,
            out: &out,
            viewer_entity: EntityId(7),
            dimension: Default::default(),
            pos: [8.0, 80.0, 8.0],
            view_distance: 4,
            simulation_distance: 3,
            game_mode: 0,
            recipes: &recipes,
            op_level: 0,
            plugin_commands: &[],
        })
        .await
        .unwrap();
    });

    let frames = read_burst_ids(&mut client).await;
    server_task.await.unwrap();
    let ids: Vec<i32> = frames.iter().map(|(id, _)| *id).collect();
    let (_, body) = frames
        .iter()
        .find(|(id, _)| *id == mc_protocol::packets::play::ClientboundCommands::ID)
        .unwrap();
    let tree = mc_protocol::packets::play::ClientboundCommands::decode(&mut &body[..]).unwrap();
    assert!(tree.nodes.len() > 1);
    assert!(!tree.nodes.iter().any(|node| matches!(&node.kind, mc_protocol::packets::play::CommandNodeKind::Literal(name) if name == "op")));

    // LoginPlay opens the burst (the client creates its load tracker there).
    assert_eq!(ids.first(), Some(&0x31), "burst must open with LoginPlay");
    let (_, login_body) = frames.iter().find(|(id, _)| *id == 0x31).unwrap();
    let login = mc_protocol::packets::play::LoginPlay::decode(&mut &login_body[..]).unwrap();
    assert_eq!(login.view_distance, 4);
    assert_eq!(
        login.simulation_distance, 3,
        "G5 radius split: LoginPlay carries independent view/sim radii"
    );
    let (_, radius_body) = frames
        .iter()
        .find(|(id, _)| *id == mc_protocol::packets::play::SetChunkCacheRadius::ID)
        .expect("join sends the effective cache radius");
    let radius =
        mc_protocol::packets::play::SetChunkCacheRadius::decode(&mut &radius_body[..]).unwrap();
    assert_eq!(radius.radius, login.view_distance);
    let load_start: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, (id, body))| {
            *id == 0x26 && body.len() == 5 && body[0] == GameEvent::EVENT_START_WAITING_FOR_CHUNKS
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        load_start.len(),
        1,
        "burst must carry exactly one LEVEL_CHUNKS_LOAD_START (0x26/event 13), got ids {ids:02x?}"
    );
    let event_at = load_start[0];
    let login_at = ids.iter().position(|id| *id == 0x31).unwrap();
    let sync_at = ids.iter().position(|id| *id == 0x48).unwrap();
    let center_at = ids.iter().position(|id| *id == 0x5E).unwrap();
    assert!(
        event_at > login_at && event_at > sync_at && event_at < center_at,
        "load-start must sit after Login/SyncPos and before CenterChunk: {ids:02x?}"
    );
}
