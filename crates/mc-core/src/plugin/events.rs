//! Plugin event observation: sealing, acknowledgement and gameplay event hooks.
//! Moved verbatim from `plugin.rs`.

use super::*;

/// Seal durable callbacks under the completed tick's WAL batch. Observations
/// have independent per-subscriber shedding queues and never halt authority.
pub(crate) fn seal_events(state: &mut TickState, tick: u64) -> Result<bool, &'static str> {
    let Some(runtime) = state.plugin_runtime.as_mut() else {
        return Ok(false);
    };
    // Zone transitions can also be emitted directly by domain command handlers.
    runtime.pending_events.retain(|event| {
        if observation_event(event) {
            runtime.boundary.enqueue_observation(event.clone());
            false
        } else {
            true
        }
    });
    runtime.boundary.seal_observations(tick);
    let pending =
        runtime.pending_events.len() + runtime.pending_changes.len() + runtime.pending_traces.len();
    let retained: usize = runtime
        .committed_events
        .iter()
        .map(|batch| batch.events.len() + batch.changes.len() + batch.traces.len())
        .sum();
    if runtime.delivery_failed {
        return Err("plugin native authority failed: world halted (fail closed)");
    }
    if retained.saturating_add(pending) > mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY {
        return Err("plugin callback backlog overflow: world halted (fail closed)");
    }
    if pending == 0 {
        return Ok(true);
    }
    runtime.committed_events.push_back(CommittedEvents {
        tick,
        events: std::mem::take(&mut runtime.pending_events).into(),
        changes: std::mem::take(&mut runtime.pending_changes),
        traces: std::mem::take(&mut runtime.pending_traces),
    });
    Ok(true)
}

/// Begin guest lifecycle callbacks only after the restored native clock is known.
pub(crate) fn start_events(state: &mut TickState) -> Result<(), CoreError> {
    let Some(runtime) = state.plugin_runtime.as_mut() else {
        return Ok(());
    };
    runtime
        .boundary
        .try_enqueue_event(mc_script::ScriptEvent::server_tick(state.tick))
        .map_err(|e| CoreError::Plugin(format!("initial native timer clock admission: {e:?}")))?;
    runtime.written_clock = Some(state.tick);
    runtime
        .boundary
        .try_enqueue_event(mc_script::ScriptEvent::server_started())
        .map_err(|e| CoreError::Plugin(format!("server.started admission: {e:?}")))?;
    Ok(())
}

