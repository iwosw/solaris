use super::*;
pub(super) struct PendingLoadedChunkRetention {
    world_read: Option<mc_world::WorldReadView>,
    position: ChunkPos,
    armed: bool,
}
impl PendingLoadedChunkRetention {
    pub(super) fn new(world_read: Option<&mc_world::WorldReadView>, chunk: (i32, i32)) -> Self {
        let world_read = world_read.cloned();
        let position = ChunkPos {
            x: chunk.0,
            z: chunk.1,
        };
        if let Some(view) = world_read.as_ref() {
            view.retain_chunk(position);
        }
        Self {
            armed: world_read.is_some(),
            world_read,
            position,
        }
    }

    pub(super) fn transfer_to_loaded(mut self) {
        self.armed = false;
    }
}
impl Drop for PendingLoadedChunkRetention {
    fn drop(&mut self) {
        if self.armed
            && let Some(view) = self.world_read.as_ref()
        {
            let released = view.release_chunk(self.position);
            debug_assert!(released, "pending loaded chunk retention disappeared");
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_chunk_request(
    request: ChunkRequest,
    world: WorldHandle,
    world_read: Option<mc_world::WorldReadView>,
    world_mutation: Option<mc_world::WorldMutationView>,
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
    resources: ChunkPipelineResources,
    dirty_flush: Option<crate::dirty_flush::DirtyFlushNotifier>,
    active_generation: Arc<AtomicU64>,
    generation_changed: Arc<tokio::sync::Notify>,
    current_tick: u64,
    loader_block_projection: Option<crate::loader::LoaderBlockProjection>,
) -> ChunkPrepareResult {
    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }
    let loaded = match load_chunk_neighbourhood(
        Arc::clone(&world),
        world_read.clone(),
        request.chunk_x,
        request.chunk_z,
        resources.clone(),
        request,
        Arc::clone(&active_generation),
        block_light.is_some(),
    )
    .await
    {
        Ok(loaded) => loaded,
        Err(err) => {
            return ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms: 0,
                pressure_flush: PressureFlushTiming::default(),
                staged: Vec::new(),
                outcome: ChunkPrepareOutcome::Failed(err),
            };
        }
    };

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }

    let LoadedNeighbourhood {
        centre,
        neighbourhood,
        staged,
        fetch_ms,
        backpressured,
    } = loaded;

