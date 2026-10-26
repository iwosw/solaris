//! Tick output pump: outbox routing, publication and join waiting.
//! Moved verbatim from `server.rs`.

use super::*;

/// Single pump task: subscribes to the core, records tick stats, drains the
/// reliable outbox FIRST, then publishes the world snapshot and routes both.
/// `Lagged` = world-skip (viewers reconcile on the next output) but the outbox is still
/// drained: reliable results are never skipped. Fanout runs on its own
/// rayon pool: on the global pool the core tick's parallel phases queued
/// behind whole publishes.
pub(super) fn spawn_pump(server: Arc<EdgeServer>) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let mut rx = server.hub.subscribe();
    let pool = rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("edge-fanout-{i}"))
        .build()?;
    // Hide enclosed ores in encoded chunks per `[anti_xray]` (built from the
    // core's exact light/opacity oracle, shared by every viewer via the cache).
    let anti_xray = crate::anti_xray::AntiXray::new(
        &server.wire.blocks,
        &server.hub.registry().light,
        &server.cfg.anti_xray,
    )?
    .map(Arc::new);
    Ok(tokio::spawn(async move {
        // Viewer world-view state keyed by connection: what each client
        // holds, reconciled against every newest snapshot.
        let mut viewers: HashMap<ConnectionId, crate::plan::ViewerState> = HashMap::new();
        // Encoded chunk packets shared across viewers: light compute costs
        // ~28 ms/chunk, so a full 81-chunk re-plan (2.3 s) always outruns
        // the 32-tick broadcast window (1.6 s) and the pump would lag,
        // resync, and recompute forever without ever routing. The cache
        // makes re-plans O(1) per chunk; block changes evict their chunk.
        let mut chunk_cache = crate::plan::ChunkPacketCache::with_anti_xray(anti_xray);
        loop {
            let out = match rx.recv().await {
                Ok(out) => out,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // World-skip, not data loss: the next received output
                    // is reconciled against each viewer's held state. The
                    // reliable outbox is drained regardless (never
                    // skipped). A lag is never fatal: the next `recv`
                    // either returns the newest output or reports a further
                    // lag handled here again.
                    server.dashboard.record_pump_skipped(skipped);
                    for viewer in viewers.values_mut() {
                        viewer.skipped = true;
                    }
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // Drain terminal reliable disconnects before closing, both
                    // for graceful shutdown and for an authoritative failure.
                    let (mut connections, _) = server.connections.routing_snapshot();
                    route_outbox(&server, &mut viewers, &mut connections);
                    if server.hub.shutdown_requested() {
                        tracing::info!(
                            connections = connections.len(),
                            "core tick broadcast closed after shutdown request"
                        );
                    } else {
                        tracing::error!(
                            connections = connections.len(),
                            "core tick broadcast closed unexpectedly; core halted"
                        );
                    }
                    break;
                }
            };
            server.tick_stats.record(out.tick);
            let (mut connections, roster) = server.connections.routing_snapshot();
            route_outbox(&server, &mut viewers, &mut connections);
            server.console_state.drained(out.tick);
            let publish_start = std::time::Instant::now();
            publish_and_route(
                &server,
                &pool,
                &mut viewers,
                &out,
                &mut chunk_cache,
                &mut connections,
            )
            .await;
            let publish_us = publish_start
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            server
                .dashboard
                .record_pump_tick(crate::dashboard::PumpTick {
                    tick: out.tick,
                    core_us: out.times.total_us,
                    publish_us,
                    bytes_written: server.net_counters.bytes_written.load(Ordering::Relaxed),
                    reliable_drops: server.net_counters.reliable_drops.load(Ordering::Relaxed),
                    chunks: DashboardChunks {
                        loaded_total: out.snapshot.chunks.len(),
                    },
                    entities: DashboardEntities {
                        total: out.snapshot.entities.len(),
                    },
                    players: roster.map(|names| crate::dashboard::DashboardPlayers {
                        count: names.len(),
                        max: server.cfg.server.max_players,
                        names,
                    }),
                });
            tracing::debug!(
                tick = out.tick,
                viewers = viewers.len(),
                elapsed_us = publish_us,
                "pump published tick"
            );
            // Yield after every publish. Routing wakes connection tasks into
            // this worker's unstealable LIFO slot, and `rx.recv()` returns
            // Ready while outputs are queued, so a catching-up pump never
            // reached an await point: a Java client's writer sat queued
            // behind it with 512 routed chunks for 5+ s (stuck loading).
            tokio::task::yield_now().await;
        }
    }))
}

