use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use mc_data::Identifier;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundKeepAlive, ConfirmTeleportation, LevelChunkWithLight, ServerboundChatCommand,
    ServerboundKeepAlive, SetCenterChunk, SynchronizePlayerPosition,
};
use mc_server::wire_client::Client;
use mc_world::{BlockRegistry, Chunk, ChunkGenerator, ChunkPos, WorldStorage};

const SECOND_CENTER: (i32, i32) = (32, 0);
const RESIDENT_BUDGET: usize = 64 * 1024 * 1024;
const SCENARIO_DEADLINE: Duration = Duration::from_secs(30);

fn view_window(center: (i32, i32), radius: i32) -> HashSet<(i32, i32)> {
    (center.1 - radius..=center.1 + radius)
        .flat_map(|z| (center.0 - radius..=center.0 + radius).map(move |x| (x, z)))
        .collect()
}

/// Fixture chunks carry a stone floor at y=0 under clear air, so the login
/// spawn scan admits a column. Pure-air chunks have no opaque surface and
/// no registered support state, so login refuses and the peer closes.
fn fixture_chunk(
    blocks: &BlockRegistry,
    table: &mc_data::block_light::BlockLightTable,
    position: ChunkPos,
) -> Chunk {
    let stone = blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:stone").expect("valid block id"),
            &[],
        )
        .expect("stone state registered");
    let air = blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:air").expect("valid block id"),
            &[],
        )
        .expect("air state registered");
    let mut chunk = Chunk::empty(
        position,
        air,
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for lx in 0..16u8 {
        for lz in 0..16u8 {
            chunk.set_block(lx, 0, lz, stone);
        }
    }
    chunk.rebuild_highest_opaque(table);
    debug_assert_eq!(chunk.highest_opaque_y(0, 0), Some(0));
    // Only chunk.heightmaps persists to disk (chunk_nbt.rs); highest_opaque
    // is rebuilt from MOTION_BLOCKING on load. Publish the built map or the
    // flush/reopen round-trip loses the surface.
    chunk
        .heightmaps
        .insert("MOTION_BLOCKING".to_string(), chunk.highest_opaque.clone());
    chunk
}

fn fixture_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("required registry builds"),
    )
}

struct FixtureGenerator {
    blocks: Arc<BlockRegistry>,
    table: Arc<mc_data::block_light::BlockLightTable>,
}

impl ChunkGenerator for FixtureGenerator {
    fn generate(&self, position: ChunkPos) -> Chunk {
        fixture_chunk(&self.blocks, &self.table, position)
    }
}

struct DiskFixture {
    directory: tempfile::TempDir,
    blocks: Arc<BlockRegistry>,
}

impl DiskFixture {
    fn new(radius: i32) -> Self {
        let directory = tempfile::tempdir().expect("temporary chunk world");
        std::fs::create_dir(directory.path().join("region")).expect("temporary region root");
        let blocks = fixture_registry();
        let table = Arc::new(
            mc_data::block_light::BlockLightTable::conservative_from_blocks_report(
                &mc_data::blocks::solaris_required_blocks_report(),
            ),
        );
        let mut seed =
            WorldStorage::open_with_region_capacity(directory.path(), Arc::clone(&blocks), 8)
                .expect("seed world");
        seed.set_chunk_byte_budgets(RESIDENT_BUDGET, RESIDENT_BUDGET)
            .expect("seed byte budgets");
        let positions = view_window((0, 0), radius)
            .union(&view_window(SECOND_CENTER, radius))
            .copied()
            .collect::<HashSet<_>>();
        for &(x, z) in &positions {
            let position = ChunkPos { x, z };
            seed.insert_generated_chunk(position, fixture_chunk(&blocks, &table, position))
                .expect("seed chunk");
        }
        assert_eq!(
            seed.flush_dirty().expect("persist seed chunks"),
            positions.len()
        );
        // Verify the surface survives the real flush/reopen round-trip the
        // server will read: reload one seeded chunk from disk and check it.
        {
            let mut probe =
                WorldStorage::open_with_region_capacity(directory.path(), Arc::clone(&blocks), 8)
                    .expect("reopen seed world");
            let surface = probe
                .get_chunk(ChunkPos { x: 0, z: 0 })
                .expect("seeded spawn chunk reloads")
                .expect("spawn chunk present")
                .highest_opaque_y(0, 0);
            assert_eq!(surface, Some(0));
        }
        drop(seed);
        Self { directory, blocks }
    }

