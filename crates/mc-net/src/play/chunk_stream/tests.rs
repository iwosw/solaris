use super::*;
use crate::ChunkPipelineGeneration;
use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_protocol::frame::Compression;
use mc_world::{BlockStateId, ChunkGenerator, ChunkPos, WorldStorage};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::sync::{Mutex, oneshot};

#[test]
fn pending_loaded_chunk_retention_is_cancellation_safe_and_transferable() {
    let registry =
        Arc::new(mc_world::BlockRegistry::from_report(&[]).expect("empty block registry builds"));
    let world = WorldStorage::in_memory(registry);
    let world_read = world.read_view();
    let chunk = (2, -3);
    let position = ChunkPos { x: 2, z: -3 };

    {
        let _pending = PendingLoadedChunkRetention::new(Some(&world_read), chunk);
    }
    assert!(
        !world_read.release_chunk(position),
        "dropping pending write ownership must release its retention"
    );

    let pending = PendingLoadedChunkRetention::new(Some(&world_read), chunk);
    pending.transfer_to_loaded();
    assert!(
        world_read.release_chunk(position),
        "loaded-set ownership retains the chunk after transfer"
    );
    assert!(!world_read.release_chunk(position));
}

fn canonical_entity_type_report() -> Vec<mc_data::entity_types::EntityTypeReport> {
    (0..mc_data::entity_types::ENTITY_TYPE_COUNT as u32)
        .map(|protocol_id| {
            let contract =
                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(protocol_id)
                    .expect("canonical 26.1.2 entity contract is dense");
            mc_data::entity_types::EntityTypeReport {
                id: Identifier::parse(contract.name)
                    .expect("canonical entity contract name is a valid identifier"),
                protocol_id,
            }
        })
        .collect()
}

fn healthy_runtime_control_input() -> crate::RuntimeControlInput {
    crate::RuntimeControlInput {
        tick_ms: 0,
        memory_used_mb: 0,
        memory_limit_mb: 0,
    }
}

async fn observe_next_runtime_control_signal(
    control: &crate::RuntimeControlHandle,
    signals: &mut crate::control_plane::RuntimeControlSignalReceiver,
) -> crate::AutoscaleDecision {
    let signal = signals
        .recv()
        .await
        .expect("runtime control signal arrives");
    control.observe_signal(signal)
}

struct InvalidatePreparedOnWrite {
    sessions: Arc<SessionRegistry>,
    chunk: (i32, i32),
    written: usize,
}

impl tokio::io::AsyncWrite for InvalidatePreparedOnWrite {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let writer = self.get_mut();
        if writer.written == 0 {
            writer
                .sessions
                .invalidate_prepared_chunks(&HashSet::from([writer.chunk]));
        }
        writer.written += bytes.len();
        std::task::Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

struct CountingGenerator {
    calls: Arc<AtomicUsize>,
}

impl ChunkGenerator for CountingGenerator {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Chunk::empty(
            pos,
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}

struct GenerationGate {
    started: tokio::sync::Notify,
    released: std::sync::Mutex<bool>,
    released_cv: std::sync::Condvar,
}

impl GenerationGate {
    fn new() -> Self {
        Self {
            started: tokio::sync::Notify::new(),
            released: std::sync::Mutex::new(false),
            released_cv: std::sync::Condvar::new(),
        }
    }

    async fn wait_started(&self) {
        self.started.notified().await;
    }

    fn block_until_released(&self) {
        self.started.notify_one();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.released_cv.wait(released).unwrap();
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.released_cv.notify_all();
    }
}

struct CountingConcurrentGenerator {
    calls: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    concurrent_call_started: Option<Arc<tokio::sync::Notify>>,
    first_call_gate: Option<Arc<GenerationGate>>,
    gated_chunk: Option<ChunkPos>,
    gate_first_call: AtomicBool,
}

impl ChunkGenerator for CountingConcurrentGenerator {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(active, Ordering::AcqRel);
        if active > 1
            && let Some(concurrent_call_started) = self.concurrent_call_started.as_ref()
        {
            concurrent_call_started.notify_one();
        }
        if self.gated_chunk.is_none_or(|chunk| chunk == pos)
            && self.gate_first_call.swap(false, Ordering::AcqRel)
            && let Some(gate) = self.first_call_gate.as_ref()
        {
            gate.block_until_released();
        }
        self.active.fetch_sub(1, Ordering::AcqRel);
        Chunk::empty(
            pos,
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}

fn test_biome_registry() -> Registry {
    Registry {
        id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
        entries: vec![Identifier::parse("minecraft:plains").unwrap()],
    }
}

async fn drive_stream_to_completion<W>(
    stream: &mut ChunkStreamState,
    writer: &mut W,
    light_cache: &mut LightCache,
    timeout: Duration,
    failure: &'static str,
) where
    W: AsyncWriteExt + Unpin,
{
    let progress_notify = stream.progress_notify();
    tokio::time::timeout(timeout, async {
        loop {
            let progress = progress_notify.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            if stream.step(writer, light_cache).await.unwrap() == ChunkStreamStep::Complete {
                return;
            }
            if !stream.has_immediate_work() {
                progress.await;
            }
        }
    })
    .await
    .expect(failure);
}

fn air_block_registry() -> BlockRegistry {
    BlockRegistry::from_report(&[BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: 0,
            default: true,
            properties: BTreeMap::new(),
        }],
    }])
    .expect("air registry builds")
}

#[test]
fn herd_surface_prefers_grass_before_fallbacks() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let air = BlockStateId(0);
    let grass = BlockStateId(1);
    let dirt = BlockStateId(2);
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, plains);
    chunk.set_block(8, 63, 8, dirt).unwrap();
    chunk.set_block(8, 64, 8, grass).unwrap();

    assert_eq!(
        herd_surface_y(&chunk, 8, 8, grass, &[dirt], &[air]),
        Some((64, grass))
    );
}

#[test]
fn herd_surface_fallback_allows_explicit_generated_land() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let air = BlockStateId(0);
    let grass = BlockStateId(1);
    let dirt = BlockStateId(2);
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, plains);
    chunk.set_block(8, 64, 8, dirt).unwrap();
    chunk
        .highest_opaque
        .set(8, 8, (64 - mc_world::MIN_Y + 1) as u32);

    assert_eq!(
        herd_surface_y(&chunk, 8, 8, grass, &[dirt], &[air]),
        Some((64, dirt))
    );
}

#[test]
fn herd_surface_fallback_rejects_unnatural_tops() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let air = BlockStateId(0);
    let grass = BlockStateId(1);
    let dirt = BlockStateId(2);
    for rejected in [BlockStateId(3), BlockStateId(4), BlockStateId(5)] {
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, plains.clone());
        chunk.set_block(8, 64, 8, rejected).unwrap();
        chunk
            .highest_opaque
            .set(8, 8, (64 - mc_world::MIN_Y + 1) as u32);

        assert_eq!(herd_surface_y(&chunk, 8, 8, grass, &[dirt], &[air]), None);
    }
    assert!(!passive_herd_fallback_surface_name("minecraft:stone"));
    assert!(!passive_herd_fallback_surface_name("minecraft:oak_planks"));
    assert!(!passive_herd_fallback_surface_name("minecraft:oak_leaves"));
}

#[test]
fn herd_surface_fallback_names_cover_natural_generated_land() {
    for name in [
        "minecraft:dirt",
        "minecraft:coarse_dirt",
        "minecraft:podzol",
        "minecraft:sand",
        "minecraft:red_sand",
        "minecraft:snow_block",
        "minecraft:moss_block",
        "minecraft:mycelium",
    ] {
        assert!(passive_herd_fallback_surface_name(name));
    }
}

#[test]
fn chunk_biome_lookup_uses_custom_geometry_and_rejects_out_of_bounds_y() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let bottom = Identifier::parse("minecraft:desert").unwrap();
    let top = Identifier::parse("minecraft:snowy_plains").unwrap();
    let geometry = mc_world::ChunkGeometry::new(0, 256).unwrap();
    let mut chunk =
        Chunk::empty_with_geometry(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains, geometry);
    chunk.biomes[0] = mc_world::BiomeSection::filled(bottom.clone());
    chunk.biomes[geometry.section_count() - 1] = mc_world::BiomeSection::filled(top.clone());

    assert_eq!(chunk_biome_at(&chunk, 0, -1, 0), None);
    assert_eq!(chunk_biome_at(&chunk, 0, 0, 0), Some(&bottom));
    assert_eq!(chunk_biome_at(&chunk, 15, 255, 15), Some(&top));
    assert_eq!(chunk_biome_at(&chunk, 15, 256, 15), None);
}

#[test]
fn chunk_biome_lookup_preserves_overworld_section_mapping() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let bottom = Identifier::parse("minecraft:desert").unwrap();
    let top = Identifier::parse("minecraft:snowy_plains").unwrap();
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains);
    let section_count = chunk.geometry().section_count();
    chunk.biomes[0] = mc_world::BiomeSection::filled(bottom.clone());
    chunk.biomes[section_count - 1] = mc_world::BiomeSection::filled(top.clone());

    assert_eq!(
        chunk_biome_at(&chunk, 0, chunk.geometry().min_y(), 0),
        Some(&bottom)
    );
    assert_eq!(
        chunk_biome_at(&chunk, 15, chunk.geometry().max_y() - 1, 15),
        Some(&top)
    );
}

#[test]
fn natural_sheep_color_weights_match_vanilla_26_1_2() {
    use mc_data::biomes::SheepColorClimate;
    use mc_entity::SheepColor;

    fn counts(climate: SheepColorClimate) -> [usize; 16] {
        let mut counts = [0; 16];
        for outer_roll in 0..100 {
            for common_roll in 0..500 {
                let color = sheep_color_for_rolls(climate, outer_roll, common_roll);
                counts[usize::from(color.id())] += 1;
            }
        }
        counts
    }

    let temperate = counts(SheepColorClimate::Temperate);
    assert_eq!(temperate[usize::from(SheepColor::White.id())], 40_918);
    assert_eq!(temperate[usize::from(SheepColor::Pink.id())], 82);
    assert_eq!(temperate[usize::from(SheepColor::LightGray.id())], 2_500);
    assert_eq!(temperate[usize::from(SheepColor::Gray.id())], 2_500);
    assert_eq!(temperate[usize::from(SheepColor::Brown.id())], 1_500);
    assert_eq!(temperate[usize::from(SheepColor::Black.id())], 2_500);

    let warm = counts(SheepColorClimate::Warm);
    assert_eq!(warm[usize::from(SheepColor::White.id())], 2_500);
    assert_eq!(warm[usize::from(SheepColor::Pink.id())], 82);
    assert_eq!(warm[usize::from(SheepColor::LightGray.id())], 2_500);
    assert_eq!(warm[usize::from(SheepColor::Gray.id())], 2_500);
    assert_eq!(warm[usize::from(SheepColor::Brown.id())], 40_918);
    assert_eq!(warm[usize::from(SheepColor::Black.id())], 1_500);

    let cold = counts(SheepColorClimate::Cold);
    assert_eq!(cold[usize::from(SheepColor::White.id())], 2_500);
    assert_eq!(cold[usize::from(SheepColor::Pink.id())], 82);
    assert_eq!(cold[usize::from(SheepColor::LightGray.id())], 2_500);
    assert_eq!(cold[usize::from(SheepColor::Gray.id())], 2_500);
    assert_eq!(cold[usize::from(SheepColor::Brown.id())], 1_500);
    assert_eq!(cold[usize::from(SheepColor::Black.id())], 40_918);
}