/// Drain `Core::try_drain_outbox` and route reliable changes FIRST (before
/// the world publish). Each `PlayerChange` is mapped to framed reliable
/// packets via `plan::plan_player_changes` (single-sourced with the world
/// plan; includes `GameModeChanged`/cursor). A full reliable queue
/// disconnects; changes for unknown/gone connections are dropped with the
/// connection.
pub(super) fn configured_viewer(
    server: &EdgeServer,
    connection: ConnectionId,
    entity: mc_core::boundary::EntityId,
    position: [f64; 3],
    dimension: mc_core::dimensions::Dimension,
) -> crate::plan::ViewerState {
    let mut viewer = crate::plan::ViewerState::new(connection, entity, position);
    viewer.dimension = dimension;
    viewer.view_distance = server
        .connections
        .inner
        .lock()
        .view_distances
        .get(&connection)
        .copied();
    viewer.dimension_type_ids = server.wire.dimension_type_ids;
    viewer.advancements = Arc::clone(&server.recipes.advancements);
    viewer
}

pub(super) fn route_outbox(
    server: &Arc<EdgeServer>,
    viewers: &mut HashMap<ConnectionId, crate::plan::ViewerState>,
    connections: &mut HashMap<ConnectionId, ConnectionHandles>,
) {
    // Ensure every live connection has a viewer entry before planning.
    // The entity id comes from the stashed `Joined` when available (the
    // core allocates it; ids diverge from connection ids in ordinary
    // operation). Otherwise fall back to spawn position [0.5,64,0.5] with
    // the entity still unknown — `plan_player_changes` adopts it from the
    // first `Joined` it plans.
    for &id in connections.keys() {
        viewers.entry(id).or_insert_with(|| {
            let (entity, position, dimension) = server
                .connections
                .take_join(id)
                .map(|(entity, position, _, dimension)| (entity, position, dimension))
                .unwrap_or((
                    mc_core::boundary::EntityId(id.0 as i32),
                    [0.5, 64.0, 0.5],
                    Default::default(),
                ));
            configured_viewer(server, id, entity, position, dimension)
        });
    }
    for (connection, changes) in server.hub.try_drain_outbox() {
        if connection == mc_core::command_console::CONNECTION {
            crate::console::output(&server.console_state, &changes);
            continue;
        }
        // Core-ordered kick: stash the reason so the play loop can log
        // peer+reason after flushing the kick packet. The packet itself
        // is planned below (reliable, last) and routed in order.
        for change in changes.iter() {
            if let mc_core::boundary::PlayerChange::Disconnect { reason, .. } = change {
                server
                    .connections
                    .stash_disconnect_reason(connection, reason.clone());
                break;
            }
        }
        // Join race: the pump may drain before the connection registered
        // its queues (join-wait runs pre-registration). Stash the Joined
        // for `wait_for_join` AND the whole ordered reliable list for
        // replay at registration — dropping the tail would lose the
        // authoritative MenuContents/Health/Experience burst.
        // A registration can race the tick snapshot: a newcomer's handles are
        // looked up live, and if it is still unregistered its whole ordered
        // list (Joined first, for `wait_join`) is stashed for replay at
        // registration, atomically with that lookup.
        let mut changes = changes;
        if let std::collections::hash_map::Entry::Vacant(slot) = connections.entry(connection) {
            let Some(handles) = server
                .connections
                .handles_or_stash(connection, &mut changes)
            else {
                continue;
            };
            slot.insert(handles);
            let (entity, position, dimension) = server
                .connections
                .take_join(connection)
                .map(|(entity, position, _, dimension)| (entity, position, dimension))
                .unwrap_or((
                    mc_core::boundary::EntityId(connection.0 as i32),
                    [0.5, 64.0, 0.5],
                    Default::default(),
                ));
            viewers.entry(connection).or_insert_with(|| {
                configured_viewer(server, connection, entity, position, dimension)
            });
        }
        let Some(handles) = connections.get(&connection) else {
            continue;
        };
        let Some(viewer) = viewers.get_mut(&connection) else {
            continue;
        };
        // The authenticated roster is also the edge's unlock notification.
        // Restore the private operator command tree once, not on every tick.
        if !viewer.player_profiles.contains_key(&viewer.entity_id)
            && let Some(uuid) = changes.iter().find_map(|change| match change {
                mc_core::boundary::PlayerChange::Roster { players, .. } => players
                    .iter()
                    .find(|player| player.connection == connection)
                    .map(|player| player.uuid),
                _ => None,
            })
            && server
                .send(
                    connection,
                    &crate::command_tree::tree(
                        server.admin.level(uuid),
                        server.hub.plugin_commands(),
                    ),
                )
                .is_err()
        {
            connections.remove(&connection);
            viewers.remove(&connection);
            continue;
        }
        for planned in crate::plan::plan_player_changes(&changes, viewer) {
            if handles.reliable_tx.try_send(planned.data).is_err() {
                server.connections.retire(connection);
                connections.remove(&connection);
                server
                    .net_counters
                    .reliable_drops
                    .fetch_add(1, Ordering::Relaxed);
                viewers.remove(&connection);
                break;
            }
        }
    }
}