    fn byte_budget_only_storage(&self) -> WorldStorage {
        let mut storage = WorldStorage::open_with_region_capacity(
            self.directory.path(),
            Arc::clone(&self.blocks),
            8,
        )
        .expect("byte-budget-only world");
        storage
            .set_chunk_byte_budgets(RESIDENT_BUDGET, RESIDENT_BUDGET)
            .expect("resident byte budgets");
        storage
    }
}

struct RunningServer {
    addr: SocketAddr,
    world: Arc<tokio::sync::Mutex<WorldStorage>>,
    shutdown: mc_net::ShutdownHandle,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl RunningServer {
    async fn start(storage: WorldStorage, blocks: Arc<BlockRegistry>, radius: i32) -> Self {
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let shutdown = mc_net::ShutdownHandle::default();
        let bound = mc_net::bind(mc_net::ServerConfig {
            tab_list: mc_net::TabListConfig::default(),
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "chunk residency regression".into(),
            max_players: 2,
            view_distance: radius,
            data: Arc::new(mc_data::solaris_required_data()),
            blocks,
            world: Some(Arc::clone(&world)),
            tags: Arc::new(mc_data::tags::TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            // No runtime lighting: the table built the fixture heightmaps
            // offline. Enabling it would demand a full light neighbourhood
            // beyond the 9-chunk pinned window (prepare.rs need_full_hood).
            block_light: None,
            items: Arc::new(mc_data::items::ItemRegistry::default()),
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
            random_tick: mc_simulation::tick_policy::RandomTickPolicy {
                friendly_spawn_interval_ticks: 0,
                hostile_spawn_interval_ticks: 0,
                ..mc_simulation::tick_policy::RandomTickPolicy::default()
            },
            command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
            loader_manifest: None,
            shutdown: shutdown.clone(),
        })
        .await
        .expect("bind residency server");
        let addr = bound.local_addr().expect("residency server address");
        let task = tokio::spawn(bound.serve());
        Self {
            addr,
            world,
            shutdown,
            task,
        }
    }

    async fn stop(self) {
        self.shutdown.request();
        self.task
            .await
            .expect("server task")
            .expect("server shutdown");
    }
}

async fn reply_to_control_frame(client: &mut Client, frame: &mc_protocol::RawFrame) {
    if frame.id == ClientboundKeepAlive::ID {
        let keepalive = ClientboundKeepAlive::decode(&mut frame.body.clone()).expect("keepalive");
        client
            .write_packet(&ServerboundKeepAlive { id: keepalive.id })
            .await
            .expect("keepalive response");
    } else if frame.id == SynchronizePlayerPosition::ID {
        let position =
            SynchronizePlayerPosition::decode(&mut frame.body.clone()).expect("teleport");
        client
            .write_packet(&ConfirmTeleportation {
                teleport_id: position.teleport_id,
            })
            .await
            .expect("teleport response");
    }
}

async fn creative_client(addr: SocketAddr, name: &str) -> Client {
    let (mut client, _) = Client::connect_to_play(addr, name)
        .await
        .expect("enter Play");
    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".into(),
        })
        .await
        .expect("creative mode");
    client
}

async fn teleport_to_second_window(client: &mut Client) {
    client
        .write_packet(&ServerboundChatCommand {
            command: "tp 512.5 80 0.5".into(),
        })
        .await
        .expect("move second player to disjoint window");
}

async fn receive_window(client: &mut Client, center: (i32, i32), radius: i32) {
    let expected = view_window(center, radius);
    let mut centered = center == (0, 0);
    let mut received = HashSet::new();
    while received != expected {
        let frame = client.read_frame().await.unwrap_or_else(|error| {
            let missing = expected.difference(&received).copied().collect::<Vec<_>>();
            panic!("chunk window {center:?} closed with missing chunks {missing:?}: {error}");
        });
        reply_to_control_frame(client, &frame).await;
        if frame.id == SetCenterChunk::ID {
            let center_packet =
                SetCenterChunk::decode(&mut frame.body.clone()).expect("view center");
            centered = (center_packet.chunk_x, center_packet.chunk_z) == center;
        } else if centered && frame.id == LevelChunkWithLight::ID {
            let chunk = LevelChunkWithLight::decode(&mut frame.body.clone()).expect("chunk packet");
            let position = (chunk.chunk_x, chunk.chunk_z);
            assert!(
                expected.contains(&position),
                "chunk {position:?} outside {center:?} view"
            );
            assert!(
                received.insert(position),
                "duplicate chunk {position:?} in {center:?} view"
            );
        }
    }
}