#[test]
fn hostile_template_plan_keeps_real_cave_opportunities() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let zombie = Identifier::parse("minecraft:zombie").unwrap();
    let rules = mc_data::biomes::BiomeSpawnRules::from_entries_with_sheep_color_climates(
        BTreeMap::from([(
            plains.clone(),
            BTreeMap::from([(
                "monster".to_string(),
                vec![mc_data::biomes::BiomeSpawnEntry {
                    entity_type: zombie,
                    min_count: 1,
                    max_count: 1,
                    weight: 100,
                }],
            )]),
        )]),
        BTreeSet::new(),
        BTreeSet::new(),
    );
    let entity_types = mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(
        &canonical_entity_type_report(),
    )
    .expect("canonical exact 26.1.2 entity type report builds");
    let air = BlockStateId(0);
    let stone = BlockStateId(1);
    let grass = BlockStateId(2);
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, plains);
    for lx in 0..16 {
        for lz in 0..16 {
            chunk.set_block(lx, 40, lz, stone).unwrap();
            chunk.set_block(lx, 43, lz, stone).unwrap();
            chunk.set_block(lx, 64, lz, grass).unwrap();
        }
    }

    let spawns = plan_passive_herd(
        &chunk,
        Some(grass),
        &[],
        None,
        &[air],
        &rules,
        &entity_types,
    );
    let hostiles = spawns
        .iter()
        .filter(|spawn| spawn.hostile)
        .collect::<Vec<_>>();

    assert_eq!(hostiles.len(), 1);
    assert!(
        hostiles.iter().any(|spawn| spawn.position.y < 64.0),
        "expected the hostile pack to retain a cave opportunity: {hostiles:?}"
    );
}

#[test]
fn prepared_cache_hit_drops_historical_cpu_and_encode_timings() {
    let prepared = PreparedChunkFrame {
        frame: Bytes::from_static(b"chunk-frame"),
        light: None,
        herd_spawns: Vec::new(),
        hydrated_campfires: Vec::new(),
        packet_data_len: 123,
        build_timing: ChunkBuildTiming {
            chunk_data_ms: 10,
            heightmap_ms: 11,
            light_compute_ms: 12,
            light_encode_ms: 13,
        },
        write_timing: ChunkWriteTiming {
            packet_encode_ms: 14,
            frame_ms: 15,
            socket_write_ms: 16,
            framed_bytes: 17,
        },
    };

    let cached = prepared.prepared_cache_hit();

    assert_eq!(cached.frame, prepared.frame);
    assert_eq!(cached.packet_data_len, prepared.packet_data_len);
    assert_eq!(cached.build_timing.chunk_data_ms, 0);
    assert_eq!(cached.build_timing.heightmap_ms, 0);
    assert_eq!(cached.build_timing.light_compute_ms, 0);
    assert_eq!(cached.build_timing.light_encode_ms, 0);
    assert_eq!(cached.write_timing.packet_encode_ms, 0);
    assert_eq!(cached.write_timing.frame_ms, 0);
    assert_eq!(cached.write_timing.socket_write_ms, 0);
    assert_eq!(cached.write_timing.framed_bytes, 17);
}

#[tokio::test]
async fn chunk_emit_registers_templates_without_one_shot_materialization() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(8);
    let desired = desired_chunk_set(0, 0, 0);
    let (session_id, _) = sessions.register(
        &LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "spawn-policy-player".to_owned(),
        },
        (0, 0),
        0,
        desired,
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let (simulation, _simulation_owner) =
        crate::play::simulation::simulation_channel_with_capacity(1);
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    )
    .with_simulation(simulation);
    let request = stream.scheduler.poll_next().expect("chunk request");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"chunk-frame"),
            light: None,
            herd_spawns: vec![
                HerdSpawn {
                    chunk: (0, 0),
                    slot: 0,
                    entity_type_id: 1,
                    entity_type_name: "minecraft:cow".to_owned(),
                    position: Vec3::new(1.5, DEFAULT_SPAWN_Y, 1.5),
                    hostile: false,
                    sheep_color: None,
                },
                HerdSpawn {
                    chunk: (0, 0),
                    slot: 1,
                    entity_type_id: 2,
                    entity_type_name: "minecraft:zombie".to_owned(),
                    position: Vec3::new(2.5, DEFAULT_SPAWN_Y, 2.5),
                    hostile: true,
                    sheep_color: None,
                },
            ],
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        })),
    });
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    assert_eq!(
        stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .unwrap(),
        EmitReadyResult::SentPacket
    );
    assert!(sessions.persisted_entity_records().is_empty());
    sessions.set_world_time_and_update_sleep(NIGHT_START_TICK);
    let mut scheduler = crate::play::NaturalSpawnScheduler::default();
    let (report, dispatches) = sessions.tick_periodic_natural_spawning(
        &mut scheduler,
        crate::play::NaturalSpawnTickInput {
            tick: 1,
            policy: crate::play::RandomTickPolicy {
                simulation_distance: 2,
                friendly_spawn_interval_ticks: 1,
                hostile_spawn_interval_ticks: 1,
                ..crate::play::RandomTickPolicy::default()
            },
            world_read: None,
            materials: None,
        },
    );
    assert!(dispatches.is_empty());
    assert_eq!(report.friendly.templates_considered, 1);
    assert_eq!(report.hostile.templates_considered, 1);
    assert_eq!(report.friendly.rejected_player_distance, 1);
    assert_eq!(report.hostile.rejected_player_distance, 1);
    assert!(sessions.persisted_entity_records().is_empty());
}

#[tokio::test]
async fn dispatch_defers_globally_in_flight_chunk_until_cache_lands() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "claim-waiter".to_string(),
    };
    let desired = desired_chunk_set(0, 0, 0);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        desired,
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 1,
        chunk_result_queue_size: 1,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    let claim = match sessions.prepared_chunk_or_claim((0, 0)) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected manual claim, got {other:?}"),
    };

    stream.dispatch_available().await;

    assert_eq!(stream.dispatched, 0);
    assert_eq!(stream.ready.len(), 0);
    assert_eq!(stream.scheduler.in_flight_len(), 0);
    assert_eq!(stream.scheduler.queued_len(), 1);

    sessions.cache_prepared_chunk(
        (0, 0),
        Arc::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"chunk-frame"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        }),
    );
    assert!(sessions.release_prepared_chunk_claim((0, 0), claim));

    stream.dispatch_available().await;

    assert_eq!(stream.dispatched, 1);
    assert_eq!(stream.ready.len(), 1);
    assert_eq!(stream.scheduler.in_flight_len(), 1);
}

#[tokio::test]
async fn same_spawn_waiters_do_not_rescan_full_inflight_window() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let view_distance = 8;
    let waiter_count = 20usize;
    let desired = desired_chunk_set(0, 0, view_distance);
    assert_eq!(desired.len(), 289);

    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: desired.len(),
        chunk_result_queue_size: 1,
        ..ChunkPipelinePolicy::default()
    };
    let mut streams = Vec::with_capacity(waiter_count);
    for waiter in 0..waiter_count {
        let (tx, _rx) = mpsc::channel(1);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(waiter as u128 + 1),
            name: format!("claim-waiter-{waiter}"),
        };
        let (session_id, _) = sessions.register(
            &profile,
            (0, 0),
            view_distance,
            desired.clone(),
            tx,
            PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
        );
        streams.push(ChunkStreamState::new(
            Arc::clone(&world),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            Arc::clone(&sessions),
            session_id,
            0,
            0,
            0.0,
            view_distance,
            ChunkPipelineResources::with_limits(1, 1),
            policy,
        ));
    }

    for &chunk in &desired {
        match sessions.prepared_chunk_or_claim(chunk) {
            PreparedChunkClaimResult::Claimed(_) => {}
            other => panic!("expected manual prepared claim for {chunk:?}, got {other:?}"),
        }
    }

    let before = sessions.prepared_chunk_claim_call_count();
    for stream in &mut streams {
        stream.dispatch_available().await;
        assert_eq!(stream.dispatched, 0);
        assert_eq!(stream.ready.len(), 0);
        assert_eq!(stream.scheduler.in_flight_len(), 0);
        assert_eq!(stream.scheduler.queued_len(), desired.len());
    }
    let after = sessions.prepared_chunk_claim_call_count();
    let delta = after.saturating_sub(before);
    let full_rescan_probes = waiter_count as u64 * desired.len() as u64;

    assert!(
        delta <= waiter_count as u64 * 2,
        "same-spawn waiters should stop after bounded in-flight probes; \
             claim_probe_count delta={delta}, bounded={} full_rescan={full_rescan_probes}",
        waiter_count * 2
    );
}

#[tokio::test]
async fn mixed_inflight_prefix_rotates_to_later_cached_chunk() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "mixed-claim-waiter".to_string(),
    };
    let view_distance = 1;
    let desired = desired_chunk_set(0, 0, view_distance);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        view_distance,
        desired,
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 1,
        chunk_result_queue_size: 1,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        view_distance,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    let queued: Vec<_> = prioritized_spiral(0, 0, view_distance, 0.0)
        .take(3)
        .map(|(cx, cz, _)| (cx, cz))
        .collect();
    let first_inflight = queued[0];
    let second_inflight = queued[1];
    let cached_later = queued[2];
    let first_claim = match sessions.prepared_chunk_or_claim(first_inflight) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected first manual claim, got {other:?}"),
    };
    let second_claim = match sessions.prepared_chunk_or_claim(second_inflight) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected second manual claim, got {other:?}"),
    };
    sessions.cache_prepared_chunk(
        cached_later,
        Arc::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"chunk-frame"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        }),
    );

    for _ in 0..=PREPARED_IN_FLIGHT_DEFERRAL_LIMIT {
        stream.dispatch_available().await;
        if stream
            .ready
            .values()
            .any(|result| (result.request.chunk_x, result.request.chunk_z) == cached_later)
        {
            break;
        }
    }

    let ready_chunks: Vec<_> = stream
        .ready
        .values()
        .map(|result| (result.request.chunk_x, result.request.chunk_z))
        .collect();
    assert_eq!(ready_chunks, vec![cached_later]);
    assert_eq!(stream.dispatched, 1);
    assert_eq!(stream.scheduler.in_flight_len(), 1);
    assert!(sessions.release_prepared_chunk_claim(first_inflight, first_claim));
    assert!(sessions.release_prepared_chunk_claim(second_inflight, second_claim));
}

#[tokio::test]
async fn stale_prepare_result_releases_global_claim() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        1,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );
    let claim = match sessions.prepared_chunk_or_claim((0, 0)) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected manual claim, got {other:?}"),
    };
    let stale_request = ChunkRequest {
        chunk_x: 0,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(stream.scheduler.current_generation().0 + 1),
    };

    stream.accept_result(ChunkPrepareResult {
        request: stale_request,
        prepare_claim: Some(PreparedChunkFence::Claimed(claim)),
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Absent,
    });

    let replacement = match sessions.prepared_chunk_or_claim((0, 0)) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected released stale claim, got {other:?}"),
    };
    assert!(sessions.release_prepared_chunk_claim((0, 0), replacement));
}

#[tokio::test]
async fn invalidation_during_socket_write_requeues_prepared_result() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        1,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );
    let request = stream.scheduler.poll_next().expect("chunk request");
    let claim = match sessions.prepared_chunk_or_claim((0, 0)) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected manual claim, got {other:?}"),
    };
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: Some(PreparedChunkFence::Claimed(claim)),
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"stale-frame"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        })),
    });
    let mut writer = InvalidatePreparedOnWrite {
        sessions: Arc::clone(&sessions),
        chunk: (0, 0),
        written: 0,
    };
    let mut light_cache = LightCache::new();

    assert_eq!(
        stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .unwrap(),
        EmitReadyResult::Blocked
    );
    assert_eq!(writer.written, b"stale-frame".len());
    assert_eq!(stream.emitted, 0);
    assert!(!stream.loaded.contains(&(0, 0)));
    assert_eq!(stream.scheduler.queued_len(), 1);
}

#[tokio::test]
async fn dropping_stream_releases_ready_prepare_claim() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let resources = ChunkPipelineResources::with_limits(1, 1);
    let metrics = resources.metrics();
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        1,
        0,
        0,
        0.0,
        0,
        resources,
        ChunkPipelinePolicy::default(),
    );
    let request = stream.scheduler.poll_next().expect("chunk request");
    let claim = match sessions.prepared_chunk_or_claim((0, 0)) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected manual claim, got {other:?}"),
    };
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: Some(PreparedChunkFence::Claimed(claim)),
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Absent,
    });
    assert!(matches!(
        sessions.prepared_chunk_or_claim((0, 0)),
        PreparedChunkClaimResult::InFlight
    ));

    drop(stream);

    let cancellation = metrics.cancellation_snapshot();
    assert_eq!(cancellation.cancelled_streams, 1);
    assert_eq!(cancellation.cancelled_requests, 1);

    let replacement = match sessions.prepared_chunk_or_claim((0, 0)) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected released ready claim, got {other:?}"),
    };
    assert!(sessions.release_prepared_chunk_claim((0, 0), replacement));
}

