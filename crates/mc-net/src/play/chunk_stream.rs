use super::campfire::campfire_cooking_states_from_chunk;
use super::session::{
    PreparedChunkClaim, PreparedChunkClaimResult, SessionPreparedChunkClaimResult,
    settlement_inhabitant_spawn,
};
use super::*;
use mc_protocol::packets::play::SetChunkCacheRadius;
use mc_world::light::compute_chunk_light_in;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
mod inflight;
mod planning;
mod prepare;
use prepare::*;
mod publication;
use planning::initial_window_target;
#[cfg(test)]
pub(super) use planning::spiral_chunks;
pub(super) use planning::{desired_chunk_set, prewarm_edge_ring_chunks, prioritized_spiral};
#[cfg(test)]
use planning::{prewarm_edge_batch_chunks, prewarm_edge_batch_limit};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ChunkBuildTiming {
    chunk_data_ms: u64,
    heightmap_ms: u64,
    light_compute_ms: u64,
    light_encode_ms: u64,
}

impl ChunkBuildTiming {
    fn add(&mut self, other: ChunkBuildTiming) {
        self.chunk_data_ms += other.chunk_data_ms;
        self.heightmap_ms += other.heightmap_ms;
        self.light_compute_ms += other.light_compute_ms;
        self.light_encode_ms += other.light_encode_ms;
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ChunkWriteTiming {
    packet_encode_ms: u64,
    frame_ms: u64,
    socket_write_ms: u64,
    framed_bytes: usize,
}

#[derive(Debug, Default, Clone, Copy)]
struct PressureFlushTiming {
    runs: usize,
    planned_chunks: usize,
    flushed_chunks: usize,
    plan_ms: u64,
    write_ms: u64,
    commit_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChunkStreamStep {
    Progress,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkPrepareBudgetClass {
    Load,
    Generate,
}

impl ChunkPrepareBudgetClass {
    fn stop_reason(self) -> ChunkPipelineStopReason {
        match self {
            Self::Load => ChunkPipelineStopReason::LoadBudget,
            Self::Generate => ChunkPipelineStopReason::GenerateBudget,
        }
    }
}

const INITIAL_CHUNK_MIN_RING: i32 = 2;
const CHUNK_STAGE_SLOW_MS: u64 = 50;
const CHUNK_BACKPRESSURE_MAX_RETRIES: usize = 16;
const PREPARED_IN_FLIGHT_DEFERRAL_LIMIT: usize = 2;
const PRESSURE_FLUSH_MAX_CHUNKS: usize = 8;
const PRESSURE_FLUSH_STALE_REGION_RETRIES: usize = 3;
const PREWARM_EDGE_RING_LIMIT: usize = 40;
const PREWARM_PREPARED_CACHE_LIMIT: usize = 64;
static PRESSURE_FLUSH_COORDINATOR: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

pub(super) struct ChunkStreamState {
    world: WorldHandle,
    world_read: Option<mc_world::WorldReadView>,
    world_mutation: Option<mc_world::WorldMutationView>,
    chunk_source: Option<mc_world::ChunkSourceView>,
    biomes: Arc<Registry>,
    blocks: Arc<BlockRegistry>,
    loader_block_projection: Option<crate::loader::LoaderBlockProjection>,
    block_light: Option<Arc<BlockLightTable>>,
    items: Arc<ItemRegistry>,
    tags: Arc<TagsData>,
    recipes: Arc<Vec<mc_data::recipes::Recipe>>,
    block_entity_types: Arc<mc_data::block_entity_types::BlockEntityTypeRegistry>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
    passive_herd_fallback_surfaces: Arc<Vec<mc_world::BlockStateId>>,
    passive_herd_water: Arc<Vec<mc_world::BlockStateId>>,
    passive_herd_passable: Arc<Vec<BlockStateId>>,
    passive_spawn_rules: Arc<mc_data::biomes::BiomeSpawnRules>,
    entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
    compression: Compression,
    sessions: Arc<SessionRegistry>,
    simulation: Option<SimulationHandle>,
    session_id: SessionId,
    resources: ChunkPipelineResources,
    dirty_flush: Option<crate::dirty_flush::DirtyFlushNotifier>,
    active_generation: Arc<AtomicU64>,
    generation_changed: Arc<tokio::sync::Notify>,
    result_tx: mpsc::Sender<ChunkPrepareResult>,
    result_rx: mpsc::Receiver<ChunkPrepareResult>,
    progress_notify: Arc<tokio::sync::Notify>,
    ready: BTreeMap<u32, ChunkPrepareResult>,
    prewarm_in_flight: HashSet<(i32, i32)>,
    pressure_retries: HashMap<(i32, i32), usize>,
    policy: ChunkPipelinePolicy,
    configured_prepare_batch_size: usize,
    prepare_limit_stop_reason: ChunkPipelineStopReason,
    runtime_control: Option<crate::RuntimeControlHandle>,
    chunk_pressure_source: Option<crate::control_plane::RuntimeControlChunkPressureSource>,
    first_chunk_sla_source: Option<crate::control_plane::RuntimeControlFirstChunkSlaSource>,
    first_chunk_sla_target_ms: u64,
    first_chunk_sla_active: bool,
    result_queue_size: usize,
    chunk_queue_saturated: bool,
    center_cx: i32,
    center_cz: i32,
    direction_yaw: f32,
    view_distance: i32,
    last_sent_view_distance: Option<i32>,
    last_observed_autoscale_decision: Option<crate::AutoscaleDecision>,
    client_view_distance_cap: i32,
    runtime_view_distance_limit: i32,
    scheduler: ChunkScheduler,
    staged: HashSet<(i32, i32)>,
    loaded: HashSet<(i32, i32)>,
    /// Chunks whose publication made an unloaded work target observable again.
    published: Vec<(i32, i32)>,
    started: Instant,
    fetch_ms: u64,
    build_timing: ChunkBuildTiming,
    packet_encode_ms: u64,
    frame_ms: u64,
    socket_write_ms: u64,
    max_fetch_ms: u64,
    max_chunk_data_ms: u64,
    max_heightmap_ms: u64,
    max_light_compute_ms: u64,
    max_light_compute_chunk: Option<(i32, i32)>,
    max_light_compute_revision: Option<u64>,
    max_light_encode_ms: u64,
    max_packet_encode_ms: u64,
    max_frame_ms: u64,
    max_socket_write_ms: u64,
    slow_fetch_chunks: usize,
    slow_light_compute_chunks: usize,
    slow_packet_encode_chunks: usize,
    slow_frame_chunks: usize,
    slow_socket_write_chunks: usize,
    framed_bytes: usize,
    first_chunk_ms: Option<u64>,
    ring1_complete_ms: Option<u64>,
    ring2_complete_ms: Option<u64>,
    ring_emitted: Vec<usize>,
    emitted: usize,
    absent: usize,
    pressure_abandoned: usize,
    pressure_staged_by_chunk: HashMap<(i32, i32), HashSet<(i32, i32)>>,
    pressure_flush_runs: usize,
    pressure_flush_planned_chunks: usize,
    pressure_flush_flushed_chunks: usize,
    pressure_flush_plan_ms: u64,
    pressure_flush_write_ms: u64,
    pressure_flush_commit_ms: u64,
    max_pressure_flush_plan_ms: u64,
    max_pressure_flush_write_ms: u64,
    max_pressure_flush_commit_ms: u64,
    memory_pressure_shed_runs: usize,
    memory_pressure_shed_ready: usize,
    memory_pressure_shed_in_flight: usize,
    memory_pressure_active: bool,
    bytes: usize,
    dispatch_turns: usize,
    yielded_turns: usize,
    dispatched: usize,
    prewarm_dispatched: usize,
    max_in_flight: usize,
    max_ready: usize,
    last_stop_reason: ChunkPipelineStopReason,
    wait_for_first_chunk: bool,
    summary_logged: bool,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedChunkFrame {
    pub(super) frame: Bytes,
    pub(super) light: Option<ChunkLight>,
    pub(super) herd_spawns: Vec<HerdSpawn>,
    pub(super) hydrated_campfires: Vec<(mc_world::BlockPos, CampfireCookingState)>,
    pub(super) packet_data_len: usize,
    pub(super) build_timing: ChunkBuildTiming,
    pub(super) write_timing: ChunkWriteTiming,
}

impl PreparedChunkFrame {
    fn prepared_cache_hit(&self) -> Self {
        let mut cached = self.clone();
        let framed_bytes = cached.write_timing.framed_bytes;
        cached.build_timing = ChunkBuildTiming::default();
        cached.write_timing = ChunkWriteTiming::default();
        cached.write_timing.framed_bytes = framed_bytes;
        cached
    }
}

enum ChunkPrepareOutcome {
    Ready(Box<PreparedChunkFrame>),
    Absent,
    Backpressured,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmitReadyResult {
    SentPacket,
    DrainedNoPacket,
    Blocked,
    Empty,
}

#[derive(Debug, Clone, Copy)]
enum PreparedChunkFence {
    Claimed(PreparedChunkClaim),
    CachedRevision(u64),
}

impl PreparedChunkFence {
    fn revision(self) -> u64 {
        match self {
            Self::Claimed(claim) => claim.revision,
            Self::CachedRevision(revision) => revision,
        }
    }
}

struct ChunkPrepareResult {
    request: crate::ChunkRequest,
    prepare_claim: Option<PreparedChunkFence>,
    fetch_ms: u64,
    pressure_flush: PressureFlushTiming,
    staged: Vec<(i32, i32)>,
    outcome: ChunkPrepareOutcome,
}

#[cfg(test)]
pub(super) fn hostile_chunk_spawns(chunk: (i32, i32)) -> bool {
    mc_entity::natural_spawn_26_1_2::hostile_chunk_spawns(chunk)
}

#[cfg(test)]
fn sheep_color_for_rolls(
    climate: mc_data::biomes::SheepColorClimate,
    outer_roll: u32,
    common_roll: u32,
) -> mc_entity::SheepColor {
    mc_entity::natural_spawn_26_1_2::sheep_color_for_rolls(climate, outer_roll, common_roll)
}

pub(super) fn plan_passive_herd(
    chunk: &Chunk,
    land_surface: Option<mc_world::BlockStateId>,
    land_fallback_surfaces: &[mc_world::BlockStateId],
    water: Option<&[mc_world::BlockStateId]>,
    passable: &[BlockStateId],
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
) -> Vec<HerdSpawn> {
    mc_entity::natural_spawn_26_1_2::plan_chunk_herd_templates(
        chunk,
        mc_entity::natural_spawn_26_1_2::ChunkHerdPlanningContext {
            land_surface,
            land_fallback_surfaces,
            water,
            passable,
            sea_level: DEFAULT_SEA_LEVEL,
        },
        rules,
        entity_types,
    )
}

#[cfg(test)]
fn chunk_biome_at(chunk: &Chunk, lx: u8, y: i32, lz: u8) -> Option<&mc_data::Identifier> {
    mc_entity::natural_spawn_26_1_2::chunk_biome_at(chunk, lx, y, lz)
}

#[cfg(test)]
fn herd_surface_y(
    chunk: &Chunk,
    lx: u8,
    lz: u8,
    surface: mc_world::BlockStateId,
    fallback_surfaces: &[BlockStateId],
    passable: &[BlockStateId],
) -> Option<(i32, BlockStateId)> {
    mc_entity::natural_spawn_26_1_2::herd_surface_y(
        chunk,
        lx,
        lz,
        surface,
        fallback_surfaces,
        passable,
    )
}

pub(crate) fn passive_entity_passable_blocks(blocks: &BlockRegistry) -> Vec<BlockStateId> {
    blocks
        .states()
        .filter(|state| passable_block_name(state.block.id.as_str()))
        .map(|state| state.id)
        .collect()
}

pub(crate) fn passive_herd_fallback_surface_blocks(blocks: &BlockRegistry) -> Vec<BlockStateId> {
    blocks
        .states()
        .filter(|state| passive_herd_fallback_surface_name(state.block.id.as_str()))
        .map(|state| state.id)
        .collect()
}

fn passive_herd_fallback_surface_name(name: &str) -> bool {
    mc_data::block_semantics_26_1_2::passive_herd_fallback_surface_name(name)
}

pub(crate) fn passable_block_name(name: &str) -> bool {
    mc_data::block_semantics_26_1_2::passable_block_name(name)
}

impl ChunkStreamState {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        world: WorldHandle,
        biomes: Arc<Registry>,
        blocks: Arc<BlockRegistry>,
        block_light: Option<Arc<BlockLightTable>>,
        items: Arc<ItemRegistry>,
        tags: Arc<TagsData>,
        recipes: Arc<Vec<mc_data::recipes::Recipe>>,
        block_entity_types: Arc<mc_data::block_entity_types::BlockEntityTypeRegistry>,
        passive_herd_surface: Option<mc_world::BlockStateId>,
        passive_herd_fallback_surfaces: Arc<Vec<mc_world::BlockStateId>>,
        passive_herd_water: Arc<Vec<mc_world::BlockStateId>>,
        passive_herd_passable: Arc<Vec<BlockStateId>>,
        passive_spawn_rules: Arc<mc_data::biomes::BiomeSpawnRules>,
        entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
        compression: Compression,
        sessions: Arc<SessionRegistry>,
        session_id: SessionId,
        center_cx: i32,
        center_cz: i32,
        direction_yaw: f32,
        view_distance: i32,
        resources: ChunkPipelineResources,
        policy: ChunkPipelinePolicy,
    ) -> Self {
        let vd = view_distance.max(0);
        let loader_block_projection = sessions.loader_block_projection(session_id, &blocks);
        let (result_tx, result_rx) = mpsc::channel(policy.chunk_result_queue_size);
        let progress_notify = Arc::new(tokio::sync::Notify::new());
        let scheduler = ChunkScheduler::new(prioritized_spiral(
            center_cx,
            center_cz,
            view_distance,
            direction_yaw,
        ));
        let active_generation = Arc::new(AtomicU64::new(scheduler.current_generation().0));
        Self {
            world,
            world_read: None,
            world_mutation: None,
            chunk_source: None,
            biomes,
            blocks,
            loader_block_projection,
            block_light,
            items,
            tags,
            recipes,
            block_entity_types,
            passive_herd_surface,
            passive_herd_fallback_surfaces,
            passive_herd_water,
            passive_herd_passable,
            passive_spawn_rules,
            entity_types,
            compression,
            sessions,
            simulation: None,
            session_id,
            resources,
            dirty_flush: None,
            active_generation,
            generation_changed: Arc::new(tokio::sync::Notify::new()),
            result_tx,
            result_rx,
            progress_notify,
            ready: BTreeMap::new(),
            prewarm_in_flight: HashSet::new(),
            pressure_retries: HashMap::new(),
            policy,
            configured_prepare_batch_size: policy.chunk_prepare_batch_size.max(1),
            prepare_limit_stop_reason: ChunkPipelineStopReason::BatchLimit,
            runtime_control: None,
            chunk_pressure_source: None,
            first_chunk_sla_source: None,
            first_chunk_sla_target_ms: u64::MAX,
            first_chunk_sla_active: false,
            result_queue_size: policy.chunk_result_queue_size,
            chunk_queue_saturated: false,
            center_cx,
            center_cz,
            direction_yaw,
            view_distance: vd,
            last_sent_view_distance: None,
            last_observed_autoscale_decision: None,
            client_view_distance_cap: vd,
            runtime_view_distance_limit: vd,
            scheduler,
            staged: HashSet::new(),
            loaded: HashSet::new(),
            published: Vec::new(),
            started: Instant::now(),
            fetch_ms: 0,
            build_timing: ChunkBuildTiming::default(),
            packet_encode_ms: 0,
            frame_ms: 0,
            socket_write_ms: 0,
            max_fetch_ms: 0,
            max_chunk_data_ms: 0,
            max_heightmap_ms: 0,
            max_light_compute_ms: 0,
            max_light_compute_chunk: None,
            max_light_compute_revision: None,
            max_light_encode_ms: 0,
            max_packet_encode_ms: 0,
            max_frame_ms: 0,
            max_socket_write_ms: 0,
            slow_fetch_chunks: 0,
            slow_light_compute_chunks: 0,
            slow_packet_encode_chunks: 0,
            slow_frame_chunks: 0,
            slow_socket_write_chunks: 0,
            framed_bytes: 0,
            first_chunk_ms: None,
            ring1_complete_ms: None,
            ring2_complete_ms: None,
            ring_emitted: vec![0; (vd + 1) as usize],
            emitted: 0,
            absent: 0,
            pressure_abandoned: 0,
            pressure_staged_by_chunk: HashMap::new(),
            pressure_flush_runs: 0,
            pressure_flush_planned_chunks: 0,
            pressure_flush_flushed_chunks: 0,
            pressure_flush_plan_ms: 0,
            pressure_flush_write_ms: 0,
            pressure_flush_commit_ms: 0,
            max_pressure_flush_plan_ms: 0,
            max_pressure_flush_write_ms: 0,
            max_pressure_flush_commit_ms: 0,
            memory_pressure_shed_runs: 0,
            memory_pressure_shed_ready: 0,
            memory_pressure_shed_in_flight: 0,
            memory_pressure_active: false,
            bytes: 0,
            dispatch_turns: 0,
            yielded_turns: 0,
            dispatched: 0,
            prewarm_dispatched: 0,
            max_in_flight: 0,
            max_ready: 0,
            last_stop_reason: ChunkPipelineStopReason::QueueEmpty,
            wait_for_first_chunk: true,
            summary_logged: false,
        }
    }

    pub(super) fn with_world_read(mut self, world_read: Option<mc_world::WorldReadView>) -> Self {
        self.world_read = world_read;
        self
    }

    pub(super) fn with_world_mutation(
        mut self,
        world_mutation: Option<mc_world::WorldMutationView>,
    ) -> Self {
        self.world_mutation = world_mutation;
        self
    }

    pub(super) fn with_chunk_source(
        mut self,
        chunk_source: Option<mc_world::ChunkSourceView>,
    ) -> Self {
        self.chunk_source = chunk_source;
        self
    }

    pub(super) fn with_simulation(mut self, simulation: SimulationHandle) -> Self {
        self.simulation = Some(simulation);
        self
    }

    pub(super) fn with_runtime_control(
        mut self,
        runtime_control: Option<crate::RuntimeControlHandle>,
    ) -> Self {
        if let Some(control) = runtime_control.as_ref() {
            self.first_chunk_sla_target_ms = control.snapshot().policy.target_first_chunk_ms;
            self.chunk_pressure_source = Some(control.chunk_pressure_source());
            self.first_chunk_sla_source = Some(control.first_chunk_sla_source());
        }
        self.runtime_control = runtime_control;
        self
    }

    pub(super) fn with_dirty_flush(
        mut self,
        dirty_flush: Option<crate::dirty_flush::DirtyFlushNotifier>,
    ) -> Self {
        self.dirty_flush = dirty_flush;
        self
    }

    fn publish_active_generation(&self) {
        self.active_generation
            .store(self.scheduler.current_generation().0, Ordering::Release);
        self.generation_changed.notify_waiters();
    }

    pub(super) fn is_complete(&self) -> bool {
        self.scheduler.is_complete()
    }

    /// Drain chunks published since the caller's last stream step. Publication
    /// is the player-visible load boundary for event-gated resident work.
    pub(super) fn take_published_chunks(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.published)
    }

    pub(super) fn progress_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.progress_notify)
    }

    pub(super) fn has_immediate_work(&self) -> bool {
        !self.ready.is_empty()
            || !self.result_rx.is_empty()
            || (self.scheduler.queued_len() > 0
                && matches!(
                    self.last_stop_reason,
                    ChunkPipelineStopReason::BatchLimit
                        | ChunkPipelineStopReason::TimeBudget
                        | ChunkPipelineStopReason::SendBudget
                        | ChunkPipelineStopReason::LoadBudget
                        | ChunkPipelineStopReason::GenerateBudget
                ))
    }

    fn release_loaded_world_chunk(&self, chunk: (i32, i32)) {
        if let Some(world_read) = self.world_read.as_ref() {
            let released = world_read.release_chunk(ChunkPos {
                x: chunk.0,
                z: chunk.1,
            });
            debug_assert!(released, "loaded chunk retention missing for {chunk:?}");
        }
    }

    fn reset_window_metrics(&mut self) {
        self.recover_runtime_control_sources();
        self.staged.clear();
        self.started = Instant::now();
        self.fetch_ms = 0;
        self.build_timing = ChunkBuildTiming::default();
        self.packet_encode_ms = 0;
        self.frame_ms = 0;
        self.socket_write_ms = 0;
        self.max_fetch_ms = 0;
        self.max_chunk_data_ms = 0;
        self.max_heightmap_ms = 0;
        self.max_light_compute_ms = 0;
        self.max_light_compute_chunk = None;
        self.max_light_compute_revision = None;
        self.max_light_encode_ms = 0;
        self.max_packet_encode_ms = 0;
        self.max_frame_ms = 0;
        self.max_socket_write_ms = 0;
        self.slow_fetch_chunks = 0;
        self.slow_light_compute_chunks = 0;
        self.slow_packet_encode_chunks = 0;
        self.slow_frame_chunks = 0;
        self.slow_socket_write_chunks = 0;
        self.framed_bytes = 0;
        self.first_chunk_ms = None;
        self.ring1_complete_ms = None;
        self.ring2_complete_ms = None;
        self.ring_emitted = vec![0; (self.view_distance.max(0) + 1) as usize];
        self.emitted = 0;
        self.absent = 0;
        self.pressure_abandoned = 0;
        self.reset_pressure_tracking();
        self.pressure_flush_runs = 0;
        self.pressure_flush_planned_chunks = 0;
        self.pressure_flush_flushed_chunks = 0;
        self.pressure_flush_plan_ms = 0;
        self.pressure_flush_write_ms = 0;
        self.pressure_flush_commit_ms = 0;
        self.max_pressure_flush_plan_ms = 0;
        self.max_pressure_flush_write_ms = 0;
        self.max_pressure_flush_commit_ms = 0;
        self.bytes = 0;
        self.dispatch_turns = 0;
        self.yielded_turns = 0;
        self.dispatched = 0;
        self.prewarm_dispatched = 0;
        self.max_in_flight = 0;
        self.max_ready = 0;
        self.last_stop_reason = ChunkPipelineStopReason::QueueEmpty;
        self.wait_for_first_chunk = false;
        self.summary_logged = false;
    }

    fn set_stop_reason(&mut self, reason: ChunkPipelineStopReason) {
        self.last_stop_reason = reason;
        self.resources.record_stop_reason(reason);
    }

    pub(super) async fn step<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<ChunkStreamStep, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let result = self.step_inner(writer, light_cache).await;
        if result.is_err() {
            self.recover_runtime_control_sources();
        }
        result
    }

    async fn step_inner<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<ChunkStreamStep, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        self.drain_ready();
        let unloads = self.observe_runtime_control();
        if self.last_sent_view_distance != Some(self.view_distance) {
            write_packet(
                writer,
                &SetChunkCacheRadius {
                    radius: self.view_distance,
                },
                self.compression,
            )
            .await?;
            let decision = self.last_observed_autoscale_decision.as_ref();
            debug!(
                target: "solaris::chunk_visibility",
                event = "chunk_radius_sent",
                session_id = self.session_id,
                tick = self.sessions.simulation_tick(),
                generation = self.scheduler.current_generation().0,
                previous_radius = ?self.last_sent_view_distance,
                new_radius = self.view_distance,
                client_radius_cap = self.client_view_distance_cap,
                runtime_radius_limit = self.runtime_view_distance_limit,
                center_cx = self.center_cx,
                center_cz = self.center_cz,
                autoscale_last_action = decision.map(|decision| {
                    crate::control_plane::autoscale_action_label(decision.action)
                }),
                autoscale_last_pressure = decision.map(|decision| {
                    crate::control_plane::autoscale_pressure_label(decision.pressure)
                }),
                autoscale_last_reason = decision.map(|decision| decision.reason.as_str()),
                "chunk cache radius packet written"
            );
            self.last_sent_view_distance = Some(self.view_distance);
        }
        for (chunk_x, chunk_z) in unloads {
            light_cache.remove(ChunkPos {
                x: chunk_x,
                z: chunk_z,
            });
            write_packet(
                writer,
                &ForgetLevelChunk { chunk_x, chunk_z },
                self.compression,
            )
            .await?;
            self.log_chunk_unload(chunk_x, chunk_z, "runtime_control");
        }
        if !self.memory_pressure_active {
            self.dispatch_available().await;
        }
        self.drain_ready();

        let made_send_progress = self.emit_ready_batch(writer, light_cache).await?;
        let initial_target = initial_window_target(self.view_distance);
        if self.emitted + self.absent >= initial_target || self.scheduler.is_complete() {
            self.wait_for_first_chunk = false;
        }

        if self.scheduler.is_complete() {
            self.dispatch_forward_prewarm();
            self.set_stop_reason(ChunkPipelineStopReason::Complete);
            self.recover_runtime_control_sources();
            return Ok(ChunkStreamStep::Complete);
        }
        if !made_send_progress {
            self.yielded_turns += 1;
        }

        Ok(ChunkStreamStep::Progress)
    }
}

impl Drop for ChunkStreamState {
    fn drop(&mut self) {
        let loaded = std::mem::take(&mut self.loaded);
        for chunk in loaded {
            self.release_loaded_world_chunk(chunk);
        }
        self.recover_runtime_control_sources();
        self.active_generation.store(0, Ordering::Release);
        self.generation_changed.notify_waiters();
        let cancelled_requests = self.scheduler.queued_len()
            + self.scheduler.in_flight_len()
            + self.prewarm_in_flight.len();
        self.resources
            .record_stream_cancellation(cancelled_requests);
        self.close_and_release_buffered_results();
        self.clear_ready();
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "chunk_stream_autoscale_tests.rs"]
mod autoscale_tests;

#[cfg(test)]
#[path = "chunk_stream_world_handle_tests.rs"]
mod world_handle_tests;