async fn keep_first_player_connected<F: Future<Output = ()>>(first: &mut Client, second_work: F) {
    tokio::pin!(second_work);
    loop {
        tokio::select! {
            () = &mut second_work => break,
            frame = first.read_frame() => {
                let frame = frame.expect("first player's retained view stays connected");
                reply_to_control_frame(first, &frame).await;
            }
        }
    }
}

#[tokio::test]
async fn byte_budget_only_world_streams_two_disjoint_vd6_windows() {
    let fixture = DiskFixture::new(6);
    let server = RunningServer::start(
        fixture.byte_budget_only_storage(),
        Arc::clone(&fixture.blocks),
        6,
    )
    .await;
    tokio::time::timeout(SCENARIO_DEADLINE, async {
        let mut first = creative_client(server.addr, "ResidencyFirst").await;
        receive_window(&mut first, (0, 0), 6).await;
        let mut second = creative_client(server.addr, "ResidencySecond").await;
        teleport_to_second_window(&mut second).await;
        keep_first_player_connected(&mut first, receive_window(&mut second, SECOND_CENTER, 6))
            .await;
        // Both exact 169-chunk views are retained at the same time. This exceeds
        // the former 289-slot production cache despite fitting the byte budget.
        drop(second);
        drop(first);
    })
    .await
    .expect("both disjoint VD6 windows must complete, not wait on clean retained residency");
    server.stop().await;
}

enum PinnedLimit {
    Slots,
    Bytes,
}

async fn pinned_pressure_disconnects(limit: PinnedLimit) {
    let blocks = fixture_registry();
    let table = Arc::new(
        mc_data::block_light::BlockLightTable::conservative_from_blocks_report(
            &mc_data::blocks::solaris_required_blocks_report(),
        ),
    );
    let generator = Arc::new(FixtureGenerator {
        blocks: Arc::clone(&blocks),
        table: Arc::clone(&table),
    });
    let mut storage = match limit {
        PinnedLimit::Slots => {
            WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), view_window((0, 0), 1).len())
        }
        PinnedLimit::Bytes => WorldStorage::in_memory_with_byte_budgets(
            Arc::clone(&blocks),
            RESIDENT_BUDGET,
            RESIDENT_BUDGET,
        )
        .expect("deliberate byte-capped world"),
    }
    .with_generator(generator);
    for (x, z) in view_window((0, 0), 1) {
        let position = ChunkPos { x, z };
        storage
            .commit_chunk_snapshot(position, fixture_chunk(&blocks, &table, position))
            .expect("clean first-player chunks");
    }
    if matches!(limit, PinnedLimit::Bytes) {
        let resident_bytes = storage.stats().resident_bytes;
        storage
            .set_chunk_byte_budgets(resident_bytes, resident_bytes)
            .expect("set deliberate exhausted resident-byte quota");
    }
    let server = RunningServer::start(storage, blocks, 1).await;
    tokio::time::timeout(SCENARIO_DEADLINE, async {
        let mut first = creative_client(server.addr, "PinnedFirst").await;
        receive_window(&mut first, (0, 0), 1).await;
        let requested = ChunkPos { x: SECOND_CENTER.0, z: SECOND_CENTER.1 };
        assert!(matches!(
            server.world.lock().await.chunk_admission_error(requested),
            Some(mc_world::WorldError::ChunkCachePinned { .. })
        ));
        let mut second = creative_client(server.addr, "PinnedSecond").await;
        teleport_to_second_window(&mut second).await;
        keep_first_player_connected(&mut first, async {
            loop {
                match second.read_frame().await {
                    Ok(frame) => reply_to_control_frame(&mut second, &frame).await,
                    Err(error) => {
                        assert!(
                            error.downcast_ref::<mc_protocol::FramingError>().is_none(),
                            "malformed protocol data is not an explicit pressure disconnect: {error}"
                        );
                        if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
                            assert!(matches!(
                                io_error.kind(),
                                std::io::ErrorKind::ConnectionReset
                                    | std::io::ErrorKind::UnexpectedEof
                                    | std::io::ErrorKind::BrokenPipe
                            ), "unexpected transport failure: {io_error}");
                        }
                        break;
                    }
                }
            }
        })
        .await;
        drop(second);
        drop(first);
    })
    .await
    .expect("pinned residency must close the requesting connection instead of remaining pending");
    server.stop().await;
}

#[tokio::test]
async fn retained_slot_limit_disconnects_instead_of_stalling() {
    pinned_pressure_disconnects(PinnedLimit::Slots).await;
}

#[tokio::test]
async fn retained_byte_limit_disconnects_instead_of_stalling() {
    pinned_pressure_disconnects(PinnedLimit::Bytes).await;
}