#[tokio::test]
async fn dropping_stream_releases_buffered_prepare_claim() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        1,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );
    let request = stream.scheduler.poll_next().expect("chunk request");
    let claim = match sessions.prepared_chunk_or_claim((0, 0)) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected manual claim, got {other:?}"),
    };
    stream
        .result_tx
        .try_send(ChunkPrepareResult {
            request,
            prepare_claim: Some(PreparedChunkFence::Claimed(claim)),
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        })
        .expect("buffer prepare result before stream drop");
    assert!(matches!(
        sessions.prepared_chunk_or_claim((0, 0)),
        PreparedChunkClaimResult::InFlight
    ));

    drop(stream);

    let replacement = match sessions.prepared_chunk_or_claim((0, 0)) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected released buffered claim, got {other:?}"),
    };
    assert!(sessions.release_prepared_chunk_claim((0, 0), replacement));
}

#[tokio::test]
async fn stale_prepare_request_records_rejection_before_world_work() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let resources = ChunkPipelineResources::with_limits(1, 1);
    let metrics = resources.metrics();
    let request = ChunkRequest {
        chunk_x: 0,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };

    let result = crate::resource_profile::measure_future(
        crate::resource_profile::CpuStage::ChunkPreparation,
        prepare_chunk_request(
            request,
            Arc::clone(&world),
            None,
            None,
            Arc::new(test_biome_registry()),
            registry,
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            resources,
            None,
            Arc::new(AtomicU64::new(2)),
            Arc::new(tokio::sync::Notify::new()),
            0,
            None,
        ),
    )
    .await;

    assert!(matches!(result.outcome, ChunkPrepareOutcome::Absent));
    assert_eq!(
        metrics.cancellation_snapshot(),
        crate::ChunkPipelineCancellationSnapshot {
            stale_results_rejected: 1,
            ..crate::ChunkPipelineCancellationSnapshot::default()
        }
    );
    assert_eq!(world.lock().await.cache_len(), 0);
}

#[test]
fn build_chunk_packet_uses_baked_section_light_without_recompute() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let mut centre = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains);
    centre.set_block(0, 0, 0, BlockStateId(1)).unwrap();
    let mut sky = vec![0; mc_world::chunk::LIGHT_LAYER_BYTES];
    let mut block = vec![0; mc_world::chunk::LIGHT_LAYER_BYTES];
    sky[0] = 0x21;
    block[0] = 0x43;
    centre.section_lights[0].sky =
        Some(mc_world::LightSection::from_bytes(sky.try_into().unwrap()));
    centre.section_lights[0].block = Some(mc_world::LightSection::from_bytes(
        block.try_into().unwrap(),
    ));
    let neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
        std::array::from_fn(|_| std::array::from_fn(|_| None));
    let table = BlockLightTable::from_arrays("test", vec![0, 0], vec![0, 15], vec![true, false]);
    let mut workspace = LightWorkspace::new();

    let built = build_chunk_packet(
        &centre,
        &neighbourhood,
        &test_biome_registry(),
        &air_block_registry(),
        &ItemRegistry::from_report(&[]),
        &TagsData::default(),
        &[],
        &mc_data::block_entity_types::BlockEntityTypeRegistry::default(),
        Some(&table),
        None,
        &[],
        &[],
        &[],
        &mc_data::biomes::BiomeSpawnRules::default(),
        &mc_data::entity_types::solaris_required_entity_types(),
        Some(&mut workspace),
        0,
        0,
        None,
    )
    .expect("chunk packet builds from baked light");

    let light = built
        .light
        .expect("baked light should populate the play light cache");
    assert_eq!(built.timing.light_compute_ms, 0);
    assert_eq!(light.sky.section(0).unwrap().byte(0), 0x21);
    assert_eq!(light.block.section(0).unwrap().byte(0), 0x43);
}

#[test]
fn build_chunk_packet_projects_loader_state_into_chunk_palette() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let mut centre = Chunk::empty_with_geometry(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        plains,
        mc_world::ChunkGeometry::new(0, 16).unwrap(),
    );
    centre.sections[0] = mc_world::ChunkSection::filled(BlockStateId(1), BlockStateId(0));
    let projection = crate::loader::LoaderBlockProjection::for_test(1, 42);
    let neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
        std::array::from_fn(|_| std::array::from_fn(|_| None));

    let built = build_chunk_packet(
        &centre,
        &neighbourhood,
        &test_biome_registry(),
        &air_block_registry(),
        &ItemRegistry::from_report(&[]),
        &TagsData::default(),
        &[],
        &mc_data::block_entity_types::BlockEntityTypeRegistry::default(),
        None,
        None,
        &[],
        &[],
        &[],
        &mc_data::biomes::BiomeSpawnRules::default(),
        &mc_data::entity_types::solaris_required_entity_types(),
        None,
        0,
        0,
        Some(&projection),
    )
    .unwrap();

    assert_eq!(built.packet.data[4], 0, "single-value block palette");
    assert_eq!(built.packet.data[5], 42, "session carrier state");
}

#[tokio::test]
async fn prepared_computed_light_is_published_for_later_readers() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16);
    for z in -1..=1 {
        for x in -1..=1 {
            let pos = ChunkPos { x, z };
            storage
                .insert_generated_chunk(pos, Chunk::empty(pos, BlockStateId(0), biome.clone()))
                .unwrap();
        }
    }
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let world = Arc::new(Mutex::new(storage));
    let request = ChunkRequest {
        chunk_x: 0,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };

    let result = crate::resource_profile::measure_future(
        crate::resource_profile::CpuStage::ChunkPreparation,
        prepare_chunk_request(
            request,
            Arc::clone(&world),
            Some(world_read.clone()),
            Some(world_mutation),
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            Some(Arc::new(BlockLightTable::from_arrays(
                "test",
                vec![0],
                vec![0],
                vec![true],
            ))),
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            ChunkPipelineResources::with_limits(1, 1),
            None,
            Arc::new(AtomicU64::new(1)),
            Arc::new(tokio::sync::Notify::new()),
            0,
            None,
        ),
    )
    .await;

    assert!(matches!(result.outcome, ChunkPrepareOutcome::Ready(_)));
    let published = world_read
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }])
        .chunk(ChunkPos { x: 0, z: 0 })
        .expect("prepared centre remains published");
    assert!(
        ChunkLight::from_section_lights(&published.section_lights).is_some(),
        "computed light must become the shared baked snapshot"
    );
}

#[tokio::test]
async fn computed_light_publish_allows_light_only_changes_but_rejects_blocks() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16);
    let mut positions = Vec::new();
    for z in -1..=1 {
        for x in -1..=1 {
            let pos = ChunkPos { x, z };
            positions.push(pos);
            storage
                .insert_generated_chunk(pos, Chunk::empty(pos, BlockStateId(0), biome.clone()))
                .unwrap();
        }
    }
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let snapshot = world_read.snapshot_chunks(&positions);
    let sources = std::array::from_fn(|dz| {
        std::array::from_fn(|dx| {
            snapshot.chunk(ChunkPos {
                x: dx as i32 - 1,
                z: dz as i32 - 1,
            })
        })
    });
    let world = Arc::new(Mutex::new(storage));

    world
        .lock()
        .await
        .set_baked_light(ChunkPos { x: 1, z: 0 }, &ChunkLight::filled(15, 0))
        .unwrap();

    assert!(
        publish_computed_light_if_sources_current(
            &world,
            Some(&world_read),
            Some(&world_mutation),
            ChunkPos { x: 0, z: 0 },
            &sources,
            &ChunkLight::filled(15, 0),
        )
        .await
    );
    let centre = world_read
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }])
        .chunk(ChunkPos { x: 0, z: 0 })
        .expect("centre remains published");
    assert!(ChunkLight::from_section_lights(&centre.section_lights).is_some());

    let snapshot = world_read.snapshot_chunks(&positions);
    let block_sources = std::array::from_fn(|dz| {
        std::array::from_fn(|dx| {
            snapshot.chunk(ChunkPos {
                x: dx as i32 - 1,
                z: dz as i32 - 1,
            })
        })
    });
    world
        .lock()
        .await
        .set_block_at(mc_world::BlockPos { x: 16, y: 64, z: 0 }, BlockStateId(1))
        .unwrap();
    assert!(
        !publish_computed_light_if_sources_current(
            &world,
            Some(&world_read),
            Some(&world_mutation),
            ChunkPos { x: 0, z: 0 },
            &block_sources,
            &ChunkLight::filled(15, 0),
        )
        .await
    );
}

#[test]
fn missing_neighbour_planner_returns_cached_and_load_sources() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let centre = ChunkPos { x: 0, z: 0 };
    let cached_neighbour = ChunkPos { x: 1, z: 0 };
    let mut storage = WorldStorage::in_memory_with_capacity(registry, 16);
    for position in [centre, cached_neighbour] {
        storage
            .insert_generated_chunk(
                position,
                Chunk::empty(position, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }
    let mut neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
        std::array::from_fn(|_| std::array::from_fn(|_| None));
    neighbourhood[1][1] = storage.cached_chunk_snapshot(centre);

    let plans = plan_missing_neighbour_snapshots(&storage, centre, &neighbourhood);

    assert_eq!(plans.len(), 8);
    assert_eq!(
        plans
            .iter()
            .filter(|plan| matches!(plan.source, NeighbourSnapshotSource::Cached(_)))
            .count(),
        1
    );
    assert!(plans.iter().any(|plan| {
        plan.position == cached_neighbour
            && matches!(plan.source, NeighbourSnapshotSource::Cached(_))
    }));
    assert_eq!(
        plans
            .iter()
            .filter(|plan| matches!(plan.source, NeighbourSnapshotSource::Load(_)))
            .count(),
        7
    );
}

#[tokio::test]
async fn cached_neighbourhood_snapshot_does_not_wait_for_world_writer() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16);
    for z in -1..=1 {
        for x in -1..=1 {
            let pos = ChunkPos { x, z };
            storage
                .insert_generated_chunk(pos, Chunk::empty(pos, BlockStateId(0), biome.clone()))
                .unwrap();
        }
    }
    let world_read = storage.read_view();
    let world = Arc::new(Mutex::new(storage));
    let request = ChunkRequest {
        chunk_x: 0,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };
    let _writer = world.lock().await;

    let loaded = tokio::time::timeout(
        Duration::from_secs(1),
        load_chunk_neighbourhood(
            Arc::clone(&world),
            Some(world_read),
            0,
            0,
            ChunkPipelineResources::with_limits(1, 1),
            request,
            Arc::new(AtomicU64::new(1)),
            true,
        ),
    )
    .await
    .expect("cached immutable snapshot must not wait for world writer")
    .unwrap();

    assert!(loaded.centre.is_some());
    assert!(loaded.neighbourhood.iter().flatten().all(Option::is_some));
    assert_eq!(loaded.staged.len(), 9);
}

#[tokio::test]
async fn generated_prepare_budget_classification_does_not_wait_for_world_writer() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16).with_generator(
        Arc::new(CountingGenerator {
            calls: Arc::clone(&calls),
        }),
    );
    let world_read = storage.read_view();
    let chunk_source = storage.chunk_source_view();
    let world = Arc::new(Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "source-classifier".to_string(),
    };
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        desired_chunk_set(0, 0, 0),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        sessions,
        session_id,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    )
    .with_world_read(Some(world_read))
    .with_chunk_source(Some(chunk_source));
    let request = ChunkRequest {
        chunk_x: 4,
        chunk_z: -2,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };
    let writer = world.lock().await;

    let class = tokio::time::timeout(
        Duration::from_secs(1),
        stream.classify_prepare_budget(request),
    )
    .await
    .expect("prepare budget classification waited for the world writer");
    drop(writer);

    assert_eq!(class, ChunkPrepareBudgetClass::Generate);
    assert_eq!(calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn published_cache_pressure_check_does_not_wait_for_world_writer() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1)
        .with_generator(Arc::new(CountingGenerator {
            calls: Arc::new(AtomicUsize::new(0)),
        }));
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let world_read = storage.read_view();
    let world = Arc::new(Mutex::new(storage));
    let writer = world.lock().await;

    let can_cache = tokio::time::timeout(
        Duration::from_secs(1),
        chunk_prepare_can_cache(
            &world,
            Some(&world_read),
            ChunkPos { x: 1, z: 0 },
            "test published cache pressure",
        ),
    )
    .await
    .expect("published pressure check waited for world writer");
    drop(writer);

    assert!(!can_cache);
}