/// Publish one tick to all viewers and route the addressed packets.
/// Reliable-overflow victims disconnect.
pub(super) async fn publish_and_route(
    server: &Arc<EdgeServer>,
    pool: &rayon::ThreadPool,
    viewers: &mut HashMap<ConnectionId, crate::plan::ViewerState>,
    out: &TickOutput,
    chunk_cache: &mut crate::plan::ChunkPacketCache,
    connections: &mut HashMap<ConnectionId, ConnectionHandles>,
) {
    viewers.retain(|id, _| connections.contains_key(id));
    // Only the pump sends world packets; available slots can only grow
    // between this credit read and routing.
    {
        let registry = server.connections.inner.lock();
        for (id, viewer) in viewers.iter_mut() {
            viewer.view_distance = registry.view_distances.get(id).copied();
            viewer.world_credit = connections
                .get(id)
                .map_or(0, |handles| handles.world_tx.capacity());
        }
    }

    let view_distance = i32::from(server.cfg.server.view_distance);
    // Real sidecar registries: the `publish` single-biome fallback would
    // fail every non-plains chunk encode and send no terrain at all.
    let registries = crate::plan::PlanRegistries {
        biomes: &server.wire.biomes,
        blocks: &server.wire.blocks,
        items: &server.wire.items,
        block_entity_types: &server.wire.block_entity_types,
    };
    let packets = pool.install(|| {
        crate::fanout::publish_with_registries(
            out,
            viewers.values_mut(),
            view_distance,
            &registries,
            chunk_cache,
        )
    });
    // Reliable overflow takes precedence: disconnect victims first.
    for victim in crate::fanout::reliable_overflow(&packets) {
        server.connections.retire(victim);
        connections.remove(&victim);
        server
            .net_counters
            .reliable_drops
            .fetch_add(1, Ordering::Relaxed);
        viewers.remove(&victim);
    }
    for packet in packets {
        let Some(handles) = connections.get(&packet.connection) else {
            continue;
        };
        match packet.priority {
            crate::fanout::Priority::Reliable => {
                // In-order, never dropped: a full reliable queue
                // disconnects the connection.
                if handles.reliable_tx.try_send(packet.data).is_err() {
                    server.connections.retire(packet.connection);
                    connections.remove(&packet.connection);
                    server
                        .net_counters
                        .reliable_drops
                        .fetch_add(1, Ordering::Relaxed);
                    viewers.remove(&packet.connection);
                }
            }
            crate::fanout::Priority::BestEffort => {
                // Planned within the queue's free slots: this fails only
                // when the connection has closed.
                let _ = handles.world_tx.try_send(packet.data);
            }
        }
    }
}

pub(super) fn effective_view_distance(client: i8, server: u8) -> i32 {
    i32::from(client).max(2).min(i32::from(server))
}

/// Wait (bounded) for our [`PlayerChange::Joined`]. Only the pump drains
/// the core outbox; this waiter never touches it. Fast path: the pump may
/// have stashed our Joined already. Otherwise register a one-shot waiter
/// (fired by the pump's [`stash_join`][ConnectionRegistry::stash_join])
/// and await it — no polling, no draining, no other connection's changes.
pub(super) async fn wait_for_join(
    server: &Arc<EdgeServer>,
    id: ConnectionId,
    _name: &str,
    client_view_distance: i8,
    client_language: &str,
) -> anyhow::Result<JoinedSpawn> {
    {
        let mut inner = server.connections.inner.lock();
        inner.view_distances.insert(
            id,
            effective_view_distance(client_view_distance, server.cfg.server.view_distance),
        );
        // Already submitted before Join: remember it so the first Play
        // ClientInformation does not repeat the same locale.
        inner.locales.insert(id, client_language.to_owned());
    }
    // Fast path: Joined arrived before we registered interest.
    if let Some(joined) = server.connections.take_join(id) {
        return Ok(joined);
    }
    let receiver = server.connections.wait_join(id);
    // Re-check after registering: the pump may have stashed between the
    // fast-path check and `wait_join` (stash fires waiters, but ours was
    // registered after the stash — `take_join` covers that window).
    if let Some(joined) = server.connections.take_join(id) {
        return Ok(joined);
    }
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(deadline);
    tokio::select! {
        biased;
        _ = &mut deadline => anyhow::bail!("timed out waiting for join"),
        result = receiver => {
            result.map_err(|_| anyhow::anyhow!("join waiter dropped before Joined"))
        }
    }
}

/// Newest tick output available without blocking (for the join burst).
pub(super) fn latest_output(server: &Arc<EdgeServer>) -> TickOutput {
    let mut rx = server.hub.subscribe();
    // A fresh broadcast subscriber only sees outputs published after
    // subscribing; the join lands on the next tick, so fall back to an
    // empty output and let the burst carry position data.
    match rx.try_recv() {
        Ok(out) => out,
        Err(_) => empty_output(),
    }
}

pub(super) fn empty_output() -> TickOutput {
    TickOutput {
        tick: 0,
        snapshot: Arc::new(mc_core::boundary::WorldSnapshot {
            tick: 0,
            day_time: 0,
            chunks: Arc::default(),
            day_time_rate: 1.0,
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    }
}
