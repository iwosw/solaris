use super::*;

impl ChunkStreamState {
    pub(super) fn spawn_prepare_worker(
        &self,
        request: ChunkRequest,
        prepare_claim: PreparedChunkClaim,
    ) {
        let world = Arc::clone(&self.world);
        let world_read = self.world_read.clone();
        let world_mutation = self.world_mutation.clone();
        let biomes = Arc::clone(&self.biomes);
        let blocks = Arc::clone(&self.blocks);
        let block_light = self.block_light.as_ref().map(Arc::clone);
        let items = Arc::clone(&self.items);
        let tags = Arc::clone(&self.tags);
        let recipes = Arc::clone(&self.recipes);
        let block_entity_types = Arc::clone(&self.block_entity_types);
        let passive_herd_surface = self.passive_herd_surface;
        let passive_herd_fallback_surfaces = Arc::clone(&self.passive_herd_fallback_surfaces);
        let passive_herd_water = Arc::clone(&self.passive_herd_water);
        let passive_herd_passable = Arc::clone(&self.passive_herd_passable);
        let passive_spawn_rules = Arc::clone(&self.passive_spawn_rules);
        let entity_types = Arc::clone(&self.entity_types);
        let resources = self.resources.clone();
        let prepare_task = resources.begin_prepare_task();
        let dirty_flush = self.dirty_flush.clone();
        let active_generation = Arc::clone(&self.active_generation);
        let generation_changed = Arc::clone(&self.generation_changed);
        let compression = self.compression;
        let loader_block_projection = self.loader_block_projection.clone();
        let current_tick = self.sessions.simulation_tick();
        let sessions = Arc::clone(&self.sessions);
        let tx = self.result_tx.clone();
        let progress_notify = Arc::clone(&self.progress_notify);
        tokio::spawn(async move {
            let _prepare_task = prepare_task;
            let mut claim = PreparedChunkClaimLease::new(
                sessions,
                (request.chunk_x, request.chunk_z),
                prepare_claim,
            );
            let worker = tokio::spawn(async move {
                let request_admission = match resources.acquire_prepare_request().await {
                    Ok(permit) => permit,
                    Err(_) => {
                        return ChunkPrepareResult {
                            request,
                            prepare_claim: None,
                            fetch_ms: 0,
                            pressure_flush: PressureFlushTiming::default(),
                            staged: Vec::new(),
                            outcome: ChunkPrepareOutcome::Failed(
                                "chunk request admission closed".into(),
                            ),
                        };
                    }
                };
                let _request_admission = request_admission;
                crate::resource_profile::measure_future(
                    crate::resource_profile::CpuStage::ChunkPreparation,
                    prepare_chunk_request(
                        request,
                        world,
                        world_read,
                        world_mutation,
                        biomes,
                        blocks,
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
                        resources,
                        dirty_flush,
                        active_generation,
                        generation_changed,
                        current_tick,
                        loader_block_projection,
                    ),
                )
                .await
            });
            let mut result = match worker.await {
                Ok(result) => result,
                Err(error) => ChunkPrepareResult {
                    request,
                    prepare_claim: None,
                    fetch_ms: 0,
                    pressure_flush: PressureFlushTiming::default(),
                    staged: Vec::new(),
                    outcome: ChunkPrepareOutcome::Failed(format!(
                        "chunk prepare worker failed: {error}"
                    )),
                },
            };
            result.prepare_claim = Some(PreparedChunkFence::Claimed(claim.claim()));
            let sent = tx.send(result).await.is_ok();
            progress_notify.notify_one();
            if sent {
                claim.disarm();
            }
        });
    }
    pub(super) fn spawn_prewarm_batch_worker(
        &self,
        batch: Vec<(ChunkRequest, PreparedChunkClaim)>,
        session_registration_epoch: SessionId,
    ) {
        let world = Arc::clone(&self.world);
        let world_read = self.world_read.clone();
        let world_mutation = self.world_mutation.clone();
        let biomes = Arc::clone(&self.biomes);
        let blocks = Arc::clone(&self.blocks);
        let block_light = self.block_light.as_ref().map(Arc::clone);
        let items = Arc::clone(&self.items);
        let tags = Arc::clone(&self.tags);
        let recipes = Arc::clone(&self.recipes);
        let block_entity_types = Arc::clone(&self.block_entity_types);
        let passive_herd_surface = self.passive_herd_surface;
        let passive_herd_fallback_surfaces = Arc::clone(&self.passive_herd_fallback_surfaces);
        let passive_herd_water = Arc::clone(&self.passive_herd_water);
        let passive_herd_passable = Arc::clone(&self.passive_herd_passable);
        let passive_spawn_rules = Arc::clone(&self.passive_spawn_rules);
        let entity_types = Arc::clone(&self.entity_types);
        let resources = self.resources.clone();
        let prepare_task = resources.begin_prepare_task();
        let dirty_flush = self.dirty_flush.clone();
        let compression = self.compression;
        let current_tick = self.sessions.simulation_tick();
        let sessions = Arc::clone(&self.sessions);
        let session_id = self.session_id;
        let prewarm_center = (self.center_cx, self.center_cz);
        let progress_notify = Arc::clone(&self.progress_notify);
        let batch = batch
            .into_iter()
            .map(|(request, claim)| {
                let chunk = (request.chunk_x, request.chunk_z);
                (
                    request,
                    PreparedChunkClaimLease::new(Arc::clone(&sessions), chunk, claim),
                )
            })
            .collect::<Vec<_>>();
        let worker_count = resources.prepare_limit().min(batch.len()).max(1);
        let mut pending = batch.into_iter();
        let mut waves = Vec::new();
        loop {
            let wave = pending.by_ref().take(worker_count).collect::<Vec<_>>();
            if wave.is_empty() {
                break;
            }
            waves.push(wave);
        }
        tokio::spawn(async move {
            let _prepare_task = prepare_task;
            for wave in waves {
                if sessions.session_registration_epoch() > session_registration_epoch
                    || !sessions.session_is_at_center(session_id, prewarm_center)
                {
                    break;
                }
                let mut workers = tokio::task::JoinSet::new();
                for (request, prepare_claim) in wave {
                    let world = Arc::clone(&world);
                    let world_read = world_read.clone();
                    let world_mutation = world_mutation.clone();
                    let biomes = Arc::clone(&biomes);
                    let blocks = Arc::clone(&blocks);
                    let block_light = block_light.as_ref().map(Arc::clone);
                    let items = Arc::clone(&items);
                    let tags = Arc::clone(&tags);
                    let recipes = Arc::clone(&recipes);
                    let block_entity_types = Arc::clone(&block_entity_types);
                    let passive_herd_fallback_surfaces =
                        Arc::clone(&passive_herd_fallback_surfaces);
                    let passive_herd_water = Arc::clone(&passive_herd_water);
                    let passive_herd_passable = Arc::clone(&passive_herd_passable);
                    let passive_spawn_rules = Arc::clone(&passive_spawn_rules);
                    let entity_types = Arc::clone(&entity_types);
                    let resources = resources.clone();
                    let dirty_flush = dirty_flush.clone();
                    let sessions = Arc::clone(&sessions);
                    workers.spawn(async move {
                        if sessions.session_registration_epoch() > session_registration_epoch
                            || !sessions.session_is_at_center(session_id, prewarm_center)
                        {
                            return;
                        }
                        let Ok(_request_admission) = resources.acquire_prepare_request().await
                        else {
                            return;
                        };
                        if sessions.session_registration_epoch() > session_registration_epoch
                            || !sessions.session_is_at_center(session_id, prewarm_center)
                        {
                            return;
                        }
                        let result = crate::resource_profile::measure_future(
                            crate::resource_profile::CpuStage::ChunkPreparation,
                            prepare_chunk_request(
                                request,
                                Arc::clone(&world),
                                world_read.clone(),
                                world_mutation,
                                Arc::clone(&biomes),
                                Arc::clone(&blocks),
                                block_light.as_ref().map(Arc::clone),
                                Arc::clone(&items),
                                Arc::clone(&tags),
                                Arc::clone(&recipes),
                                Arc::clone(&block_entity_types),
                                passive_herd_surface,
                                Arc::clone(&passive_herd_fallback_surfaces),
                                Arc::clone(&passive_herd_water),
                                Arc::clone(&passive_herd_passable),
                                Arc::clone(&passive_spawn_rules),
                                Arc::clone(&entity_types),
                                compression,
                                resources.clone(),
                                dirty_flush,
                                Arc::new(AtomicU64::new(request.generation.0)),
                                Arc::new(tokio::sync::Notify::new()),
                                current_tick,
                                None,
                            ),
                        )
                        .await;
                        if let ChunkPrepareOutcome::Ready(prepared) = result.outcome {
                            sessions.cache_prewarmed_chunk(
                                (request.chunk_x, request.chunk_z),
                                prepare_claim.claim().revision,
                                Arc::new(*prepared),
                                PREWARM_PREPARED_CACHE_LIMIT,
                            );
                        }
                    });
                }
                while let Some(result) = workers.join_next().await {
                    if let Err(error) = result {
                        warn!(?error, "forward prewarm worker failed");
                    }
                }
            }
            progress_notify.notify_one();
        });
    }
    pub(super) fn drain_ready(&mut self) {
        self.resources
            .observe_result_queue_depth(self.result_rx.len());
        while let Ok(result) = self.result_rx.try_recv() {
            self.accept_result(result);
        }
    }
    pub(super) fn accept_result(&mut self, result: ChunkPrepareResult) {
        if !self.scheduler.is_current(result.request) {
            self.release_prepare_claim_for_result(&result);
            return;
        }
        if self.ready.contains_key(&result.request.priority.sequence) {
            self.release_prepare_claim_for_result(&result);
            return;
        }
        self.ready.insert(result.request.priority.sequence, result);
        self.max_ready = self.max_ready.max(self.ready.len());
    }
    pub(super) fn clear_ready(&mut self) {
        let ready = std::mem::take(&mut self.ready);
        for result in ready.into_values() {
            self.release_prepare_claim_for_result(&result);
        }
    }
    pub(super) fn close_and_release_buffered_results(&mut self) {
        self.result_rx.close();
        while let Ok(result) = self.result_rx.try_recv() {
            self.release_prepare_claim_for_result(&result);
        }
    }
    fn release_prepare_claim_for_result(&self, result: &ChunkPrepareResult) {
        self.release_prepare_claim(
            (result.request.chunk_x, result.request.chunk_z),
            result.prepare_claim,
        );
    }
    pub(super) fn release_prepare_claim(
        &self,
        chunk: (i32, i32),
        claim: Option<PreparedChunkFence>,
    ) {
        if let Some(PreparedChunkFence::Claimed(claim)) = claim {
            self.sessions.release_prepared_chunk_claim(chunk, claim);
        }
    }
    pub(super) fn record_pressure_flush(&mut self, timing: PressureFlushTiming) {
        self.pressure_flush_runs += timing.runs;
        self.pressure_flush_planned_chunks += timing.planned_chunks;
        self.pressure_flush_flushed_chunks += timing.flushed_chunks;
        self.pressure_flush_plan_ms += timing.plan_ms;
        self.pressure_flush_write_ms += timing.write_ms;
        self.pressure_flush_commit_ms += timing.commit_ms;
        self.max_pressure_flush_plan_ms = self.max_pressure_flush_plan_ms.max(timing.plan_ms);
        self.max_pressure_flush_write_ms = self.max_pressure_flush_write_ms.max(timing.write_ms);
        self.max_pressure_flush_commit_ms = self.max_pressure_flush_commit_ms.max(timing.commit_ms);
    }
}

struct PreparedChunkClaimLease {
    sessions: Arc<SessionRegistry>,
    chunk: (i32, i32),
    claim: Option<PreparedChunkClaim>,
}
impl PreparedChunkClaimLease {
    fn new(sessions: Arc<SessionRegistry>, chunk: (i32, i32), claim: PreparedChunkClaim) -> Self {
        Self {
            sessions,
            chunk,
            claim: Some(claim),
        }
    }

    fn claim(&self) -> PreparedChunkClaim {
        self.claim.expect("prepared claim lease is armed")
    }

    fn disarm(&mut self) {
        self.claim = None;
    }
}
impl Drop for PreparedChunkClaimLease {
    fn drop(&mut self) {
        if let Some(claim) = self.claim.take() {
            self.sessions
                .release_prepared_chunk_claim(self.chunk, claim);
        }
    }
}