#[tokio::test]
async fn dirty_pressure_defers_generated_stream_chunk_instead_of_absent() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1)
        .with_generator(Arc::new(CountingGenerator {
            calls: Arc::clone(&calls),
        }));
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let world = Arc::new(Mutex::new(storage));
    let request = ChunkRequest {
        chunk_x: 1,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };

    let loaded = load_chunk_neighbourhood(
        Arc::clone(&world),
        None,
        1,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        request,
        Arc::new(AtomicU64::new(1)),
        false,
    )
    .await
    .unwrap();

    assert!(loaded.centre.is_none());
    assert_eq!(loaded.staged, vec![(0, 0)]);
    assert!(loaded.backpressured);
    assert_eq!(world.lock().await.cache_len(), 1);
    assert_eq!(calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn absent_disk_chunk_counts_absent_even_under_dirty_pressure_without_generator() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1);
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let world = Arc::new(Mutex::new(storage));
    let request = ChunkRequest {
        chunk_x: 1,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };

    let loaded = load_chunk_neighbourhood(
        Arc::clone(&world),
        None,
        1,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        request,
        Arc::new(AtomicU64::new(1)),
        false,
    )
    .await
    .unwrap();

    assert!(loaded.centre.is_none());
    assert_eq!(loaded.staged, vec![(0, 0)]);
    assert!(!loaded.backpressured);
    assert_eq!(world.lock().await.cache_len(), 1);
}

#[tokio::test]
async fn corrupt_saved_chunk_fails_without_generator_fallback() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempfile::tempdir().unwrap();
    let region_root = temp.path().join("region");
    std::fs::create_dir_all(&region_root).unwrap();
    let region_path = region_root.join("r.0.0.mca");
    let corrupt = b"corrupt region";
    std::fs::write(&region_path, corrupt).unwrap();
    let storage = WorldStorage::open_with_capacity(temp.path(), Arc::clone(&registry), 16)
        .unwrap()
        .with_generator(Arc::new(CountingGenerator {
            calls: Arc::clone(&calls),
        }));
    let world = Arc::new(Mutex::new(storage));
    let request = ChunkRequest {
        chunk_x: 0,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };

    let result = load_chunk_neighbourhood(
        world,
        None,
        0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        request,
        Arc::new(AtomicU64::new(1)),
        false,
    )
    .await;
    let error = match result {
        Ok(_) => panic!("corrupt saved chunk was replaced by generator fallback"),
        Err(error) => error,
    };

    assert!(error.contains("chunk read failed"), "{error}");
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert_eq!(std::fs::read(region_path).unwrap(), corrupt);
}

#[tokio::test]
async fn neighbour_dirty_pressure_backpressures_full_neighbourhood() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1)
        .with_generator(Arc::new(CountingGenerator {
            calls: Arc::clone(&calls),
        }));
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let world = Arc::new(Mutex::new(storage));
    let request = ChunkRequest {
        chunk_x: 0,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };

    let loaded = load_chunk_neighbourhood(
        Arc::clone(&world),
        None,
        0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        request,
        Arc::new(AtomicU64::new(1)),
        true,
    )
    .await
    .unwrap();

    assert!(loaded.centre.is_some());
    assert!(loaded.neighbourhood[1][1].is_some());
    assert_eq!(loaded.staged, vec![(0, 0)]);
    assert!(loaded.backpressured);
    assert_eq!(calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn stream_step_retains_backpressured_chunk_without_absent_success() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1)
        .with_generator(Arc::new(CountingGenerator {
            calls: Arc::clone(&calls),
        }));
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let world = Arc::new(Mutex::new(storage));
    let policy = ChunkPipelinePolicy {
        chunk_send_rate: 1,
        chunk_prepare_batch_size: 1,
        chunk_result_queue_size: 1,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        1,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    let request = stream.scheduler.poll_next().expect("request");
    let result = crate::resource_profile::measure_future(
        crate::resource_profile::CpuStage::ChunkPreparation,
        prepare_chunk_request(
            request,
            Arc::clone(&world),
            None,
            None,
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            ChunkPipelineResources::with_limits(1, 1),
            None,
            Arc::clone(&stream.active_generation),
            Arc::clone(&stream.generation_changed),
            0,
            None,
        ),
    )
    .await;
    assert!(matches!(result.outcome, ChunkPrepareOutcome::Backpressured));
    assert_eq!(result.pressure_flush.runs, 0);
    assert_eq!(result.pressure_flush.planned_chunks, 0);
    assert_eq!(result.pressure_flush.flushed_chunks, 0);
    stream.accept_result(result);
    assert_eq!(
        stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .unwrap(),
        EmitReadyResult::Blocked
    );

    assert!(!stream.is_complete());
    assert!(stream.pressure_retries.contains_key(&(1, 0)));
    assert_eq!(stream.absent, 0);
    assert_eq!(stream.emitted, 0);
    assert_eq!(stream.pressure_abandoned, 0);
    assert!(stream.staged.is_empty());
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert_eq!(world.lock().await.cache_len(), 1);
}

#[tokio::test]
async fn backpressured_result_counts_staged_chunks_without_marking_success() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        1,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );
    let request = stream.scheduler.poll_next().expect("request");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming {
            runs: 1,
            planned_chunks: 2,
            flushed_chunks: 1,
            plan_ms: 3,
            write_ms: 5,
            commit_ms: 7,
        },
        staged: vec![(0, 0)],
        outcome: ChunkPrepareOutcome::Backpressured,
    });
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    assert_eq!(
        stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .unwrap(),
        EmitReadyResult::Blocked
    );

    assert_eq!(stream.pressure_staged_count(), 1);
    assert!(stream.pressure_staged_contains((0, 0)));
    assert_eq!(stream.pressure_abandoned, 0);
    assert_eq!(stream.pressure_flush_runs, 1);
    assert_eq!(stream.pressure_flush_planned_chunks, 2);
    assert_eq!(stream.pressure_flush_flushed_chunks, 1);
    assert_eq!(stream.pressure_flush_plan_ms, 3);
    assert_eq!(stream.pressure_flush_write_ms, 5);
    assert_eq!(stream.pressure_flush_commit_ms, 7);
    assert_eq!(stream.max_pressure_flush_plan_ms, 3);
    assert_eq!(stream.max_pressure_flush_write_ms, 5);
    assert_eq!(stream.max_pressure_flush_commit_ms, 7);
    assert_eq!(stream.absent, 0);
    assert_eq!(stream.emitted, 0);
    assert!(stream.staged.is_empty());
    assert_eq!(stream.scheduler.queued_len(), 1);
    assert_eq!(stream.scheduler.finished_len(), 0);

    for _ in 1..CHUNK_BACKPRESSURE_MAX_RETRIES {
        let request = stream.scheduler.poll_next().expect("deferred request");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: vec![(0, 0)],
            outcome: ChunkPrepareOutcome::Backpressured,
        });
        let _ = stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .unwrap();
    }

    assert_eq!(stream.pressure_abandoned, 1);
    assert_eq!(stream.scheduler.finished_len(), 1);
}

#[tokio::test]
async fn backpressured_result_is_not_send_progress() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        1,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );
    let request = stream.scheduler.poll_next().expect("request");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: vec![(0, 0)],
        outcome: ChunkPrepareOutcome::Backpressured,
    });
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    assert_eq!(stream.yielded_turns, 0);
    assert_eq!(
        stream.step(&mut writer, &mut light_cache).await.unwrap(),
        ChunkStreamStep::Progress
    );

    assert_eq!(stream.yielded_turns, 1);
    assert_eq!(stream.emitted, 0);
    assert_eq!(stream.scheduler.queued_len(), 1);
    assert_eq!(stream.scheduler.finished_len(), 0);

    assert!(stream.replan_center(1, 0, 45.0).is_empty());
    assert!(stream.pressure_retries.contains_key(&(1, 0)));
    assert!(stream.pressure_staged_contains((0, 0)));
}

#[tokio::test]
async fn backpressured_request_redispatches_without_turn_delay() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 2,
        chunk_result_queue_size: 2,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    let request = stream.scheduler.poll_next().expect("request");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Backpressured,
    });
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();
    assert_eq!(
        stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .unwrap(),
        EmitReadyResult::Blocked
    );

    stream.dispatch_available().await;

    assert_eq!(stream.scheduler.in_flight_len(), 1);
}

#[tokio::test]
async fn absent_result_drains_without_counting_as_backpressure_yield() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );
    let request = stream.scheduler.poll_next().expect("request");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Absent,
    });
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    assert_eq!(
        stream.step(&mut writer, &mut light_cache).await.unwrap(),
        ChunkStreamStep::Complete
    );

    assert_eq!(stream.absent, 1);
    assert_eq!(stream.emitted, 0);
    assert_eq!(stream.yielded_turns, 0);
    assert_eq!(stream.pressure_abandoned, 0);
}

#[tokio::test]
async fn non_packet_results_do_not_consume_ready_batch_send_budget() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let policy = ChunkPipelinePolicy {
        chunk_send_rate: 4,
        chunk_result_queue_size: 4,
        ..ChunkPipelinePolicy::default()
    };
    let resources = ChunkPipelineResources::with_limits(1, 1);
    let metrics = resources.metrics();
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        1,
        resources,
        policy,
    );
    for _ in 0..2 {
        let request = stream.scheduler.poll_next().expect("queued chunk");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        });
    }
    for _ in 0..2 {
        let request = stream.scheduler.poll_next().expect("queued chunk");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"prepared-frame"),
                light: None,
                herd_spawns: Vec::new(),
                hydrated_campfires: Vec::new(),
                packet_data_len: 1,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            })),
        });
    }
    stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
        view_distance: 1,
        chunk_send_rate: 1,
        chunk_load_rate: 64,
        chunk_generate_rate: 32,
    });
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    assert!(
        stream
            .emit_ready_batch(&mut writer, &mut light_cache)
            .await
            .unwrap()
    );

    assert_eq!(stream.absent, 2);
    assert_eq!(stream.emitted, 1);
    assert_eq!(stream.ready.len(), 1);
    assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::SendBudget);
    assert_eq!(metrics.stop_reason_counts().send_budget, 1);
    assert!(
        metrics
            .observed_stop_reasons()
            .contains(&ChunkPipelineStopReason::SendBudget)
    );
}

#[tokio::test(start_paused = true)]
async fn sustained_runtime_control_queue_pressure_scales_live_send_budget() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let policy = ChunkPipelinePolicy {
        chunk_send_rate: 4,
        chunk_result_queue_size: 4,
        ..ChunkPipelinePolicy::default()
    };
    let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
        policy: crate::AutoscalePolicy {
            min_view_distance: 2,
            max_view_distance: 2,
            min_chunk_send_rate: 1,
            max_chunk_send_rate: 4,
            min_chunk_load_rate: 1,
            max_chunk_load_rate: 64,
            min_chunk_generate_rate: 1,
            max_chunk_generate_rate: 32,
            queue_pressure_percent: 75,
            scale_down_after_seconds: 60,
            ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
        },
        initial_limits: crate::RuntimeControlLimits {
            view_distance: 2,
            chunk_send_rate: 4,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
        },
    });
    let mut signals = control
        .take_signal_receiver()
        .expect("test owns runtime control receiver");
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        2,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    )
    .with_runtime_control(Some(control.clone()));
    for _ in 0..2 {
        let request = stream.scheduler.poll_next().expect("queued chunk");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"prepared-frame"),
                light: None,
                herd_spawns: Vec::new(),
                hydrated_campfires: Vec::new(),
                packet_data_len: 1,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            })),
        });
    }
    stream.observe_runtime_control();
    assert!(!stream.chunk_queue_saturated);
    let request = stream.scheduler.poll_next().expect("third queued chunk");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"prepared-frame"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 1,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        })),
    });
    stream.observe_runtime_control();
    assert!(stream.chunk_queue_saturated);
    let owner_decision = observe_next_runtime_control_signal(&control, &mut signals).await;
    assert_eq!(owner_decision.action, crate::AutoscaleAction::Hold);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(
        control.observe(healthy_runtime_control_input()).action,
        crate::AutoscaleAction::Hold
    );
    assert_eq!(control.snapshot().limits.chunk_send_rate, 4);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        control.observe(healthy_runtime_control_input()).action,
        crate::AutoscaleAction::ScaleDown
    );
    let request = stream.scheduler.poll_next().expect("fourth queued chunk");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"prepared-frame"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 1,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        })),
    });
    stream.observe_runtime_control();
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    assert!(
        stream
            .emit_ready_batch(&mut writer, &mut light_cache)
            .await
            .unwrap()
    );

    let snapshot = control.snapshot();
    assert_eq!(
        snapshot.last_decision.action,
        crate::AutoscaleAction::ScaleDown
    );
    assert_eq!(
        snapshot.last_decision.pressure,
        Some(crate::AutoscalePressure::ChunkQueue)
    );
    assert_eq!(snapshot.limits.chunk_send_rate, 3);
    assert_eq!(stream.emitted, 3);
    assert_eq!(stream.ready.len(), 1);
    assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::SendBudget);
}