pub(crate) fn acknowledge_events(state: &mut TickState, written_tick: u64) {
    let Some(mut runtime) = state.plugin_runtime.take() else {
        return;
    };
    if runtime
        .written_clock
        .is_none_or(|clock| clock < written_tick)
    {
        // The coalesced tick API drains ordinary events before its pending clock,
        // so it cannot order timer creation callbacks behind this ACK watermark.
        match runtime
            .boundary
            .try_enqueue_event(mc_script::ScriptEvent::server_tick(written_tick))
        {
            Ok(()) => {
                runtime.written_clock = Some(written_tick);
            }
            Err(error) => {
                if error != mc_script::ScriptQueueError::Full
                    && runtime
                        .committed_events
                        .iter()
                        .any(|batch| batch.tick <= written_tick && !batch.events.is_empty())
                {
                    runtime.delivery_failed = true;
                }
                state.plugin_runtime = Some(runtime);
                return;
            }
        }
    }
    while runtime
        .committed_events
        .front()
        .is_some_and(|batch| batch.tick <= written_tick)
    {
        let batch = runtime
            .committed_events
            .front_mut()
            .expect("written event batch");
        for (connection, change) in std::mem::take(&mut batch.changes) {
            if let Some(world) = player_world_mut(state, connection) {
                world.push_player(connection, change);
                runtime
                    .stats
                    .command_results
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        for trace in std::mem::take(&mut batch.traces) {
            tracing::info!(target: "plugin.command_result", plugin_id = %trace.owner, session = ?trace.session,
                tick = batch.tick, request = %trace.request, outcome = trace.outcome, "native plugin command journaled");
        }
        let mut blocked = false;
        while let Some(event) = batch.events.front() {
            match runtime.boundary.try_enqueue_event(event.clone()) {
                Ok(()) => {
                    batch.events.pop_front();
                }
                Err(mc_script::ScriptQueueError::Full) => {
                    blocked = true;
                    break;
                }
                Err(_) => {
                    runtime.delivery_failed = true;
                    blocked = true;
                    break;
                }
            }
        }
        if blocked {
            break;
        }
        runtime.committed_events.pop_front();
    }
    state.plugin_runtime = Some(runtime);
}

pub(super) fn command_identity(command: &mc_script::ScriptCommand) -> (Option<u64>, String) {
    use mc_script::ScriptCommand;
    match command {
        ScriptCommand::SendChatMessage { player_id, .. } => {
            (Some(player_id.value()), "send-message".into())
        }
        ScriptCommand::PluginStorageGet { request } => (None, request.request_id().into()),
        ScriptCommand::PluginStorageCompareAndSwap { request } => {
            (None, request.request_id().into())
        }
        ScriptCommand::PluginStorageDelete { request } => (None, request.request_id().into()),
        ScriptCommand::Operation { request } => (None, request.request_id().into()),
        ScriptCommand::InventoryStorageTransaction { transaction } => (
            Some(transaction.player_id().value()),
            transaction.id().into(),
        ),
        ScriptCommand::PlayerInventoryTransaction { transaction } => (
            Some(transaction.player_id().value()),
            transaction.request_id().into(),
        ),
        ScriptCommand::SpawnEntity {
            request_id, actor, ..
        } => (Some(actor.value()), request_id.clone()),
        ScriptCommand::DamageEntity { request } => (None, request.request_id().into()),
        ScriptCommand::SetWorldTime { request } => (None, request.request_id().into()),
        ScriptCommand::SetWorldBlock { request } => (None, request.request_id().into()),
        ScriptCommand::TeleportPlayer { request } => (
            Some(request.player_id().value()),
            request.request_id().into(),
        ),
        ScriptCommand::ListOnlinePlayers { request } => (None, request.request_id().into()),
        ScriptCommand::OpenInventoryMenu { player_id, menu } => {
            (Some(player_id.value()), menu.id().into())
        }
        ScriptCommand::CloseInventoryMenu { player_id, menu_id } => {
            (Some(player_id.value()), menu_id.clone())
        }
        ScriptCommand::UnlockSession { player_id } => {
            (Some(player_id.value()), "unlock-session".into())
        }
        ScriptCommand::UpsertZone { zone } => (None, zone.id().into()),
        ScriptCommand::RemoveZone { zone_id } => (None, zone_id.clone()),
        ScriptCommand::ClientSound { player_id, sound } => {
            (Some(player_id.value()), sound.sound_id().into())
        }
        _ => (None, "native-command".into()),
    }
}

pub(super) fn observation_event(event: &mc_script::ScriptEvent) -> bool {
    use mc_script::ScriptEventKind::*;
    matches!(
        event.kind(),
        PlayerJoined { .. }
            | PlayerLeft { .. }
            | PlayerChat { .. }
            | PlayerBlockBroken { .. }
            | PlayerBlockPlaced { .. }
            | PlayerItemCrafted { .. }
            | PlayerItemPickedUp { .. }
            | PlayerEntityKilled { .. }
            | PlayerEntityInteracted { .. }
            | PlayerDied { .. }
            | PlayerZoneEntered { .. }
            | PlayerZoneExited { .. }
            | ClientBrand { .. }
    )
}

pub(crate) fn enqueue_result(state: &mut TickState, event: Option<mc_script::ScriptEvent>) {
    if let (Some(runtime), Some(event)) = (&mut state.plugin_runtime, event) {
        if observation_event(&event) {
            runtime.boundary.enqueue_observation(event);
        } else if runtime.pending_events.len() < mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY {
            runtime.pending_events.push(event);
        } else {
            runtime.delivery_failed = true;
        }
    }
}

pub(crate) fn observe_input(state: &mut TickState, input: &crate::boundary::PlayerInput) {
    use crate::boundary::PlayerInput;
    if state.plugin_runtime.is_none() {
        return;
    }
    let connection = input.connection();
    match input {
        PlayerInput::Leave { .. } => {
            enqueue_result(
                state,
                Some(mc_script::ScriptEvent::player_left(
                    mc_script::ScriptPlayerId::new(connection.0),
                    "disconnected",
                )),
            );
        }
        PlayerInput::Chat { message, .. } => {
            if let Some(context) = player_context(
                state,
                connection,
                state.plugin_players.is_operator(connection),
            ) {
                enqueue_result(
                    state,
                    Some(mc_script::ScriptEvent::player_chat_with_context(
                        mc_script::ScriptPlayerId::new(connection.0),
                        message,
                        context,
                    )),
                );
            }
        }
        _ => {}
    }
}

/// Refuse only unauthenticated sessions when a required provider disappears.
/// Established players keep their native custody and the world keeps ticking.
pub(super) fn refuse_unavailable_auth(state: &mut TickState) {
    let Some(runtime) = &state.plugin_runtime else {
        return;
    };
    let Some(plugin) = runtime
        .required_auth_plugins
        .iter()
        .find(|plugin| !runtime.boundary.plugin_is_active(plugin))
    else {
        return;
    };
    let pending: Vec<_> = std::iter::once(&*state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .flat_map(|world| {
            world.locked_sessions.iter().copied().filter(|connection| {
                !world
                    .disconnects
                    .iter()
                    .any(|(sealed, _)| sealed == connection)
            })
        })
        .collect();
    if pending.is_empty() {
        return;
    }
    let reason = format!(
        "Authentication unavailable: auth plugin {plugin} retired; restart required. Contact the server operator."
    );
    for connection in pending {
        if let Some(world) = player_world_mut(state, connection) {
            world.push_player(
                connection,
                crate::boundary::PlayerChange::Disconnect {
                    reason: reason.clone(),
                },
            );
            world.disconnects.push((connection, world.tick));
        }
    }
}

pub(crate) fn observe_players(state: &mut TickState) {
    if state.plugin_runtime.is_none() {
        return;
    }
    refuse_unavailable_auth(state);
    let mut current = BTreeMap::new();
    for world in
        std::iter::once(&*state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        for connection in world.connections.all_connections() {
            if let Some(context) = player_context(
                world,
                connection,
                state.plugin_players.is_operator(connection),
            ) {
                current.insert(connection.0, context);
            }
        }
    }
    let runtime = state.plugin_runtime.as_mut().expect("running host");
    let sessions = current
        .iter()
        .flat_map(|(session, context)| {
            [
                (context.uuid().to_owned(), *session),
                (context.username().to_owned(), *session),
            ]
        })
        .collect();
    // Build and drop maps outside the shared host lookup's write guard.
    let previous = std::mem::replace(&mut *runtime.sessions.0.write(), sessions);
    drop(previous);
    for (&session, context) in &current {
        if !runtime.observed_players.contains_key(&session) {
            runtime.boundary.enqueue_observation(
                mc_script::ScriptEvent::player_joined_with_context(
                    mc_script::ScriptPlayerId::new(session),
                    context.clone(),
                ),
            );
        }
    }
    runtime.observed_players = current;
}

pub(super) fn game_mode(
    state: &TickState,
    entity: crate::boundary::EntityId,
) -> Option<mc_script::ScriptGameMode> {
    match state.players.get(&entity)?.game_mode {
        crate::commands::GameMode::Survival => Some(mc_script::ScriptGameMode::Survival),
        crate::commands::GameMode::Creative => Some(mc_script::ScriptGameMode::Creative),
        crate::commands::GameMode::Adventure => Some(mc_script::ScriptGameMode::Adventure),
        _ => None,
    }
}

pub(crate) fn player_died(state: &mut TickState, entity: crate::boundary::EntityId) {
    if state.plugin_runtime.is_none() {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        mc_script::ScriptEvent::try_player_died_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            game_mode(state, entity)?,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

/// Publish one native grave to the package that declared `death_graves`.
///
/// The observation is addressed to that package and rides the durable event
/// queue rather than the droppable observation queue: a shed notice would leave a
/// protected-for-nobody grave behind.
pub(crate) fn player_grave_created(
    state: &mut TickState,
    entity: crate::boundary::EntityId,
    grave: &crate::plugin_graves::Buried,
) {
    let Some(owner) = state
        .plugin_runtime
        .as_ref()
        .and_then(|runtime| runtime.death_graves.clone())
    else {
        return;
    };
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        mc_script::ScriptEvent::try_player_grave_created_with_context(
            owner,
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            &grave.dimension,
            grave.position.x,
            grave.position.y,
            grave.position.z,
            grave.items,
            grave.spilled,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

pub(crate) fn block_event(
    state: &mut TickState,
    entity: crate::boundary::EntityId,
    position: crate::boundary::BlockPos,
    block_state: u32,
    placed: bool,
) {
    if state.plugin_runtime.is_none() {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let block = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(block_state))?;
        let mode = game_mode(state, entity)?;
        if placed {
            mc_script::ScriptEvent::try_player_block_placed_with_context(
                mc_script::ScriptPlayerId::new(connection.0),
                context,
                state.dimension.name(),
                block.block.id.as_str(),
                position.x,
                position.y,
                position.z,
                mode,
            )
            .ok()
        } else {
            mc_script::ScriptEvent::try_player_block_broken_with_context(
                mc_script::ScriptPlayerId::new(connection.0),
                context,
                state.dimension.name(),
                block.block.id.as_str(),
                position.x,
                position.y,
                position.z,
                mode,
            )
            .ok()
        }
    })();
    enqueue_result(state, event);
}

pub(crate) fn pickup_event(
    state: &mut TickState,
    entity: crate::boundary::EntityId,
    item_id: u32,
    count: i32,
) {
    if state.plugin_runtime.is_none() || count <= 0 {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let item = state.registry.items.name_of(item_id)?;
        mc_script::ScriptEvent::try_player_item_picked_up_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            item.as_str(),
            count as u64,
            mc_script::ScriptItemPickupSource::ItemEntity,
            game_mode(state, entity)?,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

pub(crate) fn kill_event(
    state: &mut TickState,
    attacker: crate::boundary::EntityId,
    victim: crate::entities::EntitySlot,
) {
    if state.plugin_runtime.is_none() {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(attacker)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let row = state.entities.body(victim);
        let crate::entities::EntityKind::Mob { type_id } = row.kind else {
            return None;
        };
        let entity_type = state.registry.entity_type_name(type_id)?;
        mc_script::ScriptEvent::try_player_entity_killed_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            mc_script::ScriptEntityId::new(row.id.0 as u64),
            entity_type,
            mc_script::ScriptEntityKillSource::Melee,
            game_mode(state, attacker)?,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

pub(crate) fn damage_attacker(
    source: Option<crate::intent::DamageSource>,
) -> Option<crate::boundary::EntityId> {
    match source? {
        crate::intent::DamageSource::Melee { attacker, .. }
        | crate::intent::DamageSource::Mace { attacker, .. }
        | crate::intent::DamageSource::Sweep { attacker, .. } => Some(attacker),
        crate::intent::DamageSource::Projectile { owner, .. } => Some(owner),
        crate::intent::DamageSource::Explosion { kill_credit, .. } => {
            kill_credit.map(|(source, _)| source)
        }
        crate::intent::DamageSource::Command { cause, .. } => cause.map(|(entity, _)| entity),
    }
}

pub(crate) fn craft_event(
    state: &mut TickState,
    entity: crate::boundary::EntityId,
    item: u32,
    count: i32,
    menu: crate::inventory::MenuKind,
) {
    if state.plugin_runtime.is_none() || count <= 0 {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let item = state.registry.items.name_of(item)?;
        let source = match menu {
            crate::inventory::MenuKind::Player => mc_script::ScriptCraftingSource::Inventory,
            crate::inventory::MenuKind::CraftingTable => {
                mc_script::ScriptCraftingSource::CraftingTable
            }
            _ => return None,
        };
        mc_script::ScriptEvent::try_player_item_crafted_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            item.as_str(),
            count as u64,
            1,
            source,
            game_mode(state, entity)?,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

pub(crate) fn fail_native(state: &mut TickState) {
    if let Some(runtime) = state.plugin_runtime.as_mut() {
        runtime.delivery_failed = true;
    }
}

pub(crate) fn entity_interaction(
    state: &mut TickState,
    actor: crate::boundary::EntityId,
    target: crate::boundary::EntityId,
    hand: crate::boundary::Hand,
    secondary: bool,
    intent: Option<&crate::intent::Intent>,
) {
    if state.plugin_runtime.is_none() {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(actor)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let slot = state.entities.slot_of_any(target)?;
        let type_id = match state.entities.body(slot).kind {
            crate::entities::EntityKind::Player { .. } => crate::entities::player_type_id(),
            crate::entities::EntityKind::Mob { type_id }
            | crate::entities::EntityKind::Item { type_id }
            | crate::entities::EntityKind::ExperienceOrb { type_id } => type_id,
        };
        let entity_type = state.registry.entity_type_name(type_id)?;
        let hand = match hand {
            crate::boundary::Hand::Main => mc_script::ScriptInteractionHand::MainHand,
            crate::boundary::Hand::Off => mc_script::ScriptInteractionHand::OffHand,
        };
        mc_script::ScriptEvent::try_player_entity_interacted_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            mc_script::ScriptEntityId::new(target.0 as u64),
            entity_type,
            hand,
            secondary,
            game_mode(state, actor)?,
        )
        .ok()
    })();
    let Some(event) = event else {
        return;
    };
    if let Some(intent) = intent {
        if state.plugin_interactions.len() >= mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY {
            fail_native(state);
        } else {
            state
                .plugin_interactions
                .insert((intent.actor, intent.seq), event);
        }
    } else {
        enqueue_result(state, Some(event));
    }
}
