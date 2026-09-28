use super::*;

impl ChunkStreamState {
    pub(in crate::play) fn replan_center(
        &mut self,
        center_cx: i32,
        center_cz: i32,
        direction_yaw: f32,
    ) -> Vec<(i32, i32)> {
        if (self.center_cx, self.center_cz) == (center_cx, center_cz) {
            if (self.direction_yaw - direction_yaw).abs() >= 22.5 && !self.scheduler.is_complete() {
                self.scheduler.reprioritize_queued(prioritized_spiral(
                    center_cx,
                    center_cz,
                    self.view_distance,
                    direction_yaw,
                ));
            }
            self.direction_yaw = direction_yaw;
            return Vec::new();
        }
        let desired = desired_chunk_set(center_cx, center_cz, self.view_distance);
        let unloads: Vec<_> = self.loaded.difference(&desired).copied().collect();
        for chunk in &unloads {
            if self.loaded.remove(chunk) {
                self.release_loaded_world_chunk(*chunk);
            }
        }
        let mut visibility = self.sessions.replace_view(
            self.session_id,
            (center_cx, center_cz),
            self.view_distance,
            desired,
        );
        visibility.extend(self.sessions.mark_unloaded(self.session_id, &unloads));
        dispatch_visibility_commands(visibility);
        self.center_cx = center_cx;
        self.center_cz = center_cz;
        self.direction_yaw = direction_yaw;
        self.clear_ready();
        self.reset_pressure_tracking();
        self.reset_prewarm_tracking();
        self.scheduler.replace_view(prioritized_spiral(
            center_cx,
            center_cz,
            self.view_distance,
            self.direction_yaw,
        ));
        self.publish_active_generation();
        self.reset_window_metrics();
        self.progress_notify.notify_one();
        unloads
    }
    pub(in crate::play) fn replan_view_distance(
        &mut self,
        view_distance: i32,
        direction_yaw: f32,
    ) -> Vec<(i32, i32)> {
        self.client_view_distance_cap = view_distance.max(0);
        let effective_view_distance = self
            .runtime_view_distance_limit
            .min(self.client_view_distance_cap);
        self.replan_effective_view_distance(effective_view_distance, direction_yaw)
    }
    pub(super) fn replan_effective_view_distance(
        &mut self,
        view_distance: i32,
        direction_yaw: f32,
    ) -> Vec<(i32, i32)> {
        let view_distance = view_distance.max(0);
        if self.view_distance == view_distance {
            return Vec::new();
        }
        self.view_distance = view_distance;
        self.direction_yaw = direction_yaw;

        let desired = desired_chunk_set(self.center_cx, self.center_cz, self.view_distance);
        let unloads: Vec<_> = self.loaded.difference(&desired).copied().collect();
        for chunk in &unloads {
            if self.loaded.remove(chunk) {
                self.release_loaded_world_chunk(*chunk);
            }
        }
        let mut visibility = self.sessions.replace_view(
            self.session_id,
            (self.center_cx, self.center_cz),
            self.view_distance,
            desired,
        );
        visibility.extend(self.sessions.mark_unloaded(self.session_id, &unloads));
        dispatch_visibility_commands(visibility);
        self.clear_ready();
        self.reset_pressure_tracking();
        self.reset_prewarm_tracking();
        self.scheduler.replace_view(prioritized_spiral(
            self.center_cx,
            self.center_cz,
            self.view_distance,
            self.direction_yaw,
        ));
        self.publish_active_generation();
        self.reset_window_metrics();
        self.progress_notify.notify_one();
        unloads
    }
    pub(in crate::play) fn replay_current_view(&mut self, direction_yaw: f32) {
        let loaded: Vec<_> = self.loaded.drain().collect();
        for chunk in &loaded {
            self.release_loaded_world_chunk(*chunk);
        }
        dispatch_visibility_commands(self.sessions.mark_unloaded(self.session_id, &loaded));

        let desired = desired_chunk_set(self.center_cx, self.center_cz, self.view_distance);
        dispatch_visibility_commands(self.sessions.replace_view(
            self.session_id,
            (self.center_cx, self.center_cz),
            self.view_distance,
            desired,
        ));
        self.direction_yaw = direction_yaw;
        self.clear_ready();
        self.reset_pressure_tracking();
        self.reset_prewarm_tracking();
        self.scheduler.replay_view(prioritized_spiral(
            self.center_cx,
            self.center_cz,
            self.view_distance,
            self.direction_yaw,
        ));
        self.publish_active_generation();
        self.reset_window_metrics();
        self.progress_notify.notify_one();
        debug!(
            target: "solaris::chunk_visibility",
            event = "chunk_view_replay",
            session_id = self.session_id,
            tick = self.sessions.simulation_tick(),
            generation = self.scheduler.current_generation().0,
            center_cx = self.center_cx,
            center_cz = self.center_cz,
            radius = self.view_distance,
            reason = "respawn",
            "chunk view scheduled for full replay"
        );
    }
    pub(super) fn observe_runtime_control(&mut self) -> Vec<(i32, i32)> {
        let Some(runtime_control) = self.runtime_control.clone() else {
            return Vec::new();
        };
        // Ready results remain scheduler in-flight until publication, so the
        // scheduler count already includes every occupied result slot.
        let queued_chunks = self.scheduler.in_flight_len();
        let snapshot = runtime_control.snapshot();
        let queue_capacity = self.result_queue_size.max(1);
        let saturated = queued_chunks.saturating_mul(100)
            >= queue_capacity.saturating_mul(snapshot.policy.queue_pressure_percent as usize);
        self.set_chunk_queue_saturated(saturated);

        let memory = runtime_control.memory_pressure_observation();
        let memory_pressure_active = if memory.available && memory.sample.limit_mb > 0 {
            memory.sample.used_mb.saturating_mul(100)
                >= memory
                    .sample
                    .limit_mb
                    .saturating_mul(u64::from(snapshot.policy.memory_pressure_percent))
        } else {
            memory.failures > 0
        };
        let should_shed_memory = memory_pressure_active && !self.memory_pressure_active;
        if should_shed_memory {
            self.shed_memory_pressure_work();
        }
        let unloads = self.apply_runtime_control_limits(snapshot.limits);
        if tracing::enabled!(target: "solaris::chunk_visibility", tracing::Level::DEBUG) {
            // Preserve the decision from the snapshot actually applied, not a
            // newer control-plane decision sampled after the socket write.
            self.last_observed_autoscale_decision = Some(snapshot.last_decision);
        }
        self.memory_pressure_active = memory_pressure_active;
        if memory_pressure_active {
            self.set_stop_reason(ChunkPipelineStopReason::MemoryPressure);
        }
        unloads
    }
    fn set_chunk_queue_saturated(&mut self, saturated: bool) {
        if saturated == self.chunk_queue_saturated {
            return;
        }
        self.chunk_queue_saturated = saturated;
        if self
            .chunk_pressure_source
            .as_mut()
            .is_some_and(|source| !source.set_saturated(saturated))
        {
            debug!(saturated, "runtime control signal consumer closed");
        }
    }
    pub(super) fn set_first_chunk_sla_active(&mut self, active: bool) {
        if active == self.first_chunk_sla_active {
            return;
        }
        self.first_chunk_sla_active = active;
        if self
            .first_chunk_sla_source
            .as_mut()
            .is_some_and(|source| !source.set_active(active))
        {
            debug!(active, "runtime control signal consumer closed");
        }
    }
    pub(super) fn recover_runtime_control_sources(&mut self) {
        self.set_chunk_queue_saturated(false);
        self.set_first_chunk_sla_active(false);
    }
    pub(super) fn apply_runtime_control_limits(
        &mut self,
        limits: crate::RuntimeControlLimits,
    ) -> Vec<(i32, i32)> {
        self.policy.chunk_send_rate = limits.chunk_send_rate.max(1);
        self.policy.chunk_load_rate = limits.chunk_load_rate.max(1);
        self.policy.chunk_generate_rate = limits.chunk_generate_rate.max(1);
        self.policy.chunk_prepare_batch_size = self.configured_prepare_batch_size.max(1);
        self.prepare_limit_stop_reason = ChunkPipelineStopReason::BatchLimit;
        self.runtime_view_distance_limit = limits.view_distance.max(0);
        let effective_view_distance = self
            .runtime_view_distance_limit
            .min(self.client_view_distance_cap);
        if effective_view_distance != self.view_distance {
            return self
                .replan_effective_view_distance(effective_view_distance, self.direction_yaw);
        }
        Vec::new()
    }
    pub(super) async fn dispatch_available(&mut self) {
        self.dispatch_turns += 1;
        let started = Instant::now();
        let mut dispatched_this_turn = 0usize;
        let mut load_dispatched_this_turn = 0usize;
        let mut generate_dispatched_this_turn = 0usize;
        let mut budget_deferrals = 0usize;
        let mut claim_deferrals = 0usize;
        loop {
            if self.scheduler.in_flight_len() >= self.result_queue_size {
                self.set_stop_reason(ChunkPipelineStopReason::QueueFull);
                break;
            }
            if self.memory_pressure_active {
                self.set_stop_reason(ChunkPipelineStopReason::MemoryPressure);
                break;
            }
            if dispatched_this_turn >= self.policy.chunk_prepare_batch_size {
                self.set_stop_reason(self.prepare_limit_stop_reason);
                break;
            }
            if self.policy.chunk_prepare_budget_ms > 0
                && started.elapsed().as_millis() as u64 >= self.policy.chunk_prepare_budget_ms
            {
                self.set_stop_reason(ChunkPipelineStopReason::TimeBudget);
                break;
            }
            let Some(request) = self.scheduler.poll_next() else {
                let stop_reason = if self.scheduler.in_flight_len() == 0 {
                    ChunkPipelineStopReason::Complete
                } else {
                    ChunkPipelineStopReason::QueueEmpty
                };
                self.set_stop_reason(stop_reason);
                break;
            };
            let prepared = if self.loader_block_projection.is_some() {
                match self
                    .sessions
                    .prepared_chunk_or_claim_uncached((request.chunk_x, request.chunk_z))
                {
                    PreparedChunkClaimResult::Claimed(claim) => {
                        SessionPreparedChunkClaimResult::Claimed(claim)
                    }
                    PreparedChunkClaimResult::InFlight => SessionPreparedChunkClaimResult::InFlight,
                    PreparedChunkClaimResult::Cached => {
                        unreachable!("uncached Loader projection claims never return cached frames")
                    }
                }
            } else {
                self.sessions.prepared_chunk_or_wait_for_earlier_session(
                    (request.chunk_x, request.chunk_z),
                    self.session_id,
                )
            };
            let prepare_claim = match prepared {
                SessionPreparedChunkClaimResult::Cached(prepared, revision) => {
                    self.accept_result(ChunkPrepareResult {
                        request,
                        prepare_claim: Some(PreparedChunkFence::CachedRevision(revision)),
                        fetch_ms: 0,
                        pressure_flush: PressureFlushTiming::default(),
                        staged: Vec::new(),
                        outcome: ChunkPrepareOutcome::Ready(Box::new(
                            prepared.prepared_cache_hit(),
                        )),
                    });
                    dispatched_this_turn += 1;
                    self.dispatched += 1;
                    budget_deferrals = 0;
                    claim_deferrals = 0;
                    continue;
                }
                SessionPreparedChunkClaimResult::Claimed(claim) => claim,
                SessionPreparedChunkClaimResult::WaitingForEarlierSession => {
                    if !self.scheduler.defer_front(request) {
                        self.set_stop_reason(ChunkPipelineStopReason::QueueEmpty);
                        break;
                    }
                    self.set_stop_reason(ChunkPipelineStopReason::QueueEmpty);
                    break;
                }
                SessionPreparedChunkClaimResult::InFlight => {
                    if !self.scheduler.defer(request) {
                        self.set_stop_reason(ChunkPipelineStopReason::QueueEmpty);
                        break;
                    }
                    claim_deferrals += 1;
                    if claim_deferrals >= PREPARED_IN_FLIGHT_DEFERRAL_LIMIT
                        || claim_deferrals >= self.scheduler.queued_len().max(1)
                    {
                        self.set_stop_reason(ChunkPipelineStopReason::QueueEmpty);
                        break;
                    }
                    continue;
                }
            };
            let budget_class = self.classify_prepare_budget(request).await;
            let budget_exhausted = match budget_class {
                ChunkPrepareBudgetClass::Load => {
                    load_dispatched_this_turn >= self.policy.chunk_load_rate as usize
                }
                ChunkPrepareBudgetClass::Generate => {
                    generate_dispatched_this_turn >= self.policy.chunk_generate_rate as usize
                }
            };
            if budget_exhausted {
                self.release_prepare_claim(
                    (request.chunk_x, request.chunk_z),
                    Some(PreparedChunkFence::Claimed(prepare_claim)),
                );
                let stop_reason = budget_class.stop_reason();
                if !self.scheduler.defer(request) {
                    self.set_stop_reason(stop_reason);
                    break;
                }
                budget_deferrals += 1;
                if budget_deferrals >= self.scheduler.queued_len().max(1) {
                    self.set_stop_reason(stop_reason);
                    break;
                }
                continue;
            }
            match budget_class {
                ChunkPrepareBudgetClass::Load => load_dispatched_this_turn += 1,
                ChunkPrepareBudgetClass::Generate => generate_dispatched_this_turn += 1,
            }
            self.spawn_prepare_worker(request, prepare_claim);
            dispatched_this_turn += 1;
            self.dispatched += 1;
            budget_deferrals = 0;
            claim_deferrals = 0;
        }
        self.max_in_flight = self.max_in_flight.max(self.scheduler.in_flight_len());
    }
    pub(super) async fn classify_prepare_budget(
        &self,
        request: ChunkRequest,
    ) -> ChunkPrepareBudgetClass {
        let position = ChunkPos {
            x: request.chunk_x,
            z: request.chunk_z,
        };
        if let Some(chunk_source) = self.chunk_source.as_ref() {
            return match chunk_source.source_for(position) {
                mc_world::ChunkPrepareSource::Generator => ChunkPrepareBudgetClass::Generate,
                mc_world::ChunkPrepareSource::Resident
                | mc_world::ChunkPrepareSource::RegionFile
                | mc_world::ChunkPrepareSource::Absent => ChunkPrepareBudgetClass::Load,
            };
        }
        if self.world_read.as_ref().is_some_and(|world_read| {
            world_read
                .snapshot_chunks(&[position])
                .chunk(position)
                .is_some()
        }) {
            return ChunkPrepareBudgetClass::Load;
        }
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::ChunkPrepare,
            "chunk prepare budget classify",
            Instant::now(),
            self.world.lock().await,
        );
        match storage.plan_chunk_snapshot_without_generation(position) {
            mc_world::ChunkSnapshotPlan::Cached(_) => ChunkPrepareBudgetClass::Load,
            mc_world::ChunkSnapshotPlan::Load(plan) if plan.has_load_source() => {
                ChunkPrepareBudgetClass::Load
            }
            mc_world::ChunkSnapshotPlan::Load(_) if storage.generator().is_some() => {
                ChunkPrepareBudgetClass::Generate
            }
            mc_world::ChunkSnapshotPlan::Load(_) => ChunkPrepareBudgetClass::Load,
        }
    }
    fn shed_memory_pressure_work(&mut self) {
        let ready = self.ready.len();
        let active = self.scheduler.in_flight_len().saturating_sub(ready);
        self.set_stop_reason(ChunkPipelineStopReason::MemoryPressure);
        if ready == 0 && active == 0 {
            return;
        }

        self.clear_ready();
        self.reset_pressure_tracking();
        self.reset_prewarm_tracking();
        self.scheduler.replace_view(prioritized_spiral(
            self.center_cx,
            self.center_cz,
            self.view_distance,
            self.direction_yaw,
        ));
        self.publish_active_generation();
        self.memory_pressure_shed_runs += 1;
        self.memory_pressure_shed_ready += ready;
        self.memory_pressure_shed_in_flight += active;
    }
    pub(super) fn dispatch_forward_prewarm(&mut self) {
        if self.loader_block_projection.is_some()
            || self.view_distance <= 0
            || self.memory_pressure_active
            || self
                .runtime_control
                .as_ref()
                .is_some_and(|control| control.snapshot().draining)
            || self
                .sessions
                .has_later_session_at_center(self.session_id, (self.center_cx, self.center_cz))
        {
            return;
        }
        let mut batch = Vec::new();
        let session_registration_epoch = self.sessions.session_registration_epoch();
        let Some(player_pose) = self.sessions.player_pose(self.session_id) else {
            return;
        };
        for (sequence, coord) in prewarm_edge_batch_chunks(
            self.center_cx,
            self.center_cz,
            self.view_distance,
            self.direction_yaw,
            player_pose,
        )
        .into_iter()
        .enumerate()
        {
            if self.loaded.contains(&coord) || self.prewarm_in_flight.contains(&coord) {
                continue;
            }
            let prepare_claim = match self.sessions.prepared_chunk_or_claim(coord) {
                PreparedChunkClaimResult::Cached | PreparedChunkClaimResult::InFlight => {
                    continue;
                }
                PreparedChunkClaimResult::Claimed(claim) => claim,
            };
            let request = ChunkRequest {
                chunk_x: coord.0,
                chunk_z: coord.1,
                priority: ChunkPriority {
                    ring: (self.view_distance + 1) as u32,
                    sequence: sequence as u32,
                },
                generation: self.scheduler.current_generation(),
            };
            self.prewarm_in_flight.insert(coord);
            self.prewarm_dispatched += 1;
            batch.push((request, prepare_claim));
        }
        if !batch.is_empty() {
            self.spawn_prewarm_batch_worker(batch, session_registration_epoch);
        }
    }
}