#[tokio::test]
async fn center_replan_notifies_stalled_chunk_stream() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let mut stream = ChunkStreamState::new(
        world,
        Arc::new(test_biome_registry()),
        registry,
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );
    let notify = stream.progress_notify();
    assert!(
        tokio::time::timeout(Duration::from_millis(1), notify.notified())
            .await
            .is_err()
    );
    stream.last_stop_reason = ChunkPipelineStopReason::Complete;

    stream.replan_center(1, 0, 0.0);

    tokio::time::timeout(Duration::from_secs(1), notify.notified())
        .await
        .expect("replanned chunk stream must wake its play loop");
    assert!(stream.scheduler.queued_len() > 0);
}

#[test]
fn rotation_reprioritizes_without_cancelling_valid_chunk_work() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let mut stream = ChunkStreamState::new(
        world,
        Arc::new(test_biome_registry()),
        registry,
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        1,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );
    let ready_request = stream.scheduler.poll_next().expect("ready request");
    stream.accept_result(ChunkPrepareResult {
        request: ready_request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Absent,
    });
    let in_flight_request = stream.scheduler.poll_next().expect("in-flight request");
    let generation = stream.scheduler.current_generation();

    stream.replan_center(0, 0, 90.0);

    assert_eq!(stream.scheduler.current_generation(), generation);
    assert_eq!(stream.ready.len(), 1);
    assert_eq!(stream.scheduler.in_flight_len(), 2);
    assert!(stream.scheduler.is_current(ready_request));
    assert!(stream.scheduler.is_current(in_flight_request));
}

#[tokio::test]
async fn runtime_control_memory_admission_immediately_sheds_ready_and_in_flight_work() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
        crate::memory_pressure::MemoryPressureSnapshot {
            used_mb: 900,
            limit_mb: 1_000,
        },
    );
    let control = crate::RuntimeControlHandle::new_with_memory_pressure(
        crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy {
                min_view_distance: 16,
                max_view_distance: 16,
                min_chunk_send_rate: 1,
                max_chunk_send_rate: 4,
                min_chunk_load_rate: 1,
                max_chunk_load_rate: 64,
                min_chunk_generate_rate: 1,
                max_chunk_generate_rate: 32,
                queue_pressure_percent: 100,
                memory_pressure_percent: 50,
                ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
            },
            initial_limits: crate::RuntimeControlLimits {
                view_distance: 16,
                chunk_send_rate: 4,
                chunk_load_rate: 64,
                chunk_generate_rate: 32,
            },
        },
        memory_pressure,
    );
    let policy = ChunkPipelinePolicy {
        chunk_send_rate: 4,
        chunk_result_queue_size: 16,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        16,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    )
    .with_runtime_control(Some(control.clone()));
    let mut old_generation = None;
    for _ in 0..3 {
        let request = stream.scheduler.poll_next().expect("queued chunk");
        old_generation.get_or_insert(request.generation);
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        });
    }
    let active_request = stream.scheduler.poll_next().expect("active chunk");
    assert_eq!(Some(active_request.generation), old_generation);
    assert_eq!(stream.ready.len(), 3);
    assert_eq!(stream.scheduler.in_flight_len(), 4);

    stream.observe_runtime_control();
    control.observe(healthy_runtime_control_input());
    stream.observe_runtime_control();

    let snapshot = control.snapshot();
    assert_eq!(snapshot.last_decision.action, crate::AutoscaleAction::Hold);
    assert_eq!(snapshot.limits.view_distance, 16);
    assert_eq!(stream.view_distance, 16);
    assert_eq!(snapshot.limits.chunk_send_rate, 4);
    assert_eq!(
        snapshot.last_decision.pressure,
        Some(crate::AutoscalePressure::Memory)
    );
    assert_eq!(stream.ready.len(), 0);
    assert_eq!(stream.scheduler.in_flight_len(), 0);
    assert!(stream.scheduler.queued_len() >= 4);
    assert_eq!(stream.memory_pressure_shed_runs, 1);
    assert_eq!(stream.memory_pressure_shed_ready, 3);
    assert_eq!(stream.memory_pressure_shed_in_flight, 1);
    assert_eq!(
        stream.last_stop_reason,
        ChunkPipelineStopReason::MemoryPressure
    );

    let replayed = stream.scheduler.poll_next().expect("replayed request");
    assert_ne!(Some(replayed.generation), old_generation);
}

#[tokio::test]
async fn runtime_control_memory_admission_immediately_pauses_dispatch_until_pressure_clears() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
        crate::memory_pressure::MemoryPressureSnapshot {
            used_mb: 900,
            limit_mb: 1_000,
        },
    );
    let control = crate::RuntimeControlHandle::new_with_memory_pressure(
        crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy {
                min_view_distance: 16,
                max_view_distance: 16,
                min_chunk_send_rate: 1,
                max_chunk_send_rate: 4,
                min_chunk_load_rate: 1,
                max_chunk_load_rate: 64,
                min_chunk_generate_rate: 1,
                max_chunk_generate_rate: 32,
                memory_pressure_percent: 50,
                ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
            },
            initial_limits: crate::RuntimeControlLimits {
                view_distance: 16,
                chunk_send_rate: 4,
                chunk_load_rate: 64,
                chunk_generate_rate: 32,
            },
        },
        memory_pressure.clone(),
    );
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 2,
        chunk_result_queue_size: 4,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        16,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    )
    .with_runtime_control(Some(control));
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    stream.step(&mut writer, &mut light_cache).await.unwrap();

    assert_eq!(stream.scheduler.in_flight_len(), 0);
    assert_eq!(stream.view_distance, 16);
    assert_eq!(
        stream.last_stop_reason,
        ChunkPipelineStopReason::MemoryPressure
    );

    memory_pressure.set_sample(crate::memory_pressure::MemoryPressureSnapshot {
        used_mb: 100,
        limit_mb: 1_000,
    });
    stream.step(&mut writer, &mut light_cache).await.unwrap();

    assert!(stream.scheduler.in_flight_len() > 0);
    assert_ne!(
        stream.last_stop_reason,
        ChunkPipelineStopReason::MemoryPressure
    );
}

#[tokio::test(start_paused = true)]
async fn sustained_runtime_control_queue_pressure_replans_live_view_distance() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(8);
    let profile = crate::login::LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "RuntimeViewDistanceAlice".to_string(),
    };
    let old_desired = desired_chunk_set(0, 0, 3);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        3,
        old_desired,
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
        policy: crate::AutoscalePolicy {
            min_view_distance: 2,
            max_view_distance: 3,
            min_chunk_send_rate: 4,
            max_chunk_send_rate: 4,
            min_chunk_load_rate: 64,
            max_chunk_load_rate: 64,
            min_chunk_generate_rate: 32,
            max_chunk_generate_rate: 32,
            queue_pressure_percent: 1,
            scale_down_after_seconds: 60,
            ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
        },
        initial_limits: crate::RuntimeControlLimits {
            view_distance: 3,
            chunk_send_rate: 4,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
        },
    });
    let mut signals = control
        .take_signal_receiver()
        .expect("test owns runtime control receiver");
    let policy = ChunkPipelinePolicy {
        chunk_send_rate: 4,
        chunk_result_queue_size: 4,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        3,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    )
    .with_runtime_control(Some(control.clone()));
    for _ in 0..2 {
        let request = stream.scheduler.poll_next().expect("queued chunk");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        });
    }
    stream.loaded.insert((3, 0));
    sessions.mark_loaded(session_id, (3, 0));

    let (mut client, mut server) = tokio::io::duplex(256);
    let mut light_cache = LightCache::new();

    stream.step(&mut server, &mut light_cache).await.unwrap();
    let owner_decision = observe_next_runtime_control_signal(&control, &mut signals).await;
    assert_eq!(owner_decision.action, crate::AutoscaleAction::Hold);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(
        control.observe(healthy_runtime_control_input()).action,
        crate::AutoscaleAction::Hold
    );
    assert_eq!(control.snapshot().limits.view_distance, 3);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        control.observe(healthy_runtime_control_input()).action,
        crate::AutoscaleAction::ScaleDown
    );
    stream.step(&mut server, &mut light_cache).await.unwrap();

    let snapshot = control.snapshot();
    assert_eq!(
        snapshot.last_decision.action,
        crate::AutoscaleAction::ScaleDown
    );
    assert_eq!(snapshot.limits.view_distance, 2);
    assert_eq!(stream.view_distance, 2);
    assert!(!stream.loaded.contains(&(3, 0)));
    assert_eq!(sessions.ticketed_chunks_sorted().len(), 25);
    assert!(!sessions.ticketed_chunks_sorted().contains(&(3, 0)));
    assert!(stream.ready.is_empty());

    let mut buf = BytesMut::new();
    tokio::time::timeout(Duration::from_millis(100), client.read_buf(&mut buf))
        .await
        .expect("runtime view-distance shrink should emit a forget-level-chunk packet")
        .unwrap();
    for radius in [3, 2] {
        let mut frame = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
            .unwrap()
            .expect("initial and reduced chunk-cache radii precede the unload");
        assert_eq!(frame.id, SetChunkCacheRadius::ID);
        assert_eq!(
            SetChunkCacheRadius::decode(&mut frame.body).unwrap().radius,
            radius
        );
    }
    let frame = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("forget-level-chunk frame");
    assert_eq!(frame.id, ForgetLevelChunk::ID);
    let packet = ForgetLevelChunk::decode(&mut frame.body.clone()).unwrap();
    assert_eq!(
        packet,
        ForgetLevelChunk {
            chunk_x: 3,
            chunk_z: 0,
        }
    );
}

#[test]
fn runtime_control_view_distance_does_not_exceed_client_cap() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(8);
    let profile = crate::login::LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "RuntimeViewDistanceCapAlice".to_string(),
    };
    let old_desired = desired_chunk_set(0, 0, 3);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        3,
        old_desired,
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        3,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );

    stream.replan_view_distance(2, 0.0);
    stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
        view_distance: 3,
        chunk_send_rate: 16,
        chunk_load_rate: 64,
        chunk_generate_rate: 32,
    });

    assert_eq!(stream.view_distance, 2);
    assert_eq!(sessions.ticketed_chunks_sorted().len(), 25);
    assert!(!sessions.ticketed_chunks_sorted().contains(&(3, 0)));
}

#[test]
fn view_distance_replan_does_not_requeue_retained_loaded_chunk() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(8);
    let profile = crate::login::LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "ViewDistanceRetainedChunkAlice".to_string(),
    };
    let desired = desired_chunk_set(0, 0, 3);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        3,
        desired,
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        3,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );

    loop {
        let request = stream
            .scheduler
            .poll_next()
            .expect("center request remains queued");
        let coord = (request.chunk_x, request.chunk_z);
        assert!(stream.scheduler.mark_finished(request));
        if coord == (0, 0) {
            break;
        }
    }
    stream.loaded.insert((0, 0));
    sessions.mark_loaded(session_id, (0, 0));

    stream.replan_effective_view_distance(2, 0.0);

    assert!(stream.loaded.contains(&(0, 0)));
    while let Some(request) = stream.scheduler.poll_next() {
        assert_ne!(
            (request.chunk_x, request.chunk_z),
            (0, 0),
            "retained loaded chunk must not be replayed without unload"
        );
        assert!(stream.scheduler.mark_finished(request));
    }
}

#[tokio::test]
async fn runtime_limits_reduce_load_dispatch_budget() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 4,
        chunk_result_queue_size: 8,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        2,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
        view_distance: 2,
        chunk_send_rate: 16,
        chunk_load_rate: 1,
        chunk_generate_rate: 64,
    });

    stream.dispatch_available().await;

    assert_eq!(stream.dispatched, 1);
    assert_eq!(stream.scheduler.in_flight_len(), 1);
    assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::LoadBudget);
}

