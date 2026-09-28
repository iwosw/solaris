use super::*;

impl ChunkStreamState {
    pub(super) async fn emit_ready_batch<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<bool, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let limit = self.policy.chunk_send_rate.max(1) as usize;
        let mut sent = 0usize;
        while sent < limit {
            match self.emit_next_ready(writer, light_cache).await? {
                EmitReadyResult::SentPacket => sent += 1,
                EmitReadyResult::DrainedNoPacket => {}
                EmitReadyResult::Blocked | EmitReadyResult::Empty => break,
            }
        }
        if sent == limit && !self.ready.is_empty() {
            self.set_stop_reason(ChunkPipelineStopReason::SendBudget);
        }
        Ok(sent > 0)
    }
    pub(in crate::play) fn log_chunk_unload(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        reason: &'static str,
    ) {
        debug!(
            target: "solaris::chunk_visibility",
            event = "chunk_unload_sent",
            session_id = self.session_id,
            tick = self.sessions.simulation_tick(),
            generation = self.scheduler.current_generation().0,
            chunk_x,
            chunk_z,
            center_cx = self.center_cx,
            center_cz = self.center_cz,
            radius = self.view_distance,
            reason,
            "chunk unload packet written"
        );
    }
    pub(super) async fn emit_next_ready<W>(
        &mut self,
        writer: &mut W,
        light_cache: &mut LightCache,
    ) -> Result<EmitReadyResult, ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let Some((_, result)) = self.ready.pop_first() else {
            return Ok(EmitReadyResult::Empty);
        };
        let request = result.request;
        let cx = request.chunk_x;
        let cz = request.chunk_z;
        let prepare_claim = result.prepare_claim;
        let prepared_revision = prepare_claim.map(PreparedChunkFence::revision);
        self.fetch_ms += result.fetch_ms;
        self.record_pressure_flush(result.pressure_flush);
        self.max_fetch_ms = self.max_fetch_ms.max(result.fetch_ms);
        if result.fetch_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_fetch_chunks += 1;
        }

        match result.outcome {
            ChunkPrepareOutcome::Ready(prepared) => {
                if prepared_revision.is_some_and(|revision| {
                    !self
                        .sessions
                        .prepared_revision_is_current((cx, cz), revision)
                }) {
                    self.release_prepare_claim((cx, cz), prepare_claim);
                    if !self.scheduler.defer(request) {
                        warn!(cx, cz, "stale prepared chunk could not be requeued");
                        return Ok(EmitReadyResult::DrainedNoPacket);
                    }
                    return Ok(EmitReadyResult::Blocked);
                }
                self.clear_pressure_tracking((cx, cz));
                self.staged.extend(result.staged);
                if let Some(light) = prepared.light.clone() {
                    light_cache.insert(ChunkPos { x: cx, z: cz }, light);
                }
                let loaded_chunk = (cx, cz);
                let retention =
                    PendingLoadedChunkRetention::new(self.world_read.as_ref(), loaded_chunk);
                let mut write_timing = prepared.write_timing;
                let socket_write_started = Instant::now();
                if let Err(err) = writer.write_all(&prepared.frame).await {
                    self.release_prepare_claim((cx, cz), prepare_claim);
                    return Err(err.into());
                }
                crate::operator_metrics::record_chunk_streamed();
                crate::operator_metrics::record_outbound_bytes(prepared.frame.len() as u64);
                write_timing.socket_write_ms = socket_write_started.elapsed().as_millis() as u64;
                debug!(
                    target: "solaris::chunk_visibility",
                    event = "chunk_sent",
                    session_id = self.session_id,
                    tick = self.sessions.simulation_tick(),
                    generation = request.generation.0,
                    sequence = request.priority.sequence,
                    chunk_x = cx,
                    chunk_z = cz,
                    prepared_revision = ?prepared_revision,
                    already_loaded = self.loaded.contains(&loaded_chunk),
                    center_cx = self.center_cx,
                    center_cz = self.center_cz,
                    radius = self.view_distance,
                    framed_bytes = prepared.frame.len(),
                    "chunk data packet written"
                );
                let visibility = if let Some(revision) = prepared_revision {
                    let Some(visibility) = self.sessions.mark_loaded_if_prepared_revision_current(
                        self.session_id,
                        (cx, cz),
                        revision,
                    ) else {
                        debug!(
                            target: "solaris::chunk_visibility",
                            event = "chunk_send_invalidated",
                            session_id = self.session_id,
                            tick = self.sessions.simulation_tick(),
                            generation = request.generation.0,
                            sequence = request.priority.sequence,
                            chunk_x = cx,
                            chunk_z = cz,
                            prepared_revision = revision,
                            "chunk invalidated after socket write; requeueing"
                        );
                        light_cache.remove(ChunkPos { x: cx, z: cz });
                        self.release_prepare_claim((cx, cz), prepare_claim);
                        if !self.scheduler.defer(request) {
                            warn!(cx, cz, "invalidated prepared chunk could not be requeued");
                            return Ok(EmitReadyResult::DrainedNoPacket);
                        }
                        return Ok(EmitReadyResult::Blocked);
                    };
                    visibility
                } else {
                    self.sessions.mark_loaded(self.session_id, (cx, cz))
                };
                let newly_loaded = self.loaded.insert(loaded_chunk);
                debug_assert!(
                    newly_loaded,
                    "chunk emitted twice without unload: {loaded_chunk:?}"
                );
                if newly_loaded {
                    retention.transfer_to_loaded();
                }
                if newly_loaded {
                    self.published.push(loaded_chunk);
                }
                for (position, cooking) in &prepared.hydrated_campfires {
                    self.sessions
                        .restore_campfire_cooking(*position, cooking.clone());
                }
                #[cfg(test)]
                let mut visibility = visibility;
                let herd_spawns = prepared.herd_spawns.clone();
                let settlement_spawns = self
                    .world_read
                    .as_ref()
                    .and_then(|world_read| {
                        world_read
                            .snapshot_chunks(&[ChunkPos { x: cx, z: cz }])
                            .chunk(ChunkPos { x: cx, z: cz })
                    })
                    .map_or_else(Vec::new, |chunk| {
                        chunk
                            .settlement_inhabitants()
                            .into_iter()
                            .filter_map(|marker| {
                                settlement_inhabitant_spawn(marker, &self.entity_types, &self.items)
                            })
                            .collect()
                    });
                let settlement_vacant_homes = self
                    .world_read
                    .as_ref()
                    .and_then(|world_read| {
                        world_read
                            .snapshot_chunks(&[ChunkPos { x: cx, z: cz }])
                            .chunk(ChunkPos { x: cx, z: cz })
                    })
                    .map_or_else(Vec::new, |chunk| chunk.settlement_vacant_homes());
                for home in settlement_vacant_homes {
                    let [x, y, z] = home.position;
                    if !self
                        .sessions
                        .register_settlement_vacant_home(home.claim.clone(), Vec3::new(x, y, z))
                    {
                        warn!(
                            claim = home.claim,
                            cx, cz, "settlement vacant-home marker rejected"
                        );
                    }
                }
                if let Some(simulation) = self.simulation.as_ref() {
                    if !settlement_spawns.is_empty()
                        && let Err(error) =
                            simulation.ensure_settlement_inhabitants((cx, cz), settlement_spawns)
                    {
                        warn!(
                            ?error,
                            cx, cz, "simulation settlement inhabitant request rejected"
                        );
                    }
                    self.sessions
                        .register_natural_spawn_templates((cx, cz), herd_spawns);
                } else {
                    self.sessions
                        .register_natural_spawn_templates((cx, cz), herd_spawns.clone());
                    #[cfg(test)]
                    {
                        visibility.extend(
                            self.sessions
                                .ensure_chunk_herd_legacy_for_test((cx, cz), &herd_spawns),
                        );
                    }
                    #[cfg(not(test))]
                    {
                        warn!(cx, cz, "chunk stream has no simulation owner");
                    }
                }
                dispatch_visibility_commands(visibility);
                if self.loader_block_projection.is_none()
                    && let Some(revision) = prepared_revision
                {
                    self.sessions.cache_prepared_chunk_if_current(
                        (cx, cz),
                        revision,
                        Arc::new((*prepared).clone()),
                    );
                }
                self.release_prepare_claim((cx, cz), prepare_claim);
                self.record_stage_maxima(
                    cx,
                    cz,
                    prepared_revision,
                    prepared.build_timing,
                    write_timing,
                );
                self.build_timing.add(prepared.build_timing);
                self.record_emitted(cx, cz, prepared.packet_data_len, write_timing);
            }
            ChunkPrepareOutcome::Absent => {
                self.release_prepare_claim((cx, cz), prepare_claim);
                self.clear_pressure_tracking((cx, cz));
                self.staged.extend(result.staged);
                self.absent += 1;
                info!(cx, cz, "no chunk in storage");
                self.scheduler.mark_finished(request);
                return Ok(EmitReadyResult::DrainedNoPacket);
            }
            ChunkPrepareOutcome::Backpressured => {
                self.release_prepare_claim((cx, cz), prepare_claim);
                self.set_pressure_staged((cx, cz), &result.staged);
                let retries = self.pressure_retries.entry((cx, cz)).or_default();
                *retries += 1;
                if *retries >= CHUNK_BACKPRESSURE_MAX_RETRIES {
                    self.clear_pressure_tracking((cx, cz));
                    self.pressure_abandoned += 1;
                    warn!(
                        cx,
                        cz,
                        retries = CHUNK_BACKPRESSURE_MAX_RETRIES,
                        pressure_abandoned = self.pressure_abandoned,
                        "chunk preparation abandoned after repeated dirty chunk cache pressure"
                    );
                    self.scheduler.mark_finished(request);
                    return Ok(EmitReadyResult::DrainedNoPacket);
                }
                if !self.scheduler.defer(request) {
                    self.clear_pressure_tracking((cx, cz));
                    self.pressure_abandoned += 1;
                    warn!(
                        cx,
                        cz,
                        pressure_abandoned = self.pressure_abandoned,
                        "dirty chunk pressure defer failed after request left in-flight set"
                    );
                    return Ok(EmitReadyResult::Blocked);
                }
                debug!(
                    target: "solaris::profile",
                    cx,
                    cz,
                    retry = *retries,
                    "chunk preparation deferred by dirty chunk cache pressure"
                );
                return Ok(EmitReadyResult::Blocked);
            }
            ChunkPrepareOutcome::Failed(err) => {
                self.release_prepare_claim((cx, cz), prepare_claim);
                self.clear_pressure_tracking((cx, cz));
                return Err(ConnectionError::ChunkPreparation {
                    chunk_x: cx,
                    chunk_z: cz,
                    reason: err,
                });
            }
        }

        self.scheduler.mark_finished(request);
        Ok(EmitReadyResult::SentPacket)
    }
    fn set_pressure_staged(&mut self, coord: (i32, i32), staged: &[(i32, i32)]) {
        self.pressure_staged_by_chunk
            .insert(coord, staged.iter().copied().collect());
    }
    fn clear_pressure_staged(&mut self, coord: (i32, i32)) {
        self.pressure_staged_by_chunk.remove(&coord);
    }
    fn clear_pressure_tracking(&mut self, coord: (i32, i32)) {
        self.pressure_retries.remove(&coord);
        self.clear_pressure_staged(coord);
    }
    pub(super) fn reset_pressure_tracking(&mut self) {
        self.pressure_retries.clear();
        self.pressure_staged_by_chunk.clear();
    }
    pub(super) fn reset_prewarm_tracking(&mut self) {
        self.prewarm_in_flight.clear();
    }
    pub(super) fn pressure_staged_count(&self) -> usize {
        self.pressure_staged_by_chunk
            .values()
            .flat_map(|staged| staged.iter().copied())
            .collect::<HashSet<_>>()
            .len()
    }
    #[cfg(test)]
    pub(super) fn pressure_staged_contains(&self, coord: (i32, i32)) -> bool {
        self.pressure_staged_by_chunk
            .values()
            .any(|staged| staged.contains(&coord))
    }
    #[cfg(test)]
    pub(super) fn pressure_staged_is_empty(&self) -> bool {
        self.pressure_staged_by_chunk
            .values()
            .all(HashSet::is_empty)
    }
    fn record_stage_maxima(
        &mut self,
        cx: i32,
        cz: i32,
        prepared_revision: Option<u64>,
        build_timing: ChunkBuildTiming,
        write_timing: ChunkWriteTiming,
    ) {
        self.max_chunk_data_ms = self.max_chunk_data_ms.max(build_timing.chunk_data_ms);
        self.max_heightmap_ms = self.max_heightmap_ms.max(build_timing.heightmap_ms);
        if build_timing.light_compute_ms > self.max_light_compute_ms {
            self.max_light_compute_ms = build_timing.light_compute_ms;
            self.max_light_compute_chunk = Some((cx, cz));
            self.max_light_compute_revision = prepared_revision;
        }
        self.max_light_encode_ms = self.max_light_encode_ms.max(build_timing.light_encode_ms);
        self.max_packet_encode_ms = self.max_packet_encode_ms.max(write_timing.packet_encode_ms);
        self.max_frame_ms = self.max_frame_ms.max(write_timing.frame_ms);
        self.max_socket_write_ms = self.max_socket_write_ms.max(write_timing.socket_write_ms);
        if build_timing.light_compute_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_light_compute_chunks += 1;
        }
        if write_timing.packet_encode_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_packet_encode_chunks += 1;
        }
        if write_timing.frame_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_frame_chunks += 1;
        }
        if write_timing.socket_write_ms >= CHUNK_STAGE_SLOW_MS {
            self.slow_socket_write_chunks += 1;
        }
    }
    fn record_emitted(
        &mut self,
        cx: i32,
        cz: i32,
        packet_data_len: usize,
        write_timing: ChunkWriteTiming,
    ) {
        self.packet_encode_ms += write_timing.packet_encode_ms;
        self.frame_ms += write_timing.frame_ms;
        self.socket_write_ms += write_timing.socket_write_ms;
        self.framed_bytes += write_timing.framed_bytes;
        self.emitted += 1;
        if self.first_chunk_ms.is_none() {
            let elapsed_ms = self.started.elapsed().as_millis() as u64;
            self.first_chunk_ms = Some(elapsed_ms);
            self.set_first_chunk_sla_active(elapsed_ms > self.first_chunk_sla_target_ms);
        }
        self.record_ring_progress(cx, cz);
        self.bytes += packet_data_len;
    }
    fn record_ring_progress(&mut self, cx: i32, cz: i32) {
        let ring = (cx - self.center_cx).abs().max((cz - self.center_cz).abs()) as usize;
        if let Some(count) = self.ring_emitted.get_mut(ring) {
            *count += 1;
            let needed = if ring == 0 { 1 } else { ring * 8 };
            if *count == needed {
                let elapsed = self.started.elapsed().as_millis() as u64;
                if ring == 1 {
                    self.ring1_complete_ms.get_or_insert(elapsed);
                } else if ring == 2 {
                    self.ring2_complete_ms.get_or_insert(elapsed);
                }
            }
        }
    }
    pub(in crate::play) fn log_summary_once(&mut self) {
        if self.summary_logged {
            return;
        }
        self.summary_logged = true;
        debug!(
            target: "solaris::profile",
            center_cx = self.center_cx,
            center_cz = self.center_cz,
            direction_yaw = self.direction_yaw,
            view_distance = self.view_distance,
            staged = self.staged.len(),
            emitted = self.emitted,
            absent = self.absent,
            pressure_abandoned = self.pressure_abandoned,
            pressure_staged = self.pressure_staged_count(),
            pressure_flush_runs = self.pressure_flush_runs,
            pressure_flush_planned_chunks = self.pressure_flush_planned_chunks,
            pressure_flush_flushed_chunks = self.pressure_flush_flushed_chunks,
            pressure_flush_plan_ms = self.pressure_flush_plan_ms,
            pressure_flush_write_ms = self.pressure_flush_write_ms,
            pressure_flush_commit_ms = self.pressure_flush_commit_ms,
            max_pressure_flush_plan_ms = self.max_pressure_flush_plan_ms,
            max_pressure_flush_write_ms = self.max_pressure_flush_write_ms,
            max_pressure_flush_commit_ms = self.max_pressure_flush_commit_ms,
            memory_pressure_shed_runs = self.memory_pressure_shed_runs,
            memory_pressure_shed_ready = self.memory_pressure_shed_ready,
            memory_pressure_shed_in_flight = self.memory_pressure_shed_in_flight,
            degraded_delivery = self.pressure_abandoned > 0 || self.absent > 0,
            bytes = self.bytes,
            framed_bytes = self.framed_bytes,
            fetch_ms = self.fetch_ms,
            chunk_data_ms = self.build_timing.chunk_data_ms,
            heightmap_ms = self.build_timing.heightmap_ms,
            light_compute_ms = self.build_timing.light_compute_ms,
            light_encode_ms = self.build_timing.light_encode_ms,
            packet_encode_ms = self.packet_encode_ms,
            frame_ms = self.frame_ms,
            socket_write_ms = self.socket_write_ms,
            max_fetch_ms = self.max_fetch_ms,
            max_chunk_data_ms = self.max_chunk_data_ms,
            max_heightmap_ms = self.max_heightmap_ms,
            max_light_compute_ms = self.max_light_compute_ms,
            max_light_compute_chunk = ?self.max_light_compute_chunk,
            max_light_compute_revision = self.max_light_compute_revision,
            max_light_encode_ms = self.max_light_encode_ms,
            max_packet_encode_ms = self.max_packet_encode_ms,
            max_frame_ms = self.max_frame_ms,
            max_socket_write_ms = self.max_socket_write_ms,
            slow_stage_threshold_ms = CHUNK_STAGE_SLOW_MS,
            slow_fetch_chunks = self.slow_fetch_chunks,
            slow_light_compute_chunks = self.slow_light_compute_chunks,
            slow_packet_encode_chunks = self.slow_packet_encode_chunks,
            slow_frame_chunks = self.slow_frame_chunks,
            slow_socket_write_chunks = self.slow_socket_write_chunks,
            chunk_send_rate = self.policy.chunk_send_rate,
            chunk_load_rate = self.policy.chunk_load_rate,
            chunk_generate_rate = self.policy.chunk_generate_rate,
            chunk_prepare_budget_ms = self.policy.chunk_prepare_budget_ms,
            chunk_prepare_batch_size = self.policy.chunk_prepare_batch_size,
            chunk_io_threads = self.policy.chunk_io_threads,
            chunk_worker_threads = self.policy.chunk_worker_threads,
            chunk_result_queue_size = self.policy.chunk_result_queue_size,
            compression_level = ?self.policy.compression_level,
            dispatch_turns = self.dispatch_turns,
            yielded_turns = self.yielded_turns,
            dispatched = self.dispatched,
            prewarm_dispatched = self.prewarm_dispatched,
            in_flight = self.scheduler.in_flight_len(),
            max_in_flight = self.max_in_flight,
            ready = self.ready.len(),
            max_ready = self.max_ready,
            stop_reason = ?self.last_stop_reason,
            first_chunk_ms = self.first_chunk_ms,
            ring1_complete_ms = self.ring1_complete_ms,
            ring2_complete_ms = self.ring2_complete_ms,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "chunk stream finished",
        );
    }
}