pub(in crate::play) fn desired_chunk_set(
    center_cx: i32,
    center_cz: i32,
    view_distance: i32,
) -> HashSet<(i32, i32)> {
    mc_world::chunk_stream_plan_26_1_2::desired_chunk_set(
        center_cx,
        center_cz,
        view_distance,
        crate::MAX_VIEW_DISTANCE,
    )
}
/// Iterate chunk positions around `(center_x, center_z)` outwards
/// to `view_distance` in chebyshev-ring order. The first cell is the
/// centre; subsequent yields are every cell on ring `r = 1`, then
/// every cell on ring `r = 2`, etc. Within a ring the order is
/// row-major over the bounding square — perceptually this still
/// "spreads" because each ring fills before the next starts.
/// Coverage is identical to a row-major scan: `(2*view_distance +
/// 1)²` cells total, each yielded exactly once. Distances above the
/// protocol limit are capped before allocating or iterating.
pub(in crate::play) fn spiral_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
) -> impl Iterator<Item = (i32, i32)> {
    mc_world::chunk_stream_plan_26_1_2::spiral_chunks(
        center_x,
        center_z,
        view_distance,
        crate::MAX_VIEW_DISTANCE,
    )
    .into_iter()
}
pub(in crate::play) fn prioritized_spiral(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    direction_yaw: f32,
) -> impl Iterator<Item = (i32, i32, ChunkPriority)> {
    mc_world::chunk_stream_plan_26_1_2::prioritize_chunks(
        center_x,
        center_z,
        direction_yaw,
        spiral_chunks(center_x, center_z, view_distance).collect(),
    )
    .into_iter()
    .map(|(cx, cz, priority)| {
        (
            cx,
            cz,
            ChunkPriority {
                ring: priority.ring,
                sequence: priority.sequence,
            },
        )
    })
}
pub(in crate::play) fn prewarm_edge_ring_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    direction_yaw: f32,
) -> Vec<(i32, i32)> {
    mc_world::chunk_stream_plan_26_1_2::prewarm_edge_ring_chunks(
        center_x,
        center_z,
        view_distance,
        crate::MAX_VIEW_DISTANCE,
        direction_yaw,
    )
}
#[cfg(test)]
pub(super) fn prewarm_edge_batch_limit(view_distance: i32) -> usize {
    mc_world::chunk_stream_plan_26_1_2::prewarm_edge_batch_limit(
        view_distance,
        crate::MAX_VIEW_DISTANCE,
        PREWARM_EDGE_RING_LIMIT,
    )
}
pub(super) fn prewarm_edge_batch_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
    direction_yaw: f32,
    player_pose: PlayerPose,
) -> Vec<(i32, i32)> {
    mc_world::chunk_stream_plan_26_1_2::prewarm_edge_batch_chunks(
        (center_x, center_z),
        view_distance,
        crate::MAX_VIEW_DISTANCE,
        direction_yaw,
        (player_pose.x, player_pose.z),
        PREWARM_EDGE_RING_LIMIT,
    )
}
pub(super) fn initial_window_target(view_distance: i32) -> usize {
    mc_world::chunk_stream_plan_26_1_2::initial_window_target(view_distance, INITIAL_CHUNK_MIN_RING)
}