#[tokio::test]
async fn runtime_limits_reduce_generate_dispatch_budget() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let calls = Arc::new(AtomicUsize::new(0));
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1).with_generator(Arc::new(
            CountingGenerator {
                calls: Arc::clone(&calls),
            },
        )),
    ));
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 4,
        chunk_result_queue_size: 8,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        2,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
        view_distance: 2,
        chunk_send_rate: 16,
        chunk_load_rate: 64,
        chunk_generate_rate: 1,
    });

    stream.dispatch_available().await;

    assert_eq!(stream.dispatched, 1);
    assert_eq!(stream.scheduler.in_flight_len(), 1);
    assert_eq!(
        stream.last_stop_reason,
        ChunkPipelineStopReason::GenerateBudget
    );
}

#[tokio::test]
async fn low_load_budget_does_not_throttle_generated_dispatches() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let calls = Arc::new(AtomicUsize::new(0));
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1).with_generator(Arc::new(
            CountingGenerator {
                calls: Arc::clone(&calls),
            },
        )),
    ));
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 4,
        chunk_result_queue_size: 8,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        2,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
        view_distance: 2,
        chunk_send_rate: 16,
        chunk_load_rate: 1,
        chunk_generate_rate: 4,
    });

    stream.dispatch_available().await;

    assert_eq!(stream.dispatched, 4);
    assert_eq!(stream.scheduler.in_flight_len(), 4);
    assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::BatchLimit);
}

#[tokio::test]
async fn low_generate_budget_does_not_throttle_cached_world_load_dispatches() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 8);
    for chunk in prioritized_spiral(0, 0, 1, 0.0)
        .take(4)
        .map(|(cx, cz, _)| ChunkPos { x: cx, z: cz })
    {
        storage
            .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
            .unwrap();
    }
    let world = Arc::new(Mutex::new(storage));
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 4,
        chunk_result_queue_size: 8,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        1,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
        view_distance: 1,
        chunk_send_rate: 16,
        chunk_load_rate: 4,
        chunk_generate_rate: 1,
    });

    stream.dispatch_available().await;

    assert_eq!(stream.dispatched, 4);
    assert_eq!(stream.scheduler.in_flight_len(), 4);
    assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::BatchLimit);
}

#[tokio::test]
async fn low_generate_budget_does_not_throttle_prepared_cache_dispatches() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "cache-budget".to_string(),
    };
    let desired = desired_chunk_set(0, 0, 1);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        1,
        desired,
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    for chunk in prioritized_spiral(0, 0, 1, 0.0)
        .take(4)
        .map(|(cx, cz, _)| (cx, cz))
    {
        sessions.cache_prepared_chunk(
            chunk,
            Arc::new(PreparedChunkFrame {
                frame: Bytes::from_static(b"chunk-frame"),
                light: None,
                herd_spawns: Vec::new(),
                hydrated_campfires: Vec::new(),
                packet_data_len: 0,
                build_timing: ChunkBuildTiming::default(),
                write_timing: ChunkWriteTiming::default(),
            }),
        );
    }
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 4,
        chunk_result_queue_size: 8,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        sessions,
        session_id,
        0,
        0,
        0.0,
        1,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    stream.apply_runtime_control_limits(crate::RuntimeControlLimits {
        view_distance: 1,
        chunk_send_rate: 16,
        chunk_load_rate: 64,
        chunk_generate_rate: 1,
    });

    stream.dispatch_available().await;

    assert_eq!(stream.dispatched, 4);
    assert_eq!(stream.ready.len(), 4);
    assert_eq!(stream.scheduler.in_flight_len(), 4);
    assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::BatchLimit);
}

#[tokio::test]
async fn dispatch_prewarms_forward_edge_chunks_before_center_crossing() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 128).with_generator(Arc::new(
            CountingGenerator {
                calls: Arc::clone(&calls),
            },
        )),
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "forward-prewarm".to_string(),
    };
    let desired = desired_chunk_set(0, 0, 4);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        4,
        desired,
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 16,
        chunk_generate_rate: 16,
        chunk_result_queue_size: 64,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        4,
        ChunkPipelineResources::with_limits(2, 2),
        policy,
    );

    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();
    drive_stream_to_completion(
        &mut stream,
        &mut writer,
        &mut light_cache,
        Duration::from_secs(2),
        "visible chunk window should complete before asserting forward prewarm",
    )
    .await;
    assert!(stream.is_complete(), "visible chunk window should complete");

    let forward_edge = (-4..=4).map(|x| (x, 5)).collect::<Vec<_>>();
    for chunk in &forward_edge {
        assert!(
            matches!(
                sessions.prepared_chunk_or_claim(*chunk),
                PreparedChunkClaimResult::InFlight | PreparedChunkClaimResult::Cached
            ),
            "forward edge chunk {chunk:?} should be claimed or cached before the client crosses into center_z=1"
        );
    }
    assert!(
        forward_edge
            .iter()
            .all(|chunk| !sessions.ticketed_chunks_sorted().contains(chunk)),
        "forward prewarm must not expand the client's visible/ticketed view"
    );
}

#[tokio::test]
async fn single_client_prewarm_also_covers_opposite_edge_within_batch() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(Arc::new(
            CountingGenerator {
                calls: Arc::clone(&calls),
            },
        )),
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "negative-z-prewarm".to_string(),
    };
    let desired = desired_chunk_set(0, 0, 4);
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        4,
        desired,
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 16,
        chunk_generate_rate: 16,
        chunk_result_queue_size: 64,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        4,
        ChunkPipelineResources::with_limits(8, 8),
        policy,
    );

    stream.dispatch_forward_prewarm();

    let negative_z_edge = (-4..=4).map(|x| (x, -5)).collect::<Vec<_>>();
    for chunk in &negative_z_edge {
        assert!(
            matches!(
                sessions.prepared_chunk_or_claim(*chunk),
                PreparedChunkClaimResult::InFlight | PreparedChunkClaimResult::Cached
            ),
            "single-client prewarm should claim opposite edge chunk {chunk:?} within the background batch"
        );
    }
    assert!(
        negative_z_edge
            .iter()
            .all(|chunk| !sessions.ticketed_chunks_sorted().contains(chunk)),
        "opposite-edge prewarm must not expand the client's visible/ticketed view"
    );
}

#[tokio::test]
async fn healthy_background_observation_does_not_drop_prewarm_after_tick_pressure() {
    let registry = Arc::new(air_block_registry());
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        64,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let view_distance = 4;
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "pressure-prewarm".to_string(),
    };
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        view_distance,
        desired_chunk_set(0, 0, view_distance),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
        policy: crate::AutoscalePolicy {
            min_view_distance: view_distance,
            max_view_distance: view_distance,
            min_chunk_send_rate: 1,
            max_chunk_send_rate: 16,
            min_chunk_load_rate: 1,
            max_chunk_load_rate: 64,
            min_chunk_generate_rate: 1,
            max_chunk_generate_rate: 32,
            target_tick_ms: 1,
            scale_down_after_seconds: 60,
            ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
        },
        initial_limits: crate::RuntimeControlLimits {
            view_distance,
            chunk_send_rate: 16,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
        },
    });
    let decision = control.observe(crate::RuntimeControlInput {
        tick_ms: 2,
        memory_used_mb: 0,
        memory_limit_mb: 0,
    });
    assert_eq!(decision.pressure, Some(crate::AutoscalePressure::TickTime));

    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        sessions,
        session_id,
        0,
        0,
        0.0,
        view_distance,
        ChunkPipelineResources::with_limits(2, 2),
        ChunkPipelinePolicy::default(),
    )
    .with_runtime_control(Some(control));

    assert!(stream.observe_runtime_control().is_empty());
    stream.dispatch_forward_prewarm();

    let expected = prewarm_edge_batch_limit(view_distance);
    assert_eq!(stream.prewarm_dispatched, expected);
    assert_eq!(stream.prewarm_in_flight.len(), expected);
}

#[test]
fn prewarm_edge_ring_prioritizes_forward_z_edge() {
    let chunks = prewarm_edge_ring_chunks(0, 0, 4, 0.0);
    let first_edge = chunks.into_iter().take(9).collect::<Vec<_>>();

    for chunk in (-4..=4).map(|x| (x, 5)) {
        assert!(
            first_edge.contains(&chunk),
            "forward edge chunk {chunk:?} should be in the first prewarm edge batch, got {first_edge:?}"
        );
    }
}

#[test]
fn prewarm_batch_adds_the_nearest_lateral_edge_at_playable_distance() {
    let limit = prewarm_edge_batch_limit(4);
    let chunks =
        prewarm_edge_batch_chunks(0, 0, 4, 0.0, PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 8.5));

    assert_eq!(limit, 27);
    assert_eq!(chunks.len(), limit);
    for chunk in (-4..=4).map(|x| (x, 5)).chain((-4..=4).map(|x| (x, -5))) {
        assert!(
            chunks.contains(&chunk),
            "missing likely edge chunk {chunk:?}"
        );
    }
    for chunk in (-4..=4).map(|z| (-5, z)) {
        assert!(chunks.contains(&chunk), "missing west edge chunk {chunk:?}");
    }
    assert!(!chunks.contains(&(5, 0)), "far east edge was prewarmed");
    assert!(
        chunks.iter().position(|chunk| *chunk == (-5, 0))
            < chunks.iter().position(|chunk| *chunk == (0, -5)),
        "the nearby lateral edge must start before the farther opposite edge"
    );
}

#[test]
fn prewarm_batch_uses_east_edge_when_player_is_nearer_east_boundary() {
    let chunks =
        prewarm_edge_batch_chunks(0, 0, 4, 0.0, PlayerPose::new(15.5, DEFAULT_SPAWN_Y, 8.5));

    for chunk in (-4..=4).map(|z| (5, z)) {
        assert!(chunks.contains(&chunk), "missing east edge chunk {chunk:?}");
    }
    assert!(!chunks.contains(&(-5, 0)), "far west edge was prewarmed");
}

#[test]
fn prewarm_edge_ring_caps_untrusted_view_distance() {
    let chunks = prewarm_edge_ring_chunks(0, 0, crate::MAX_VIEW_DISTANCE + 1, 0.0);
    let radius = crate::MAX_VIEW_DISTANCE + 1;

    assert_eq!(chunks.len(), (8 * radius) as usize);
}