    let Some(centre) = centre else {
        if backpressured {
            let pressure_flush = relieve_dirty_pressure(
                Arc::clone(&world),
                request,
                current_tick,
                dirty_flush.as_ref(),
                &active_generation,
                &generation_changed,
            )
            .await;
            return ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms,
                pressure_flush,
                staged,
                outcome: ChunkPrepareOutcome::Backpressured,
            };
        }
        return ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms,
            pressure_flush: PressureFlushTiming::default(),
            staged,
            outcome: ChunkPrepareOutcome::Absent,
        };
    };

    if backpressured {
        let pressure_flush = relieve_dirty_pressure(
            Arc::clone(&world),
            request,
            current_tick,
            dirty_flush.as_ref(),
            &active_generation,
            &generation_changed,
        )
        .await;
        return ChunkPrepareResult {
            request,
            prepare_claim: None,
            fetch_ms,
            pressure_flush,
            staged,
            outcome: ChunkPrepareOutcome::Backpressured,
        };
    }

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }

    let light_sources = (block_light.is_some() && ChunkLight::from_chunk(&centre).is_none())
        .then(|| neighbourhood.clone());

    let cpu_permit = match resources.acquire_cpu().await {
        Ok(permit) => permit,
        Err(_) => {
            return ChunkPrepareResult {
                request,
                prepare_claim: None,
                fetch_ms,
                pressure_flush: PressureFlushTiming::default(),
                staged,
                outcome: ChunkPrepareOutcome::Failed("CPU worker pool closed".into()),
            };
        }
    };

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }

    let outcome = match tokio::task::spawn_blocking(move || {
        let _permit = cpu_permit;
        let built = if let Some(table) = block_light.as_deref() {
            CHUNK_LIGHT_WORKSPACE.with_borrow_mut(|workspace| {
                let result = build_chunk_packet(
                    centre.as_ref(),
                    &neighbourhood,
                    biomes.as_ref(),
                    blocks.as_ref(),
                    items.as_ref(),
                    tags.as_ref(),
                    recipes.as_ref(),
                    block_entity_types.as_ref(),
                    Some(table),
                    passive_herd_surface,
                    passive_herd_fallback_surfaces.as_ref(),
                    passive_herd_water.as_slice(),
                    passive_herd_passable.as_ref(),
                    passive_spawn_rules.as_ref(),
                    entity_types.as_ref(),
                    Some(&mut *workspace),
                    request.chunk_x,
                    request.chunk_z,
                    loader_block_projection.as_ref(),
                );
                CHUNK_LIGHT_WORKSPACE_GAUGE
                    .with_borrow_mut(|gauge| gauge.update(workspace.allocated_bytes()));
                result
            })
        } else {
            build_chunk_packet(
                centre.as_ref(),
                &neighbourhood,
                biomes.as_ref(),
                blocks.as_ref(),
                items.as_ref(),
                tags.as_ref(),
                recipes.as_ref(),
                block_entity_types.as_ref(),
                None,
                passive_herd_surface,
                passive_herd_fallback_surfaces.as_ref(),
                passive_herd_water.as_slice(),
                passive_herd_passable.as_ref(),
                passive_spawn_rules.as_ref(),
                entity_types.as_ref(),
                None,
                request.chunk_x,
                request.chunk_z,
                loader_block_projection.as_ref(),
            )
        }
        .map_err(|err| err.to_string())?;
        crate::resource_profile::measure(crate::resource_profile::CpuStage::PacketFraming, || {
            frame_chunk_packet(built, compression)
        })
        .map_err(|err| err.to_string())
    })
    .await
    {
        Ok(Ok(prepared)) => ChunkPrepareOutcome::Ready(Box::new(prepared)),
        Ok(Err(err)) => ChunkPrepareOutcome::Failed(err),
        Err(err) => ChunkPrepareOutcome::Failed(err.to_string()),
    };

    if is_active_request(request, &active_generation)
        && let ChunkPrepareOutcome::Ready(prepared) = &outcome
        && let Some(light_sources) = light_sources.as_ref()
        && let Some(light) = prepared.light.as_ref()
    {
        publish_computed_light_if_sources_current(
            &world,
            world_read.as_ref(),
            world_mutation.as_ref(),
            ChunkPos {
                x: request.chunk_x,
                z: request.chunk_z,
            },
            light_sources,
            light,
        )
        .await;
    }

    if !is_active_request(request, &active_generation) {
        return stale_chunk_result(request, &resources);
    }

    ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms,
        pressure_flush: PressureFlushTiming::default(),
        staged,
        outcome,
    }
}
pub(super) async fn publish_computed_light_if_sources_current(
    world: &WorldHandle,
    world_read: Option<&mc_world::WorldReadView>,
    world_mutation: Option<&mc_world::WorldMutationView>,
    centre: ChunkPos,
    sources: &[[Option<Arc<Chunk>>; 3]; 3],
    light: &ChunkLight,
) -> bool {
    if let Some(world_read) = world_read {
        let Some(world_mutation) = world_mutation else {
            return false;
        };
        let positions = sources
            .iter()
            .enumerate()
            .flat_map(|(dz, row)| {
                row.iter().enumerate().map(move |(dx, _)| ChunkPos {
                    x: centre.x + dx as i32 - 1,
                    z: centre.z + dz as i32 - 1,
                })
            })
            .collect::<Vec<_>>();
        let current = world_read.snapshot_chunks(&positions);
        let mut expected_current = HashMap::with_capacity(positions.len());
        for (position, expected) in positions.into_iter().zip(sources.iter().flatten()) {
            let Some(expected) = expected else {
                return false;
            };
            let Some(current) = current.chunk(position) else {
                return false;
            };
            if current.light_source_token() != expected.light_source_token() {
                return false;
            }
            expected_current.insert(position, Some(current));
        }
        return world_mutation.publish_baked_light_conditionally(
            &expected_current,
            std::iter::once((centre, light)),
        );
    }

    let mut storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::ChunkPrepare,
        "publish computed chunk light",
        Instant::now(),
        world.lock().await,
    );
    for (dz, row) in sources.iter().enumerate() {
        for (dx, expected) in row.iter().enumerate() {
            let Some(expected) = expected else {
                return false;
            };
            let position = ChunkPos {
                x: centre.x + dx as i32 - 1,
                z: centre.z + dz as i32 - 1,
            };
            let Some(current) = storage.cached_chunk_snapshot(position) else {
                return false;
            };
            if current.light_source_token() != expected.light_source_token() {
                return false;
            }
        }
    }
    match storage.set_baked_light(centre, light) {
        Ok(published) => published,
        Err(error) => {
            warn!(error = %error, cx = centre.x, cz = centre.z, "computed chunk light publish failed");
            false
        }
    }
}
fn is_active_request(request: ChunkRequest, active_generation: &AtomicU64) -> bool {
    active_generation.load(Ordering::Acquire) == request.generation.0
}
fn stale_chunk_result(
    request: ChunkRequest,
    resources: &ChunkPipelineResources,
) -> ChunkPrepareResult {
    resources.record_stale_result_rejection();
    ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Absent,
    }
}
pub(super) async fn relieve_dirty_pressure(
    world: WorldHandle,
    request: ChunkRequest,
    current_tick: u64,
    dirty_flush: Option<&crate::dirty_flush::DirtyFlushNotifier>,
    active_generation: &AtomicU64,
    generation_changed: &tokio::sync::Notify,
) -> PressureFlushTiming {
    if let Some(ticket) =
        dirty_flush.and_then(crate::dirty_flush::DirtyFlushNotifier::request_dirty_flush_ticket)
    {
        let completion = ticket.wait();
        tokio::pin!(completion);
        loop {
            let changed = generation_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !is_active_request(request, active_generation) {
                return PressureFlushTiming::default();
            }
            tokio::select! {
                biased;
                completed = completion.as_mut() => {
                    if !completed {
                        debug!(
                            cx = request.chunk_x,
                            cz = request.chunk_z,
                            "dirty flush worker stopped before pressure request completed"
                        );
                    }
                    return PressureFlushTiming::default();
                }
                () = changed.as_mut() => {}
            }
        }
    }

    match flush_dirty_chunks_for_pressure(world, request, current_tick).await {
        Ok(timing) => timing,
        Err(err) => {
            warn!(
                cx = request.chunk_x,
                cz = request.chunk_z,
                error = %err,
                "dirty pressure flush failed"
            );
            PressureFlushTiming::default()
        }
    }
}
pub(super) async fn flush_dirty_chunks_for_pressure(
    world: WorldHandle,
    request: ChunkRequest,
    current_tick: u64,
) -> Result<PressureFlushTiming, String> {
    let _flush_guard = PRESSURE_FLUSH_COORDINATOR
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let mut timing = PressureFlushTiming::default();
    let mut stale_retries = 0usize;
    loop {
        let plan_started = Instant::now();
        let plan = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk pressure flush plan",
                Instant::now(),
                world.lock().await,
            );
            if storage.world_root().is_none() || !storage.dirty_chunk_cache_saturated() {
                return Ok(timing);
            }
            storage
                .plan_dirty_flush_at_tick_bounded(current_tick, PRESSURE_FLUSH_MAX_CHUNKS)
                .map_err(|err| err.to_string())?
        };
        timing.plan_ms += plan_started.elapsed().as_millis() as u64;
        if plan.is_empty() {
            return Ok(timing);
        }
        let planned_chunks = plan.chunk_count();
        timing.planned_chunks += planned_chunks;
        let write_started = Instant::now();
        let commit = match crate::dirty_flush::write_dirty_flush_blocking_typed(plan).await {
            Ok(commit) => commit,
            Err(err)
                if err.is_stale_region() && stale_retries < PRESSURE_FLUSH_STALE_REGION_RETRIES =>
            {
                stale_retries += 1;
                timing.runs += 1;
                timing.write_ms += write_started.elapsed().as_millis() as u64;
                continue;
            }
            Err(err) => return Err(err.to_string()),
        };
        timing.write_ms += write_started.elapsed().as_millis() as u64;
        let commit_started = Instant::now();
        let install = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk pressure flush install",
                Instant::now(),
                world.lock().await,
            );
            storage.install_dirty_flush(commit)
        };
        let install = match install {
            Ok(install) => install,
            Err(mc_world::WorldError::StaleRegion(_))
                if stale_retries < PRESSURE_FLUSH_STALE_REGION_RETRIES =>
            {
                stale_retries += 1;
                timing.runs += 1;
                timing.commit_ms += commit_started.elapsed().as_millis() as u64;
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        let synced = crate::dirty_flush::sync_dirty_flush_install_blocking_typed(install)
            .await
            .map_err(|error| error.to_string())?;
        let flushed = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk pressure flush finalize",
                Instant::now(),
                world.lock().await,
            );
            storage.finalize_dirty_flush(synced).cleaned_chunks()
        };
        let commit_ms = commit_started.elapsed().as_millis() as u64;
        timing.runs += 1;
        timing.flushed_chunks += flushed;
        timing.commit_ms += commit_ms;
        debug!(
            target: "solaris::profile",
            cx = request.chunk_x,
            cz = request.chunk_z,
            planned_chunks,
            flushed,
            plan_ms = timing.plan_ms,
            write_ms = timing.write_ms,
            commit_ms,
            stale_retries,
            "dirty pressure flush completed"
        );
        return Ok(timing);
    }
}
pub(super) struct LoadedNeighbourhood {
    pub(super) centre: Option<Arc<Chunk>>,
    pub(super) neighbourhood: [[Option<Arc<Chunk>>; 3]; 3],
    pub(super) staged: Vec<(i32, i32)>,
    pub(super) fetch_ms: u64,
    pub(super) backpressured: bool,
}
pub(super) struct NeighbourSnapshotPlan {
    row: usize,
    column: usize,
    pub(super) position: ChunkPos,
    pub(super) source: NeighbourSnapshotSource,
}
pub(super) enum NeighbourSnapshotSource {
    Cached(Arc<Chunk>),
    Load(mc_world::ChunkDiskLoadPlan),
}
pub(super) fn plan_missing_neighbour_snapshots(
    storage: &mc_world::WorldStorage,
    centre: ChunkPos,
    neighbourhood: &[[Option<Arc<Chunk>>; 3]; 3],
) -> Vec<NeighbourSnapshotPlan> {
    let mut plans = Vec::with_capacity(8);
    for (row, chunks) in neighbourhood.iter().enumerate() {
        for (column, chunk) in chunks.iter().enumerate() {
            if (row == 1 && column == 1) || chunk.is_some() {
                continue;
            }
            let position = ChunkPos {
                x: centre.x + column as i32 - 1,
                z: centre.z + row as i32 - 1,
            };
            let source = match storage.plan_chunk_snapshot_without_generation(position) {
                mc_world::ChunkSnapshotPlan::Cached(chunk) => {
                    NeighbourSnapshotSource::Cached(chunk)
                }
                mc_world::ChunkSnapshotPlan::Load(plan) => NeighbourSnapshotSource::Load(plan),
            };
            plans.push(NeighbourSnapshotPlan {
                row,
                column,
                position,
                source,
            });
        }
    }
    plans
}
pub(super) async fn chunk_prepare_can_cache(
    world: &WorldHandle,
    world_read: Option<&mc_world::WorldReadView>,
    position: ChunkPos,
    operation: &'static str,
) -> bool {
    if let Some(world_read) = world_read {
        return world_read.can_cache_new_chunk(position);
    }
    let storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::ChunkPrepare,
        operation,
        Instant::now(),
        world.lock().await,
    );
    storage.can_cache_new_chunk(position)
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn load_chunk_neighbourhood(
    world: WorldHandle,
    world_read: Option<mc_world::WorldReadView>,
    cx: i32,
    cz: i32,
    resources: ChunkPipelineResources,
    request: ChunkRequest,
    active_generation: Arc<AtomicU64>,
    need_full_neighbourhood: bool,
) -> Result<LoadedNeighbourhood, String> {
    if !is_active_request(request, &active_generation) {
        return Ok(LoadedNeighbourhood {
            centre: None,
            neighbourhood: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            staged: Vec::new(),
            fetch_ms: 0,
            backpressured: false,
        });
    }
    let fetch_started = Instant::now();
    if let Some(world_read) = world_read.as_ref() {
        let centre_pos = ChunkPos { x: cx, z: cz };
        let mut positions = vec![centre_pos];
        if need_full_neighbourhood {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let position = ChunkPos {
                        x: cx + dx,
                        z: cz + dz,
                    };
                    if position != centre_pos {
                        positions.push(position);
                    }
                }
            }
        }
        let snapshot = world_read.snapshot_chunks(&positions);
        if let Some(centre) = snapshot.chunk(centre_pos) {
            let mut neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
                std::array::from_fn(|_| std::array::from_fn(|_| None));
            neighbourhood[1][1] = Some(Arc::clone(&centre));
            let mut staged = vec![(cx, cz)];
            if need_full_neighbourhood {
                for (dz, row) in neighbourhood.iter_mut().enumerate() {
                    for (dx, slot) in row.iter_mut().enumerate() {
                        if dx == 1 && dz == 1 {
                            continue;
                        }
                        let ncx = cx + (dx as i32 - 1);
                        let ncz = cz + (dz as i32 - 1);
                        if let Some(chunk) = snapshot.chunk(ChunkPos { x: ncx, z: ncz }) {
                            *slot = Some(chunk);
                            staged.push((ncx, ncz));
                        }
                    }
                }
            }
            if !need_full_neighbourhood || neighbourhood.iter().flatten().all(Option::is_some) {
                return Ok(LoadedNeighbourhood {
                    centre: Some(centre),
                    neighbourhood,
                    staged,
                    fetch_ms: fetch_started.elapsed().as_millis() as u64,
                    backpressured: false,
                });
            }
        }
    }
    let mut neighbourhood: [[Option<Arc<Chunk>>; 3]; 3] =
        std::array::from_fn(|_| std::array::from_fn(|_| None));
    let mut centre = None;
    let mut staged = Vec::new();
    let mut disk_plan = None;
    let mut backpressured = false;

    let generator = {
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ChunkPrepare,
            "chunk prepare snapshot",
            Instant::now(),
            world.lock().await,
        );
        if !is_active_request(request, &active_generation) {
            return Ok(LoadedNeighbourhood {
                centre: None,
                neighbourhood,
                staged,
                fetch_ms: fetch_started.elapsed().as_millis() as u64,
                backpressured: false,
            });
        }
        match storage.plan_chunk_snapshot_without_generation(ChunkPos { x: cx, z: cz }) {
            mc_world::ChunkSnapshotPlan::Cached(chunk) => {
                centre = Some(Arc::clone(&chunk));
                neighbourhood[1][1] = Some(chunk);
                staged.push((cx, cz));
            }
            mc_world::ChunkSnapshotPlan::Load(plan) => disk_plan = Some(plan),
        }
        for (dz, row) in neighbourhood.iter_mut().enumerate() {
            for (dx, slot) in row.iter_mut().enumerate() {
                if dx == 1 && dz == 1 {
                    continue;
                }
                let ncx = cx + (dx as i32 - 1);
                let ncz = cz + (dz as i32 - 1);
                if let Some(chunk) = storage.cached_chunk_snapshot(ChunkPos { x: ncx, z: ncz }) {
                    *slot = Some(chunk);
                    staged.push((ncx, ncz));
                }
            }
        }
        storage.generator()
    };

    if centre.is_none()
        && let Some(plan) = disk_plan
    {
        match load_chunk_from_disk(
            plan,
            resources.clone(),
            request,
            Arc::clone(&active_generation),
        )
        .await
        {
            Ok(Some(chunk)) => {
                let chunk = {
                    let mut storage = crate::lock_metrics::timed_guard(
                        crate::lock_metrics::LockMetricKind::ChunkPrepare,
                        "chunk prepare disk commit",
                        Instant::now(),
                        world.lock().await,
                    );
                    if !is_active_request(request, &active_generation) {
                        return Ok(LoadedNeighbourhood {
                            centre: None,
                            neighbourhood,
                            staged,
                            fetch_ms: fetch_started.elapsed().as_millis() as u64,
                            backpressured: false,
                        });
                    }
                    storage.try_commit_chunk_snapshot(ChunkPos { x: cx, z: cz }, chunk)
                };
                match chunk {
                    Ok(Some(chunk)) => {
                        centre = Some(Arc::clone(&chunk));
                        neighbourhood[1][1] = Some(chunk);
                        staged.push((cx, cz));
                        crate::operator_metrics::record_chunk_loaded();
                    }
                    Ok(None) => backpressured = true,
                    Err(err) => {
                        return Err(format!("chunk commit failed at ({cx},{cz}): {err}"));
                    }
                }
            }
            Ok(None) => {}
            Err(err) => return Err(format!("chunk read failed at ({cx},{cz}): {err}")),
        }
    }

    if centre.is_none()
        && !backpressured
        && let Some(generator) = generator.as_ref()
    {
        let can_cache = chunk_prepare_can_cache(
            &world,
            world_read.as_ref(),
            ChunkPos { x: cx, z: cz },
            "chunk prepare generation pressure check",
        )
        .await;
        if !can_cache {
            return Ok(LoadedNeighbourhood {
                centre: None,
                neighbourhood,
                staged,
                fetch_ms: fetch_started.elapsed().as_millis() as u64,
                backpressured: true,
            });
        }
        let chunk = match generate_fresh_chunk(
            Arc::clone(generator),
            ChunkPos { x: cx, z: cz },
            resources.clone(),
            request,
            Arc::clone(&active_generation),
        )
        .await
        {
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                return Ok(LoadedNeighbourhood {
                    centre: None,
                    neighbourhood,
                    staged,
                    fetch_ms: fetch_started.elapsed().as_millis() as u64,
                    backpressured: false,
                });
            }
            Err(err) => {
                return Err(format!("chunk generation failed at ({cx},{cz}): {err}"));
            }
        };
        let chunk = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk prepare commit",
                Instant::now(),
                world.lock().await,
            );
            if !is_active_request(request, &active_generation) {
                return Ok(LoadedNeighbourhood {
                    centre: None,
                    neighbourhood,
                    staged,
                    fetch_ms: fetch_started.elapsed().as_millis() as u64,
                    backpressured: false,
                });
            }
            match storage.try_insert_generated_chunk(ChunkPos { x: cx, z: cz }, chunk) {
                Ok(true) => crate::operator_metrics::record_chunk_generated(),
                Ok(false) => backpressured = true,
                Err(err) => {
                    return Err(format!(
                        "generated chunk insert failed at ({cx},{cz}): {err}"
                    ));
                }
            }
            storage.cached_chunk_snapshot(ChunkPos { x: cx, z: cz })
        };
        if let Some(chunk) = chunk {
            centre = Some(Arc::clone(&chunk));
            neighbourhood[1][1] = Some(chunk);
            staged.push((cx, cz));
        }
    }

    if need_full_neighbourhood
        && centre.is_some()
        && let Some(generator) = generator.as_ref()
    {
        let plans = {
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::ChunkPrepare,
                "chunk prepare neighbour snapshot batch",
                Instant::now(),
                world.lock().await,
            );
            if !is_active_request(request, &active_generation) {
                return Ok(LoadedNeighbourhood {
                    centre: None,
                    neighbourhood,
                    staged,
                    fetch_ms: fetch_started.elapsed().as_millis() as u64,
                    backpressured: false,
                });
            }
            plan_missing_neighbour_snapshots(&storage, ChunkPos { x: cx, z: cz }, &neighbourhood)
        };
        for plan in plans {
            let NeighbourSnapshotPlan {
                row,
                column,
                position,
                source,
            } = plan;
            let ncx = position.x;
            let ncz = position.z;
            let disk_plan = match source {
                NeighbourSnapshotSource::Cached(chunk) => {
                    neighbourhood[row][column] = Some(chunk);
                    staged.push((ncx, ncz));
                    continue;
                }
                NeighbourSnapshotSource::Load(plan) => plan,
            };
            if let Some(chunk) = world_read
                .as_ref()
                .and_then(|world_read| world_read.snapshot_chunks(&[position]).chunk(position))
            {
                neighbourhood[row][column] = Some(chunk);
                staged.push((ncx, ncz));
                continue;
            }
            let mut chunk = match load_chunk_from_disk(
                disk_plan,
                resources.clone(),
                request,
                Arc::clone(&active_generation),
            )
            .await
            {
                Ok(Some(chunk)) => Some(chunk),
                Ok(None) => None,
                Err(err) => {
                    return Err(format!(
                        "neighbour chunk read failed at ({ncx},{ncz}): {err}"
                    ));
                }
            };
            if chunk.is_none() {
                let can_generate = chunk_prepare_can_cache(
                    &world,
                    world_read.as_ref(),
                    position,
                    "chunk prepare neighbour generation pressure check",
                )
                .await;
                if !can_generate {
                    return Ok(LoadedNeighbourhood {
                        centre,
                        neighbourhood,
                        staged,
                        fetch_ms: fetch_started.elapsed().as_millis() as u64,
                        backpressured: true,
                    });
                }
                chunk = match generate_fresh_chunk(
                    Arc::clone(generator),
                    position,
                    resources.clone(),
                    request,
                    Arc::clone(&active_generation),
                )
                .await
                {
                    Ok(chunk) => chunk,
                    Err(err) => {
                        return Err(format!(
                            "neighbour chunk generation failed at ({ncx},{ncz}): {err}"
                        ));
                    }
                };
            }
            let Some(chunk) = chunk else {
                continue;
            };
            let committed = {
                let mut storage = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::ChunkPrepare,
                    "chunk prepare neighbour commit",
                    Instant::now(),
                    world.lock().await,
                );
                if !is_active_request(request, &active_generation) {
                    return Ok(LoadedNeighbourhood {
                        centre: None,
                        neighbourhood,
                        staged,
                        fetch_ms: fetch_started.elapsed().as_millis() as u64,
                        backpressured: false,
                    });
                }
                storage.try_commit_chunk_snapshot(position, chunk)
            };
            match committed {
                Ok(Some(chunk)) => {
                    neighbourhood[row][column] = Some(chunk);
                    staged.push((ncx, ncz));
                }
                Ok(None) => {
                    return Ok(LoadedNeighbourhood {
                        centre,
                        neighbourhood,
                        staged,
                        fetch_ms: fetch_started.elapsed().as_millis() as u64,
                        backpressured: true,
                    });
                }
                Err(err) => {
                    return Err(format!(
                        "neighbour chunk commit failed at ({ncx},{ncz}): {err}"
                    ));
                }
            }
        }
    }

    Ok(LoadedNeighbourhood {
        centre,
        neighbourhood,
        staged,
        fetch_ms: fetch_started.elapsed().as_millis() as u64,
        backpressured,
    })
}
async fn load_chunk_from_disk(
    plan: mc_world::ChunkDiskLoadPlan,
    resources: ChunkPipelineResources,
    request: ChunkRequest,
    active_generation: Arc<AtomicU64>,
) -> Result<Option<Chunk>, String> {
    if !is_active_request(request, &active_generation) {
        return Ok(None);
    }
    let permit = resources
        .acquire_io()
        .await
        .map_err(|_| "IO worker pool closed".to_string())?;
    if !is_active_request(request, &active_generation) {
        return Ok(None);
    }
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        crate::resource_profile::measure(crate::resource_profile::CpuStage::ChunkDisk, || {
            plan.load()
        })
    })
    .await
    {
        Ok(Ok(chunk)) => Ok(chunk),
        Ok(Err(err)) => Err(err.to_string()),
        Err(err) => Err(err.to_string()),
    }
}
async fn generate_fresh_chunk(
    generator: Arc<dyn mc_world::ChunkGenerator>,
    pos: ChunkPos,
    resources: ChunkPipelineResources,
    request: ChunkRequest,
    active_generation: Arc<AtomicU64>,
) -> Result<Option<Chunk>, String> {
    if !is_active_request(request, &active_generation) {
        return Ok(None);
    }
    let permit = resources
        .acquire_cpu()
        .await
        .map_err(|_| "CPU worker pool closed".to_string())?;
    if !is_active_request(request, &active_generation) {
        return Ok(None);
    }
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut chunk = crate::resource_profile::measure(
            crate::resource_profile::CpuStage::ChunkGeneration,
            || generator.generate(pos),
        );
        chunk.mark_dirty();
        chunk
    })
    .await
    {
        Ok(chunk) => Ok(Some(chunk)),
        Err(err) => Err(err.to_string()),
    }
}
pub(super) struct BuiltChunkPacket {
    pub(super) packet: LevelChunkWithLight,
    pub(super) light: Option<ChunkLight>,
    herd_spawns: Vec<HerdSpawn>,
    hydrated_campfires: Vec<(mc_world::BlockPos, CampfireCookingState)>,
    pub(super) timing: ChunkBuildTiming,
}
#[allow(clippy::too_many_arguments)]
pub(super) fn build_chunk_packet(
    centre: &Chunk,
    neighbourhood: &[[Option<Arc<Chunk>>; 3]; 3],
    biomes: &Registry,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    tags: &TagsData,
    recipes: &[mc_data::recipes::Recipe],
    block_entity_types: &mc_data::block_entity_types::BlockEntityTypeRegistry,
    block_light: Option<&BlockLightTable>,
    passive_herd_surface: Option<mc_world::BlockStateId>,
    passive_herd_fallback_surfaces: &[mc_world::BlockStateId],
    passive_herd_water: &[mc_world::BlockStateId],
    passive_herd_passable: &[BlockStateId],
    passive_spawn_rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    workspace: Option<&mut LightWorkspace>,
    cx: i32,
    cz: i32,
    loader_block_projection: Option<&crate::loader::LoaderBlockProjection>,
) -> Result<BuiltChunkPacket, mc_world::wire::WireError> {
    let _cpu =
        crate::resource_profile::CpuScope::new(crate::resource_profile::CpuStage::ChunkPreparation);
    let mut timing = ChunkBuildTiming::default();

    let chunk_data_started = Instant::now();
    let data = match loader_block_projection {
        Some(projection) => {
            crate::resource_profile::measure(crate::resource_profile::CpuStage::ChunkData, || {
                mc_world::wire::encode_chunk_data_with_block_projection(
                    centre,
                    biomes,
                    blocks,
                    &|state| projection.project(state),
                )
            })?
        }
        None => {
            crate::resource_profile::measure(crate::resource_profile::CpuStage::ChunkData, || {
                encode_chunk_data(centre, biomes, blocks)
            })?
        }
    };
    timing.chunk_data_ms = chunk_data_started.elapsed().as_millis() as u64;

    let heightmap_started = Instant::now();
    let heightmaps =
        crate::resource_profile::measure(crate::resource_profile::CpuStage::Heightmaps, || {
            client_heightmaps(centre)
        })
        .into_iter()
        .map(|h| ChunkHeightmap {
            type_id: h.type_id,
            data: h.data,
        })
        .collect();
    timing.heightmap_ms = heightmap_started.elapsed().as_millis() as u64;

    let mut computed_light = None;
    let light = if block_light.is_some() {
        if let Some(baked) = ChunkLight::from_chunk(centre) {
            let light_encode_started = Instant::now();
            let wire = crate::resource_profile::measure(
                crate::resource_profile::CpuStage::LightEncoding,
                || encode_chunk_light(&baked),
            );
            timing.light_encode_ms = light_encode_started.elapsed().as_millis() as u64;
            computed_light = Some(baked);
            LightData {
                sky_y_mask: wire.sky_y_mask,
                block_y_mask: wire.block_y_mask,
                empty_sky_y_mask: wire.empty_sky_y_mask,
                empty_block_y_mask: wire.empty_block_y_mask,
                sky_updates: wire.sky_updates,
                block_updates: wire.block_updates,
            }
        } else if let (Some(table), Some(ws)) = (block_light, workspace) {
            // Centre slot is the chunk we already have a reference to;
            // off-centre slots come from the staged map.
            let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
            for (dz, row) in neighbourhood.iter().enumerate() {
                for (dx, slot) in row.iter().enumerate() {
                    refs[dz][dx] = slot.as_deref();
                }
            }
            refs[1][1] = Some(centre);

            let light_compute_started = Instant::now();
            let computed = crate::resource_profile::measure(
                crate::resource_profile::CpuStage::Lighting,
                || compute_chunk_light_in(ws, refs, table),
            );
            timing.light_compute_ms = light_compute_started.elapsed().as_millis() as u64;

            let light_encode_started = Instant::now();
            let wire = crate::resource_profile::measure(
                crate::resource_profile::CpuStage::LightEncoding,
                || encode_chunk_light(&computed),
            );
            timing.light_encode_ms = light_encode_started.elapsed().as_millis() as u64;
            computed_light = Some(computed);
            LightData {
                sky_y_mask: wire.sky_y_mask,
                block_y_mask: wire.block_y_mask,
                empty_sky_y_mask: wire.empty_sky_y_mask,
                empty_block_y_mask: wire.empty_block_y_mask,
                sky_updates: wire.sky_updates,
                block_updates: wire.block_updates,
            }
        } else {
            LightData::empty()
        }
    } else {
        LightData::empty()
    };
    let block_entities = mc_world::wire::client_block_entities(centre, blocks, items)
        .into_iter()
        .filter_map(|record| {
            let type_id = block_entity_types.id_of(&record.type_name)?;
            let packed_xz =
                ((record.pos.x.rem_euclid(16) as u8) << 4) | record.pos.z.rem_euclid(16) as u8;
            Some(BlockEntityInfo {
                packed_xz,
                y: record.pos.y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
                type_id: i32::try_from(type_id).ok()?,
                nbt: record.nbt,
            })
        })
        .collect();
    let hydrated_campfires = campfire_cooking_states_from_chunk(centre, recipes, items, tags);

    let herd_spawns = plan_passive_herd(
        centre,
        passive_herd_surface,
        passive_herd_fallback_surfaces,
        Some(passive_herd_water),
        passive_herd_passable,
        passive_spawn_rules,
        entity_types,
    );
    Ok(BuiltChunkPacket {
        packet: LevelChunkWithLight {
            chunk_x: cx,
            chunk_z: cz,
            heightmaps,
            data,
            block_entities,
            light,
        },
        light: computed_light,
        herd_spawns,
        hydrated_campfires,
        timing,
    })
}
fn frame_chunk_packet(
    built: BuiltChunkPacket,
    compression: Compression,
) -> Result<PreparedChunkFrame, ConnectionError> {
    let mut timing = ChunkWriteTiming::default();

    let packet_encode_started = Instant::now();
    let mut body = BytesMut::new();
    built.packet.encode(&mut body)?;
    timing.packet_encode_ms = packet_encode_started.elapsed().as_millis() as u64;
    let packet_data_len = built.packet.data.len();

    let frame_started = Instant::now();
    let framed = encode_frame(LevelChunkWithLight::ID, &body, compression)?;
    timing.frame_ms = frame_started.elapsed().as_millis() as u64;
    timing.framed_bytes = framed.len();

    Ok(PreparedChunkFrame {
        frame: framed,
        light: built.light,
        herd_spawns: built.herd_spawns,
        hydrated_campfires: built.hydrated_campfires,
        packet_data_len,
        build_timing: built.timing,
        write_timing: timing,
    })
}