#[tokio::test]
async fn forward_prewarm_uses_autoscaler_cpu_limit() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let concurrent_call_started = Arc::new(tokio::sync::Notify::new());
    let first_call_gate = Arc::new(GenerationGate::new());
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(Arc::new(
            CountingConcurrentGenerator {
                calls: Arc::clone(&calls),
                active: Arc::clone(&active),
                max_active: Arc::clone(&max_active),
                concurrent_call_started: Some(Arc::clone(&concurrent_call_started)),
                first_call_gate: Some(Arc::clone(&first_call_gate)),
                gated_chunk: None,
                gate_first_call: AtomicBool::new(true),
            },
        )),
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "sequential-prewarm".to_string(),
    };
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        4,
        desired_chunk_set(0, 0, 4),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 16,
        chunk_generate_rate: 16,
        chunk_result_queue_size: 64,
        ..ChunkPipelinePolicy::default()
    };
    let resources = ChunkPipelineResources::with_limits(8, 8);
    for _ in 0..5 {
        resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, None, false);
    }
    assert_eq!(resources.prepare_limit(), 2);
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        4,
        resources,
        policy,
    );

    let prewarm_progress = stream.progress_notify();
    let prewarm_settled = prewarm_progress.notified();
    tokio::pin!(prewarm_settled);
    prewarm_settled.as_mut().enable();
    stream.dispatch_forward_prewarm();
    first_call_gate.wait_started().await;
    let parallel_start =
        tokio::time::timeout(Duration::from_secs(2), concurrent_call_started.notified()).await;
    first_call_gate.release();
    parallel_start.expect("prewarm should use the autoscaler CPU allowance concurrently");
    let forward_edge = (-4..=4).map(|x| (x, 5)).collect::<Vec<_>>();
    tokio::time::timeout(Duration::from_secs(3), prewarm_settled)
        .await
        .expect("forward prewarm completion must publish progress");
    assert!(
        forward_edge
            .iter()
            .all(|chunk| sessions.prepared_chunk(*chunk).is_some()),
        "forward prewarm completion event must cover the full edge"
    );

    assert!(
        calls.load(Ordering::Acquire) > 0,
        "prewarm should exercise generated chunk path"
    );
    assert_eq!(max_active.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn forward_prewarm_releases_remaining_claims_when_new_session_joins() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let first_call_gate = Arc::new(GenerationGate::new());
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(Arc::new(
            CountingConcurrentGenerator {
                calls: Arc::clone(&calls),
                active: Arc::clone(&active),
                max_active: Arc::clone(&max_active),
                concurrent_call_started: None,
                first_call_gate: Some(Arc::clone(&first_call_gate)),
                gated_chunk: None,
                gate_first_call: AtomicBool::new(true),
            },
        )),
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "prewarm-owner".to_string(),
    };
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        4,
        desired_chunk_set(0, 0, 4),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 16,
        chunk_generate_rate: 16,
        chunk_result_queue_size: 64,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        4,
        ChunkPipelineResources::with_limits(8, 8),
        policy,
    );

    stream.dispatch_forward_prewarm();
    first_call_gate.wait_started().await;

    let (secondary_tx, _secondary_rx) = mpsc::channel(1);
    let secondary_profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1),
        name: "prewarm-visible-join".to_string(),
    };
    let (_secondary_id, _) = sessions.register(
        &secondary_profile,
        (0, 0),
        4,
        desired_chunk_set(0, 0, 4),
        secondary_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let prewarm_progress = stream.progress_notify();
    let prewarm_settled = prewarm_progress.notified();
    tokio::pin!(prewarm_settled);
    prewarm_settled.as_mut().enable();
    first_call_gate.release();

    tokio::time::timeout(Duration::from_secs(2), prewarm_settled)
        .await
        .expect("new visible session must wake background prewarm cancellation");

    let forward_edge = (-4..=4).map(|x| (x, 5)).collect::<Vec<_>>();
    let mut cached_count = 0;
    let mut claimable_count = 0;
    for chunk in &forward_edge {
        match sessions.prepared_chunk_or_claim(*chunk) {
            PreparedChunkClaimResult::Cached => {
                cached_count += 1;
            }
            PreparedChunkClaimResult::Claimed(claim) => {
                claimable_count += 1;
                sessions.release_prepared_chunk_claim(*chunk, claim);
            }
            PreparedChunkClaimResult::InFlight => {}
        }
    }

    assert!(
        cached_count < forward_edge.len(),
        "prewarm should not finish the entire forward edge after a newer visible session joins"
    );
    assert!(
        claimable_count > 0,
        "remaining forward edge chunks should be claimable by visible work"
    );
}

#[tokio::test]
async fn crossing_keeps_inflight_nearest_prewarm_result_for_new_visible_edge() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let first_call_gate = Arc::new(GenerationGate::new());
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(Arc::new(
            CountingConcurrentGenerator {
                calls,
                active,
                max_active,
                concurrent_call_started: None,
                first_call_gate: Some(Arc::clone(&first_call_gate)),
                gated_chunk: Some(ChunkPos { x: -5, z: -4 }),
                gate_first_call: AtomicBool::new(true),
            },
        )),
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "crossing-prewarm".to_string(),
    };
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        4,
        desired_chunk_set(0, 0, 4),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        0,
        0,
        0.0,
        4,
        ChunkPipelineResources::with_limits(8, 8),
        ChunkPipelinePolicy::default(),
    );

    stream.dispatch_forward_prewarm();
    first_call_gate.wait_started().await;

    stream.replan_center(-1, 0, 0.0);
    let prewarm_progress = stream.progress_notify();
    prewarm_progress.notified().await;
    let prewarm_settled = prewarm_progress.notified();
    tokio::pin!(prewarm_settled);
    prewarm_settled.as_mut().enable();
    first_call_gate.release();

    tokio::time::timeout(Duration::from_secs(2), prewarm_settled)
        .await
        .expect("center crossing must settle the old prewarm batch");
    assert!(
        sessions.prepared_chunk((-5, -4)).is_some(),
        "the prewarm already producing the new visible edge must publish its result"
    );
    assert!(
        sessions.prepared_chunk((4, -5)).is_none(),
        "remaining work for the old center must be cancelled"
    );
}

#[tokio::test]
async fn latest_same_center_session_owns_shared_forward_prewarm() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(Arc::new(
            CountingGenerator {
                calls: Arc::clone(&calls),
            },
        )),
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let view_distance = 4;
    let (primary_tx, _primary_rx) = mpsc::channel(1);
    let primary = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1),
        name: "prewarm-primary".to_string(),
    };
    let (primary_session, _) = sessions.register(
        &primary,
        (0, 1),
        view_distance,
        desired_chunk_set(0, 1, view_distance),
        primary_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 17.5),
    );
    let (secondary_tx, _secondary_rx) = mpsc::channel(1);
    let secondary = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(2),
        name: "prewarm-secondary".to_string(),
    };
    let (secondary_session, _) = sessions.register(
        &secondary,
        (0, 1),
        view_distance,
        desired_chunk_set(0, 1, view_distance),
        secondary_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 17.5),
    );
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 16,
        chunk_generate_rate: 16,
        chunk_result_queue_size: 64,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        primary_session,
        0,
        1,
        0.0,
        view_distance,
        ChunkPipelineResources::with_limits(8, 8),
        policy,
    );

    stream.dispatch_forward_prewarm();

    assert_eq!(stream.prewarm_dispatched, 0);
    assert_eq!(calls.load(Ordering::Acquire), 0);

    stream.session_id = secondary_session;
    stream.dispatch_forward_prewarm();

    assert_eq!(
        stream.prewarm_dispatched,
        prewarm_edge_batch_limit(view_distance)
    );
    for chunk in (-4..=4).map(|x| (x, 6)) {
        match sessions.prepared_chunk_or_claim(chunk) {
            PreparedChunkClaimResult::InFlight | PreparedChunkClaimResult::Cached => {}
            PreparedChunkClaimResult::Claimed(claim) => {
                sessions.release_prepared_chunk_claim(chunk, claim);
                panic!("latest same-center session should claim forward chunk {chunk:?}");
            }
        }
    }
}

#[tokio::test]
async fn moved_apart_two_client_stream_prewarms_next_owned_edge() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(Arc::new(
            CountingGenerator {
                calls: Arc::clone(&calls),
            },
        )),
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let view_distance = 4;
    let (primary_tx, _primary_rx) = mpsc::channel(1);
    let primary = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1),
        name: "prewarm-positive-client".to_string(),
    };
    let (_primary_session, _) = sessions.register(
        &primary,
        (0, 1),
        view_distance,
        desired_chunk_set(0, 1, view_distance),
        primary_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 17.5),
    );
    let (secondary_tx, _secondary_rx) = mpsc::channel(1);
    let secondary = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(2),
        name: "prewarm-negative-client".to_string(),
    };
    let (secondary_session, _) = sessions.register(
        &secondary,
        (0, -1),
        view_distance,
        desired_chunk_set(0, -1, view_distance),
        secondary_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, -17.5),
    );
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 16,
        chunk_generate_rate: 16,
        chunk_result_queue_size: 64,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        secondary_session,
        0,
        -1,
        0.0,
        view_distance,
        ChunkPipelineResources::with_limits(8, 8),
        policy,
    );

    stream.dispatch_forward_prewarm();

    let next_negative_edge = (-4..=4).map(|x| (x, -6)).collect::<Vec<_>>();
    for chunk in &next_negative_edge {
        assert!(
            matches!(
                sessions.prepared_chunk_or_claim(*chunk),
                PreparedChunkClaimResult::InFlight | PreparedChunkClaimResult::Cached
            ),
            "moved-apart two-client prewarm should claim next negative edge chunk {chunk:?}"
        );
    }
    assert!(
        next_negative_edge
            .iter()
            .all(|chunk| !sessions.ticketed_chunks_sorted().contains(chunk)),
        "moved-apart prewarm must not expand either client's visible/ticketed view"
    );
}

#[tokio::test]
async fn current_second_client_reuses_then_releases_prepared_spawn_window() {
    let registry = Arc::new(air_block_registry());
    let calls = Arc::new(AtomicUsize::new(0));
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 256).with_generator(Arc::new(
            CountingGenerator {
                calls: Arc::clone(&calls),
            },
        )),
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let view_distance = 4;
    let policy = ChunkPipelinePolicy {
        chunk_send_rate: 128,
        chunk_load_rate: 128,
        chunk_generate_rate: 128,
        chunk_prepare_batch_size: 32,
        chunk_result_queue_size: 128,
        ..ChunkPipelinePolicy::default()
    };

    let (primary_tx, _primary_rx) = mpsc::channel(1);
    let primary = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1),
        name: "spawn-cache-primary".to_string(),
    };
    let (primary_session, _) = sessions.register(
        &primary,
        (0, 0),
        view_distance,
        desired_chunk_set(0, 0, view_distance),
        primary_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let (secondary_tx, _secondary_rx) = mpsc::channel(1);
    let secondary = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(2),
        name: "spawn-cache-secondary".to_string(),
    };
    let (secondary_session, _) = sessions.register(
        &secondary,
        (0, 0),
        view_distance,
        desired_chunk_set(0, 0, view_distance),
        secondary_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let mut primary_stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        primary_session,
        0,
        0,
        0.0,
        view_distance,
        ChunkPipelineResources::with_limits(4, 4),
        policy,
    );
    let mut writer = tokio::io::sink();
    let mut primary_light_cache = LightCache::new();
    drive_stream_to_completion(
        &mut primary_stream,
        &mut writer,
        &mut primary_light_cache,
        Duration::from_secs(2),
        "primary spawn window should flush",
    )
    .await;
    assert!(primary_stream.is_complete());
    let spawn_window = desired_chunk_set(0, 0, view_distance);
    assert!(
        spawn_window
            .iter()
            .all(|chunk| sessions.prepared_chunk(*chunk).is_some()),
        "prepared frames must remain while the current second subscriber still needs them"
    );
    let calls_after_primary = calls.load(Ordering::Acquire);

    let mut secondary_stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        secondary_session,
        0,
        0,
        0.0,
        view_distance,
        ChunkPipelineResources::with_limits(4, 4),
        policy,
    );
    let mut secondary_light_cache = LightCache::new();
    drive_stream_to_completion(
        &mut secondary_stream,
        &mut writer,
        &mut secondary_light_cache,
        Duration::from_secs(2),
        "secondary spawn window should flush from shared prepared cache",
    )
    .await;

    assert_eq!(secondary_stream.emitted, 81);
    assert_eq!(secondary_stream.fetch_ms, 0);
    assert_eq!(secondary_stream.slow_fetch_chunks, 0);
    assert_eq!(secondary_stream.build_timing.light_compute_ms, 0);
    assert_eq!(secondary_stream.slow_light_compute_chunks, 0);
    assert_eq!(calls.load(Ordering::Acquire), calls_after_primary);
    assert!(
        spawn_window
            .iter()
            .all(|chunk| sessions.prepared_chunk(*chunk).is_none()),
        "prepared frames must be released after every current subscriber loaded them"
    );
}

#[tokio::test]
async fn waiting_same_spawn_client_keeps_center_first_until_owner_caches_it() {
    let registry = Arc::new(air_block_registry());
    let world = Arc::new(Mutex::new(
        WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 32).with_generator(Arc::new(
            CountingGenerator {
                calls: Arc::new(AtomicUsize::new(0)),
            },
        )),
    ));
    let sessions = Arc::new(SessionRegistry::new());
    let view_distance = 1;
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 1,
        chunk_send_rate: 16,
        chunk_load_rate: 16,
        chunk_generate_rate: 16,
        chunk_result_queue_size: 16,
        ..ChunkPipelinePolicy::default()
    };

    let (primary_tx, _primary_rx) = mpsc::channel(1);
    let primary = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(1),
        name: "same-spawn-owner".to_string(),
    };
    let (_primary_session, _) = sessions.register(
        &primary,
        (0, 0),
        view_distance,
        desired_chunk_set(0, 0, view_distance),
        primary_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let (secondary_tx, _secondary_rx) = mpsc::channel(1);
    let secondary = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(2),
        name: "same-spawn-waiter".to_string(),
    };
    let (secondary_session, _) = sessions.register(
        &secondary,
        (0, 0),
        view_distance,
        desired_chunk_set(0, 0, view_distance),
        secondary_tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let mut secondary_stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        secondary_session,
        0,
        0,
        0.0,
        view_distance,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    let owner_claim = match sessions.prepared_chunk_or_claim((0, 0)) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("expected owner center claim, got {other:?}"),
    };

    secondary_stream.dispatch_available().await;

    assert_eq!(
        secondary_stream.dispatched, 0,
        "later same-spawn client should not duplicate prepare work before the earlier session has warmed the chunk"
    );
    assert_eq!(secondary_stream.ready.len(), 0);
    assert_eq!(secondary_stream.scheduler.in_flight_len(), 0);
    assert_eq!(secondary_stream.scheduler.queued_len(), 9);

    sessions.cache_prepared_chunk(
        (0, 0),
        Arc::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"center-chunk-frame"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        }),
    );
    assert!(sessions.release_prepared_chunk_claim((0, 0), owner_claim));
    secondary_stream.dispatch_available().await;

    let ready = secondary_stream
        .ready
        .values()
        .map(|result| {
            (
                result.request.priority.sequence,
                (result.request.chunk_x, result.request.chunk_z),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(ready, vec![(0, (0, 0))]);
}

#[tokio::test]
async fn prepare_worker_panic_publishes_failure_and_releases_claim() {
    let registry = Arc::new(air_block_registry());
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let chunk = (i32::MAX, 0);
    let (tx, _rx) = mpsc::channel(1);
    let (session_id, _) = sessions.register(
        &LoggedInProfile {
            uuid: uuid::Uuid::from_u128(1),
            name: "prepare-panic".to_string(),
        },
        chunk,
        0,
        HashSet::from([chunk]),
        tx,
        PlayerPose::new(f64::from(i32::MAX) * 16.0, DEFAULT_SPAWN_Y, 0.5),
    );
    let mut stream = ChunkStreamState::new(
        world,
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::clone(&sessions),
        session_id,
        chunk.0,
        chunk.1,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy {
            chunk_prepare_batch_size: 1,
            chunk_result_queue_size: 1,
            ..ChunkPipelinePolicy::default()
        },
    );
    let progress = stream.progress_notify();
    let worker_finished = progress.notified();
    tokio::pin!(worker_finished);
    worker_finished.as_mut().enable();

    stream.dispatch_available().await;
    tokio::time::timeout(Duration::from_secs(1), worker_finished)
        .await
        .expect("panicked prepare worker must publish a terminal result");
    stream.drain_ready();

    assert_eq!(stream.ready.len(), 1);
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();
    let error = stream
        .emit_next_ready(&mut writer, &mut light_cache)
        .await
        .expect_err("failed preparation must fail the stream");
    assert!(
        matches!(
            &error,
            ConnectionError::ChunkPreparation {
                chunk_x,
                chunk_z,
                ..
            } if *chunk_x == i32::MAX && *chunk_z == 0
        ),
        "{error}"
    );
    assert!(!stream.scheduler.is_complete());
    let replacement = match sessions.prepared_chunk_or_claim(chunk) {
        PreparedChunkClaimResult::Claimed(claim) => claim,
        other => panic!("panicked worker leaked claim: {other:?}"),
    };
    assert!(sessions.release_prepared_chunk_claim(chunk, replacement));
}

#[tokio::test(start_paused = true)]
async fn sustained_runtime_control_pressure_scales_prepare_dispatch_before_spawning_workers() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let policy = ChunkPipelinePolicy {
        chunk_prepare_batch_size: 4,
        chunk_result_queue_size: 8,
        chunk_send_rate: 16,
        ..ChunkPipelinePolicy::default()
    };
    let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
        policy: crate::AutoscalePolicy {
            min_view_distance: 2,
            max_view_distance: 2,
            min_chunk_send_rate: 16,
            max_chunk_send_rate: 16,
            min_chunk_load_rate: 1,
            max_chunk_load_rate: 2,
            min_chunk_generate_rate: 64,
            max_chunk_generate_rate: 64,
            queue_pressure_percent: 1,
            scale_down_after_seconds: 60,
            ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
        },
        initial_limits: crate::RuntimeControlLimits {
            view_distance: 2,
            chunk_send_rate: 16,
            chunk_load_rate: 2,
            chunk_generate_rate: 64,
        },
    });
    let mut signals = control
        .take_signal_receiver()
        .expect("test owns runtime control receiver");
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        2,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    )
    .with_runtime_control(Some(control.clone()));
    for _ in 0..4 {
        let request = stream.scheduler.poll_next().expect("queued chunk");
        stream.accept_result(ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms: 0,
            pressure_flush: PressureFlushTiming::default(),
            staged: Vec::new(),
            outcome: ChunkPrepareOutcome::Absent,
        });
    }
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    stream.observe_runtime_control();
    let owner_decision = observe_next_runtime_control_signal(&control, &mut signals).await;
    assert_eq!(owner_decision.action, crate::AutoscaleAction::Hold);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(
        control.observe(healthy_runtime_control_input()).action,
        crate::AutoscaleAction::Hold
    );
    assert_eq!(control.snapshot().limits.chunk_load_rate, 2);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        control.observe(healthy_runtime_control_input()).action,
        crate::AutoscaleAction::ScaleDown
    );
    stream.step(&mut writer, &mut light_cache).await.unwrap();

    let snapshot = control.snapshot();
    assert_eq!(snapshot.limits.chunk_load_rate, 1);
    assert_eq!(stream.dispatched, 1);
    assert_eq!(stream.scheduler.in_flight_len(), 1);
    assert_eq!(stream.last_stop_reason, ChunkPipelineStopReason::LoadBudget);
}

#[tokio::test]
async fn successful_retry_clears_pressure_staged_for_chunk() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy::default(),
    );
    let request = stream.scheduler.poll_next().expect("request");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: vec![(0, 0), (1, 0)],
        outcome: ChunkPrepareOutcome::Backpressured,
    });
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    assert_eq!(
        stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .unwrap(),
        EmitReadyResult::Blocked
    );
    assert!(stream.pressure_staged_contains((0, 0)));
    assert!(stream.pressure_staged_contains((1, 0)));

    let request = stream.scheduler.poll_next().expect("deferred request");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: vec![(0, 0)],
        outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
            frame: Bytes::new(),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 0,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        })),
    });

    assert_eq!(
        stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .unwrap(),
        EmitReadyResult::SentPacket
    );
    assert!(stream.pressure_staged_is_empty());
    assert!(stream.pressure_staged_by_chunk.is_empty());
    assert_eq!(stream.emitted, 1);
}

#[tokio::test]
async fn concurrent_pressure_flush_coalesces_one_bounded_batch() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let mut storage = WorldStorage::open_with_capacity(temp.path(), Arc::clone(&registry), 32)
        .expect("open world storage");
    for x in 0..32 {
        storage
            .insert_generated_chunk(
                ChunkPos { x, z: 0 },
                Chunk::empty(ChunkPos { x, z: 0 }, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }
    assert!(storage.dirty_chunk_cache_saturated());
    let world = Arc::new(Mutex::new(storage));
    let request = ChunkRequest {
        chunk_x: 32,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };
    let mut flushes = Vec::new();
    for _ in 0..64 {
        flushes.push(tokio::spawn(flush_dirty_chunks_for_pressure(
            Arc::clone(&world),
            request,
            0,
        )));
    }

    let mut pressure_flush_runs = 0;
    let mut pressure_flush_planned = 0;
    for flush in flushes {
        let timing = flush.await.unwrap().expect("pressure flush must complete");
        pressure_flush_runs += timing.runs;
        pressure_flush_planned += timing.planned_chunks;
    }

    assert_eq!(pressure_flush_runs, 1);
    assert_eq!(pressure_flush_planned, PRESSURE_FLUSH_MAX_CHUNKS);
    assert_eq!(
        world.lock().await.dirty_count(),
        32 - PRESSURE_FLUSH_MAX_CHUNKS
    );
}

#[tokio::test]
async fn stale_pressure_wait_wakes_on_stream_generation_change() {
    let registry = Arc::new(air_block_registry());
    let world = Arc::new(Mutex::new(WorldStorage::in_memory(Arc::clone(&registry))));
    let (started, started_rx) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let mut started = Some(started);
    let mut release_rx = Some(release_rx);
    let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
        move || {
            let started = started.take().expect("one dirty action");
            let release_rx = release_rx.take().expect("one dirty action");
            async move {
                let _ = started.send(());
                let _ = release_rx.await;
                crate::dirty_flush::DirtyFlushCompletion::Complete
            }
        },
        || async {},
    );
    let notifier = coordinator.notifier();
    let request = ChunkRequest {
        chunk_x: 32,
        chunk_z: 0,
        priority: ChunkPriority {
            ring: 0,
            sequence: 0,
        },
        generation: ChunkPipelineGeneration(1),
    };
    let active_generation = Arc::new(AtomicU64::new(1));
    let generation_changed = Arc::new(tokio::sync::Notify::new());
    let wait = tokio::spawn({
        let active_generation = Arc::clone(&active_generation);
        let generation_changed = Arc::clone(&generation_changed);
        async move {
            relieve_dirty_pressure(
                world,
                request,
                0,
                Some(&notifier),
                &active_generation,
                &generation_changed,
            )
            .await
        }
    });

    started_rx.await.expect("dirty action starts");
    active_generation.store(2, Ordering::Release);
    generation_changed.notify_waiters();

    assert_eq!(wait.await.unwrap().runs, 0);
    release.send(()).expect("release dirty action");
    assert!(matches!(
        coordinator.drain().await,
        crate::dirty_flush::DirtyFlushDrainOutcome::Complete
    ));
}

#[tokio::test]
async fn stream_recovers_deferred_chunk_after_dirty_pressure_clears() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let mut storage = WorldStorage::open_with_capacity(temp.path(), Arc::clone(&registry), 1)
        .unwrap()
        .with_generator(Arc::new(CountingGenerator {
            calls: Arc::clone(&calls),
        }));
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let world = Arc::new(Mutex::new(storage));
    let policy = ChunkPipelinePolicy {
        chunk_send_rate: 1,
        chunk_prepare_batch_size: 1,
        chunk_result_queue_size: 1,
        ..ChunkPipelinePolicy::default()
    };
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(test_biome_registry()),
        Arc::clone(&registry),
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        1,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    );
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    let request = stream.scheduler.poll_next().expect("request");
    let result = crate::resource_profile::measure_future(
        crate::resource_profile::CpuStage::ChunkPreparation,
        prepare_chunk_request(
            request,
            Arc::clone(&world),
            None,
            None,
            Arc::new(test_biome_registry()),
            Arc::clone(&registry),
            None,
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(TagsData::default()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
            None,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            Compression::Disabled,
            ChunkPipelineResources::with_limits(1, 1),
            None,
            Arc::clone(&stream.active_generation),
            Arc::clone(&stream.generation_changed),
            0,
            None,
        ),
    )
    .await;
    assert!(matches!(result.outcome, ChunkPrepareOutcome::Backpressured));
    assert_eq!(result.pressure_flush.runs, 1);
    assert_eq!(result.pressure_flush.planned_chunks, 1);
    assert_eq!(result.pressure_flush.flushed_chunks, 1);
    stream.accept_result(result);
    assert_eq!(
        stream
            .emit_next_ready(&mut writer, &mut light_cache)
            .await
            .unwrap(),
        EmitReadyResult::Blocked
    );

    assert_eq!(stream.emitted, 0);
    assert_eq!(stream.pressure_abandoned, 0);
    assert_eq!(calls.load(Ordering::Acquire), 0);
    {
        let storage = world.lock().await;
        assert_eq!(storage.dirty_count(), 0);
    }

    drive_stream_to_completion(
        &mut stream,
        &mut writer,
        &mut light_cache,
        Duration::from_secs(2),
        "deferred chunk should recover after dirty pressure clears",
    )
    .await;

    assert!(stream.is_complete());
    assert_eq!(stream.emitted, 1);
    assert_eq!(stream.absent, 0);
    assert_eq!(stream.pressure_abandoned, 0);
    assert!(stream.pressure_staged_is_empty());
    assert!(stream.pressure_staged_by_chunk.is_empty());
    assert_eq!(calls.load(Ordering::Acquire), 1);
}
