use std::sync::Arc;

use mc_script::{
    ScriptOperation, ScriptOperationFailure, ScriptOperationRequest, ScriptOperationState,
    ScriptPluginStorageVersionGuard, ScriptStorageMutation,
};

use crate::plugin::{PluginState, operation};
use crate::tick::TickState;

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, position: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            position,
            mc_world::BlockStateId(self.0),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}

fn fixture() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn batch(
    request: &str,
    operation_id: &str,
    mutations: Vec<ScriptStorageMutation>,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        request,
        ScriptOperation::StorageBatch {
            operation_id: operation_id.into(),
            mutations,
        },
    )
    .unwrap()
}

#[test]
fn storage_namespaces_and_monotone_deletion_tombstones_share_the_native_image() {
    let mut image = PluginState::default();
    let first = ScriptStorageMutation::compare_and_swap("balance", None, "10").unwrap();
    image.apply_storage("first", &[first]);
    let version = image.version("first", "balance").unwrap();
    let other = ScriptStorageMutation::compare_and_swap("balance", None, "99").unwrap();
    image.apply_storage("second", &[other]);
    assert_eq!(
        image.storage_value_version("first", "balance"),
        Some((Some("10"), version))
    );
    let deleted = ScriptStorageMutation::delete("balance", Some(version)).unwrap();
    image.apply_storage("first", &[deleted]);
    let (value, tombstone_revision) = image.storage_value_version("first", "balance").unwrap();
    assert_eq!(value, None);
    assert!(tombstone_revision > version);
    assert_eq!(image.version("first", "balance"), None);
    assert!(!image.check_storage(
        "first",
        &[ScriptStorageMutation::compare_and_swap("balance", Some(version), "old").unwrap()]
    ));
    image.apply_storage(
        "first",
        &[ScriptStorageMutation::compare_and_swap("balance", None, "20").unwrap()],
    );
    assert!(image.version("first", "balance").unwrap() > tombstone_revision);
    assert_eq!(
        image.record("second", "balance").unwrap().value.as_deref(),
        Some("99")
    );
    image.validate().unwrap();
}

#[test]
fn storage_batch_checks_every_compare_before_any_key_is_changed() {
    let mut state = fixture();
    state.plugins.apply_storage(
        "owner",
        &[ScriptStorageMutation::compare_and_swap("a", None, "before").unwrap()],
    );
    let version = state.plugins.version("owner", "a").unwrap();
    let request = batch(
        "request",
        "atomic",
        vec![
            ScriptStorageMutation::compare_and_swap("a", Some(version), "after").unwrap(),
            ScriptStorageMutation::compare_and_swap("b", Some(version), "must-not-appear").unwrap(),
        ],
    );
    let result = operation(&mut state, "owner", &request);
    assert_eq!(
        result.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(
        state.plugins.storage_value_version("owner", "a"),
        Some((Some("before"), version))
    );
    assert_eq!(state.plugins.record("owner", "b"), None);
}

#[test]
fn same_operation_retry_returns_original_receipt_before_rechecking_changed_guards() {
    let mut state = fixture();
    state.plugins.apply_storage(
        "owner",
        &[ScriptStorageMutation::compare_and_swap("guard", None, "before").unwrap()],
    );
    let version = state.plugins.version("owner", "guard").unwrap();
    let request = batch(
        "first-request",
        "once",
        vec![ScriptStorageMutation::compare_and_swap("guard", Some(version), "after").unwrap()],
    )
    .with_expected_plugin_storage_versions(vec![ScriptPluginStorageVersionGuard::new(
        "guard".into(),
        version,
    )])
    .unwrap();
    let first = operation(&mut state, "owner", &request);
    assert_eq!(first.state(), ScriptOperationState::Committed);
    let retry = batch(
        "different-request",
        "once",
        vec![ScriptStorageMutation::compare_and_swap("guard", Some(version), "after").unwrap()],
    )
    .with_expected_plugin_storage_versions(vec![ScriptPluginStorageVersionGuard::new(
        "guard".into(),
        version,
    )])
    .unwrap();
    let before = serde_json::to_vec(&state.plugins).unwrap();
    assert_eq!(operation(&mut state, "owner", &retry), first);
    assert_eq!(serde_json::to_vec(&state.plugins).unwrap(), before);
    let conflict = batch(
        "conflicting-request",
        "once",
        vec![
            ScriptStorageMutation::compare_and_swap("guard", Some(version), "different-effect")
                .unwrap(),
        ],
    );
    assert_eq!(
        operation(&mut state, "owner", &conflict).failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
}

#[test]
fn checkpoint_and_wal_replay_preserve_receipt_and_tombstone_without_double_apply() {
    let mut state = fixture();
    let request = batch(
        "write",
        "write-once",
        vec![ScriptStorageMutation::compare_and_swap("value", None, "saved").unwrap()],
    );
    let original = operation(&mut state, "owner", &request);
    let mut initial_entries = Vec::new();
    crate::plugin::journal(&mut state, &mut initial_entries);
    let base: PluginState =
        serde_json::from_slice(&serde_json::to_vec(&state.plugins).unwrap()).unwrap();
    let version = state.plugins.version("owner", "value").unwrap();
    let delete = batch(
        "delete",
        "delete-once",
        vec![ScriptStorageMutation::delete("value", Some(version)).unwrap()],
    );
    operation(&mut state, "owner", &delete);
    let mut entries = Vec::new();
    crate::plugin::journal(&mut state, &mut entries);
    assert!(matches!(
        entries.as_slice(),
        [crate::journal::JournalEntry::Plugins { .. }]
    ));
    let batches = vec![(1, 1, entries)];
    let replay = crate::journal::Replay {
        plugins: base,
        ..Default::default()
    };
    let replay = crate::journal::replay_batches(replay, &batches);
    let replay = crate::journal::replay_batches(replay, &batches);
    let mut recovered = fixture();
    recovered.plugins = replay.plugins;
    assert_eq!(
        recovered.plugins.record("owner", "value").unwrap().value,
        None
    );
    assert_eq!(operation(&mut recovered, "owner", &request), original);
    assert_eq!(
        recovered.plugins.record("owner", "value").unwrap().value,
        None
    );
    assert!(recovered.plugins.record("owner", "value").unwrap().revision > version);
    recovered.plugins.validate().unwrap();
}

#[test]
fn storage_scan_skips_tombstones_and_cursor_is_exclusive() {
    let mut state = fixture();
    state.plugins.apply_storage(
        "owner",
        &[
            ScriptStorageMutation::compare_and_swap("entry/a", None, "a").unwrap(),
            ScriptStorageMutation::compare_and_swap("entry/b", None, "b").unwrap(),
            ScriptStorageMutation::compare_and_swap("entry/c", None, "c").unwrap(),
        ],
    );
    let version = state.plugins.version("owner", "entry/b").unwrap();
    state.plugins.apply_storage(
        "owner",
        &[ScriptStorageMutation::delete("entry/b", Some(version)).unwrap()],
    );
    let request = ScriptOperationRequest::try_new(
        "scan",
        ScriptOperation::StorageScan {
            prefix: "entry/".into(),
            cursor: None,
            limit: 1,
        },
    )
    .unwrap();
    let result = operation(&mut state, "owner", &request);
    let mc_script::ScriptOperationPayload::StoragePage { entries, cursor } = result.payload()
    else {
        panic!("storage page");
    };
    assert_eq!(entries[0].key, "entry/a");
    assert_eq!(cursor.as_deref(), Some("entry/a"));
    let request = ScriptOperationRequest::try_new(
        "next",
        ScriptOperation::StorageScan {
            prefix: "entry/".into(),
            cursor: cursor.clone(),
            limit: 1,
        },
    )
    .unwrap();
    let result = operation(&mut state, "owner", &request);
    let mc_script::ScriptOperationPayload::StoragePage { entries, cursor } = result.payload()
    else {
        panic!("storage page");
    };
    assert_eq!(entries[0].key, "entry/c");
    assert_eq!(cursor, &None);
}

#[test]
fn startup_policy_is_checkpoint_owned_and_mismatches_fail_closed() {
    let mut state = fixture();
    crate::plugin::validate_startup(&mut state, true).unwrap();
    crate::plugin::validate_startup(&mut state, false).unwrap();
    state
        .native_globals
        .get_mut("plugin_startup")
        .unwrap()
        .payload
        .push(b' ');
    assert!(crate::plugin::validate_startup(&mut state, false).is_err());
}

#[test]
fn native_wal_contains_only_touched_keys_and_replays_domain_removal_and_null() {
    let mut state = fixture();
    let old = batch(
        "old-request",
        "old-operation",
        vec![ScriptStorageMutation::compare_and_swap("untouched", None, "keep").unwrap()],
    );
    let old_outcome = operation(&mut state, "owner", &old);
    state
        .plugins
        .set_domain("owner", "keep", serde_json::json!({"cargo": [1, 2, 3]}));
    state
        .plugins
        .set_domain("owner", "remove", serde_json::json!({"before": true}));
    let mut entries = Vec::new();
    crate::plugin::journal(&mut state, &mut entries);
    let mut replay = crate::journal::Replay {
        plugins: serde_json::from_slice(&serde_json::to_vec(&state.plugins).unwrap()).unwrap(),
        ..Default::default()
    };
    let current = batch(
        "new-request",
        "new-operation",
        vec![ScriptStorageMutation::compare_and_swap("changed", None, "after").unwrap()],
    );
    operation(&mut state, "owner", &current);
    state.plugins.delete_domain("owner", "remove");
    state
        .plugins
        .set_domain("owner", "null", serde_json::Value::Null);
    state.plugins.get_domain_mut("owner", "keep").unwrap()["cargo"] = serde_json::json!([4]);
    entries.clear();
    crate::plugin::journal(&mut state, &mut entries);
    let [crate::journal::JournalEntry::Plugins { value }] = entries.as_slice() else {
        panic!("one delta");
    };
    let encoded = serde_json::to_value(value).unwrap();
    assert_eq!(encoded["storage"]["owner"].as_object().unwrap().len(), 1);
    assert!(encoded["storage"]["owner"].get("untouched").is_none());
    assert_eq!(encoded["receipts"]["owner"].as_object().unwrap().len(), 1);
    assert!(encoded["receipts"]["owner"].get("old-operation").is_none());
    let batches = [(2, 2, entries)];
    replay = crate::journal::replay_batches(replay, &batches);
    replay = crate::journal::replay_batches(replay, &batches);
    assert_eq!(replay.plugins.get_domain("owner", "remove"), None);
    assert_eq!(
        replay.plugins.get_domain("owner", "null"),
        Some(&serde_json::Value::Null)
    );
    assert_eq!(
        replay.plugins.get_domain("owner", "keep"),
        Some(&serde_json::json!({"cargo": [4]}))
    );
    assert_eq!(
        replay
            .plugins
            .storage_value_version("owner", "untouched")
            .unwrap()
            .0,
        Some("keep")
    );
    assert_eq!(
        serde_json::to_vec(&replay.plugins).unwrap(),
        serde_json::to_vec(&state.plugins).unwrap()
    );
    state.plugins = replay.plugins;
    assert_eq!(operation(&mut state, "owner", &old), old_outcome);
    let status = ScriptOperationRequest::try_new(
        "long-ago-status",
        ScriptOperation::Status {
            operation_id: "old-operation".into(),
        },
    )
    .unwrap();
    assert_eq!(operation(&mut state, "owner", &status), old_outcome);
    let mut retry_entries = Vec::new();
    crate::plugin::journal(&mut state, &mut retry_entries);
    assert!(retry_entries.is_empty());
    state.plugins.validate().unwrap();
}

fn join_native_player(state: &mut TickState) -> crate::boundary::EntityId {
    let connection = crate::boundary::ConnectionId(1);
    crate::tick::run_tick(
        state,
        &[crate::boundary::PlayerInput::Join {
            connection,
            uuid: uuid::Uuid::new_v4(),
            name: "NativePlayer".into(),
            permission_level: 0,
        }],
    );
    state.connections.entity_of(connection).unwrap()
}

#[test]
fn teleport_of_connected_dead_player_refuses_without_native_side_effects() {
    let mut state = fixture();
    let entity = join_native_player(&mut state);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.body_mut(slot).alive = false;
    let position = state.entities.pos(slot);
    let player = state.players[&entity].clone();
    let request = mc_script::ScriptPlayerTeleportRequest::try_new(
        "dead-teleport",
        mc_script::ScriptPlayerId::new(1),
        "minecraft:overworld",
        mc_script::ScriptPosition::try_new(20.0, 80.0, 20.0).unwrap(),
    )
    .unwrap();
    assert_eq!(
        crate::plugin::teleport_player(&mut state, &request),
        Some(mc_script::ScriptPlayerTeleportFailure::PlayerUnavailable)
    );
    assert_eq!(state.entities.pos(slot), position);
    assert_eq!(state.players[&entity], player);
    assert_eq!(
        state
            .connections
            .entity_of(crate::boundary::ConnectionId(1)),
        Some(entity)
    );
}

#[tokio::test]
async fn restored_clock_and_owned_command_follow_native_committed_event_fifo() {
    use mc_script::{ScriptEvent, ScriptEventKind};
    let mut state = fixture();
    join_native_player(&mut state);
    let capacity = std::num::NonZeroUsize::new(4).unwrap();
    let (boundary, mut endpoint) = mc_script::script_boundary_pair(capacity, capacity);
    let manifest = mc_script::ScriptPluginManifest::new(
        "owner",
        "Owner",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("owned")
    .subscribe_event("player.joined")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    state.plugin_runtime = Some(crate::plugin::PluginRuntime::with_test_boundary(
        boundary.clone(),
    ));
    state.tick = 70;
    crate::plugin::start_events(&mut state).unwrap();
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerTick { tick: 70 }
    ));
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerStarted
    ));
    crate::plugin::observe_players(&mut state);
    assert!(crate::plugin::route_command(
        &mut state,
        crate::boundary::ConnectionId(1),
        "owned keep order",
        0
    ));
    assert_eq!(
        state.plugin_runtime.as_ref().unwrap().pending_events.len(),
        1
    );
    crate::plugin::seal_events(&mut state, 71).unwrap();
    for _ in 0..4 {
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
    }
    crate::plugin::acknowledge_events(&mut state, 71);
    for _ in 0..4 {
        assert!(matches!(
            endpoint.recv_event().await.unwrap().kind(),
            ScriptEventKind::ServerStarted
        ));
    }
    crate::plugin::acknowledge_events(&mut state, 70);
    crate::plugin::acknowledge_events(&mut state, 71);
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerTick { tick: 71 }
    ));
    // Required commands preserve their FIFO; observations use a separate fair
    // queue and need not precede or follow a command from the same native tick.
    let delivered = [
        endpoint.recv_event().await.unwrap(),
        endpoint.recv_event().await.unwrap(),
    ];
    assert!(
        delivered
            .iter()
            .any(|event| matches!(event.kind(), ScriptEventKind::PlayerJoined { .. }))
    );
    let command = delivered
        .iter()
        .find(|event| matches!(event.kind(), ScriptEventKind::PlayerCommand { .. }))
        .unwrap();
    assert_eq!(command.target_plugin_id(), Some("owner"));
    assert!(
        matches!(command.kind(), ScriptEventKind::PlayerCommand { root, arguments, .. }
        if root == "owned" && arguments == "keep order")
    );
}

#[test]
fn console_msg_uses_native_path_despite_player_plugin_reservation() {
    let mut state = fixture();
    join_native_player(&mut state);
    let capacity = std::num::NonZeroUsize::new(4).unwrap();
    let (boundary, endpoint) = mc_script::script_boundary_pair(capacity, capacity);
    let manifest = mc_script::ScriptPluginManifest::new(
        "owner",
        "Owner",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("msg")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    state.plugin_runtime = Some(crate::plugin::PluginRuntime::with_test_boundary(boundary));
    let (command, mut completed) =
        crate::command_console::ConsoleCommand::new("msg NativePlayer console-message".into());
    let (_, deliveries, _) = crate::tick::run_tick(
        &mut state,
        &[crate::boundary::PlayerInput::ConsoleCommand(command)],
    );
    assert!(completed.try_recv().is_ok());
    assert!(
        deliveries.iter().any(|(connection, changes)| {
            *connection == crate::boundary::ConnectionId(1)
                && changes.iter().any(|change| {
                    matches!(
                        change,
                        crate::boundary::PlayerChange::StyledChat {
                            content: mc_nbt::Tag::Compound(fields),
                        } if fields.iter().any(|(name, value)| {
                            name == "translate"
                                && matches!(value, mc_nbt::Tag::String(key)
                                    if key == "commands.message.display.incoming")
                        }) && fields.iter().any(|(name, value)| {
                            name == "with"
                                && matches!(value, mc_nbt::Tag::List(arguments)
                                    if arguments.elements.iter().any(|argument| {
                                        matches!(argument, mc_nbt::Tag::Compound(text)
                                            if text.iter().any(|(name, value)| {
                                                name == "text"
                                                    && matches!(value, mc_nbt::Tag::String(message)
                                                        if message == "console-message")
                                            }))
                                    }))
                        })
                    )
                })
        }),
        "{deliveries:?}"
    );
    assert!(
        state
            .plugin_runtime
            .as_ref()
            .unwrap()
            .pending_events
            .iter()
            .all(|event| !matches!(
                event.kind(),
                mc_script::ScriptEventKind::PlayerCommand { .. }
            ))
    );
}

#[test]
fn retired_manifest_root_never_falls_back_to_the_native_command() {
    use crate::boundary::{ConnectionId, PlayerChange, PlayerInput};
    let mut state = fixture();
    join_native_player(&mut state);
    let capacity = std::num::NonZeroUsize::new(4).unwrap();
    let (boundary, endpoint) = mc_script::script_boundary_pair(capacity, capacity);
    let manifest = mc_script::ScriptPluginManifest::new(
        "owner",
        "Owner",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("msg")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    let runtime = crate::plugin::PluginRuntime::with_test_boundary(boundary.clone());
    let catalog = runtime.commands();
    state.plugin_runtime = Some(runtime);
    assert!(crate::plugin::route_command(
        &mut state,
        ConnectionId(1),
        "msg NativePlayer plugin-owned",
        0
    ));
    assert!(
        matches!(state.plugin_runtime.as_ref().unwrap().pending_events[0].kind(),
        mc_script::ScriptEventKind::PlayerCommand { root, .. } if root == "msg")
    );
    state
        .plugin_runtime
        .as_mut()
        .unwrap()
        .pending_events
        .clear();
    boundary.close_event_admission();
    assert!(boundary.player_command_roots().is_empty());
    assert_eq!(&*catalog, &[("msg".into(), 0)]);
    assert_eq!(state.plugin_runtime.as_ref().unwrap().commands(), catalog);
    let (_, deliveries, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            permission_level: 0,
            command: "msg NativePlayer native-fallback-must-not-run".into(),
        }],
    );
    let queue = deliveries
        .iter()
        .find_map(|(connection, queue)| (*connection == ConnectionId(1)).then_some(queue))
        .unwrap();
    let replies: Vec<_> = queue
        .iter()
        .filter_map(|change| match change {
            PlayerChange::Chat { message, .. } => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(replies, ["Plugin command admission is unavailable."]);
    assert!(
        !state
            .plugin_runtime
            .as_ref()
            .unwrap()
            .pending_events
            .iter()
            .any(|event| matches!(
                event.kind(),
                mc_script::ScriptEventKind::PlayerCommand { .. }
            ))
    );
}

fn reserve_native_item(
    state: &mut TickState,
    endpoint: mc_script::ScriptInventoryEndpoint,
    resource: &str,
    count: u64,
) {
    use mc_script::{
        ScriptOperationPayload, ScriptOwnedInventoryOperation, ScriptOwnedInventoryResult,
    };
    let query = ScriptOperationRequest::try_new(
        "floor-query",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Query {
                endpoint: endpoint.clone(),
                expected_revision: None,
            },
        },
    )
    .unwrap();
    let queried = operation(state, "floor-owner", &query);
    let ScriptOperationPayload::OwnedInventory { result } = queried.payload() else {
        panic!("native inventory query: {queried:?}");
    };
    let ScriptOwnedInventoryResult::Snapshot { inventory } = &**result else {
        panic!("native inventory snapshot");
    };
    let request = ScriptOperationRequest::try_new(
        "floor-reserve",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Reserve {
                operation_id: "native-floor".into(),
                endpoint,
                resource_plan: mc_script::ScriptInventoryResourcePlan::new(vec![
                    mc_script::ScriptInventoryWorkPortion::new(
                        1,
                        vec![mc_script::ScriptInventoryMaterial::new(
                            resource.into(),
                            count,
                        )],
                    ),
                ]),
                expected_revision: inventory.fence.clone(),
            },
        },
    )
    .unwrap();
    let reserved = operation(state, "floor-owner", &request);
    assert_eq!(
        reserved.state(),
        ScriptOperationState::Committed,
        "{reserved:?}"
    );
}

#[test]
fn actual_native_barter_spends_only_excess_and_refusal_does_not_begin_admiration() {
    let mut state = fixture();
    let actor = join_native_player(&mut state);
    let gold = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:gold_ingot").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::new(gold, 2);
    reserve_native_item(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "minecraft:gold_ingot",
        1,
    );
    let piglin = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:piglin").unwrap())
        .unwrap();
    let first = state.spawn_mob(piglin, [4.0, 64.0, 0.0], 16.0);
    assert!(crate::piglin::interact(
        &mut state,
        actor,
        first,
        crate::boundary::Hand::Main
    ));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(crate::plugin_inventories::has_reservations_for_player(
        &state, actor
    ));
    let second = state.spawn_mob(piglin, [5.0, 64.0, 0.0], 16.0);
    let slot = state.entities.slot_of(second).unwrap();
    assert!(!crate::piglin::interact(
        &mut state,
        actor,
        second,
        crate::boundary::Hand::Main
    ));
    assert!(state.entities.row(slot).special_mob.is_none());
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(crate::plugin_inventories::has_reservations_for_player(
        &state, actor
    ));
}

#[test]
fn actual_native_break_refuses_reserved_tool_destruction_but_allows_nonbreaking_wear() {
    let mut state = fixture();
    let actor = join_native_player(&mut state);
    let desired = state.desired_chunks();
    state.world.ensure_blocking(&desired);
    let pickaxe = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap())
        .unwrap();
    let mut tool = mc_data::ItemStack::new(pickaxe, 1);
    let max_damage = state
        .registry
        .item_facts
        .facts_for_stack(&tool, &state.registry.items)
        .unwrap()
        .max_damage
        .unwrap();
    tool.damage = Some(max_damage as i32 - 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = tool;
    reserve_native_item(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "minecraft:iron_pickaxe",
        1,
    );
    let stone = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    let position = crate::boundary::BlockPos { x: 4, y: 64, z: 0 };
    state
        .world
        .set_block(position.x, position.y, position.z, stone);
    let intent = crate::intent::Intent {
        actor,
        seq: 0,
        kind: crate::intent::IntentKind::BreakBlock { pos: position },
        footprint: crate::intent::Footprint {
            block_expect: vec![(position, stone)],
            block_write: vec![(position, state.registry.states.air)],
            ..Default::default()
        },
    };
    let before = state.players[&actor].clone();
    assert_eq!(state.merge(&mut vec![intent.clone()]), 0);
    assert_eq!(
        state.world.block_at(position.x, position.y, position.z),
        Some(stone)
    );
    assert_eq!(state.players[&actor], before);
    assert!(state.merge_broken.is_empty());
    state.players.get_mut(&actor).unwrap().inventory.slots[0].damage = None;
    assert_eq!(state.merge(&mut vec![intent]), 1);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
    assert!(crate::plugin_inventories::has_reservations_for_player(
        &state, actor
    ));
    crate::tick::run_tick(&mut state, &[]);
    assert_eq!(
        state.world.block_at(position.x, position.y, position.z),
        Some(state.registry.states.air)
    );
}

#[test]
fn actual_drop_input_retains_floor_without_blanket_disabling_excess_stock() {
    let mut state = fixture();
    let actor = join_native_player(&mut state);
    let gold = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:gold_ingot").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::new(gold, 2);
    reserve_native_item(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "minecraft:gold_ingot",
        1,
    );
    let input = crate::boundary::PlayerInput::DropSelected {
        connection: crate::boundary::ConnectionId(1),
        whole_stack: false,
    };
    let (accepted, _, _) = crate::tick::run_tick(&mut state, std::slice::from_ref(&input));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(accepted.changes.entities_spawned.iter().any(|id| {
        state.entities.slot_of(*id).is_some_and(|slot| {
            state
                .entities
                .row(slot)
                .item
                .as_ref()
                .is_some_and(|item| item.item_id == gold && item.count == 1)
        })
    }));
    let (refused, _, _) = crate::tick::run_tick(&mut state, &[input]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(!refused.changes.entities_spawned.iter().any(|id| {
        state.entities.slot_of(*id).is_some_and(|slot| {
            state
                .entities
                .row(slot)
                .item
                .as_ref()
                .is_some_and(|item| item.item_id == gold)
        })
    }));
    assert!(crate::plugin_inventories::has_reservations_for_player(
        &state, actor
    ));
}

#[test]
fn actual_furnace_ticks_use_excess_input_then_pause_before_crossing_held_quantity() {
    let mut state = fixture();
    join_native_player(&mut state);
    let desired = state.desired_chunks();
    state.world.ensure_blocking(&desired);
    let position = crate::boundary::BlockPos { x: 4, y: 64, z: 0 };
    let furnace = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .unwrap()
        .default
        .0;
    state
        .world
        .set_block(position.x, position.y, position.z, furnace);
    let log = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:acacia_log").unwrap())
        .unwrap();
    let coal = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:coal").unwrap())
        .unwrap();
    let charcoal = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:charcoal").unwrap())
        .unwrap();
    let cell = (position.x, position.y, position.z);
    state.block_entities.insert(
        cell,
        crate::tick::BlockEntityState {
            slots: vec![
                mc_data::ItemStack::new(log, 2),
                mc_data::ItemStack::new(coal, 1),
                mc_data::ItemStack::EMPTY,
            ],
            furnace: Some(Default::default()),
        },
    );
    let endpoint = mc_script::ScriptInventoryEndpoint::Container {
        dimension: state.dimension.name().into(),
        position: mc_script::ScriptBlockPosition::new(position.x, position.y, position.z),
    };
    reserve_native_item(&mut state, endpoint, "minecraft:acacia_log", 1);
    for _ in 0..450 {
        crate::tick::run_tick(&mut state, &[]);
    }
    let record = &state.block_entities[&cell];
    assert_eq!(record.slots[0].count, 1);
    assert_eq!(record.slots[2].item_id, charcoal);
    assert_eq!(record.slots[2].count, 1);
    let properties = record.furnace.as_ref().unwrap().properties();
    crate::tick::run_tick(&mut state, &[]);
    assert_eq!(
        state.block_entities[&cell]
            .furnace
            .as_ref()
            .unwrap()
            .properties(),
        properties
    );
    assert_eq!(state.block_entities[&cell].slots[0].count, 1);
    assert_eq!(state.block_entities[&cell].slots[2].count, 1);
    assert!(crate::plugin_inventories::has_reservations_for_block(
        &state, position
    ));
}

struct WorldBlockFixture {
    state: TickState,
    boundary: mc_script::ScriptBoundary,
    host: mc_script::ScriptHostEndpoint,
    admission: mc_script::HostCommandAdmission,
    request: mc_script::ScriptWorldBlockSetRequest,
}

impl WorldBlockFixture {
    fn new(hooked: bool) -> Self {
        use mc_script::precommit::{HookFailurePolicy, HookKind, HookRegistration};
        let mut state = fixture();
        state
            .world
            .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(
                crate::boundary::ChunkPos { x: 0, z: 0 },
                0,
            )]));
        let (boundary, host) = mc_script::script_boundary_pair(
            std::num::NonZeroUsize::new(128).unwrap(),
            std::num::NonZeroUsize::new(128).unwrap(),
        );
        let manifest = mc_script::ScriptPluginManifest::new(
            "builder",
            "Generic block builder",
            "1",
            mc_script::COMPONENT_PLUGIN_API_VERSION,
        )
        .declare_world_blocks()
        .validate()
        .unwrap();
        host.register_plugin_routes(&manifest).unwrap();
        if hooked {
            boundary
                .set_precommit_hooks(vec![HookRegistration::new(
                    "protector",
                    HookKind::Build,
                    0,
                    HookFailurePolicy::Deny,
                )])
                .unwrap();
        }
        state.plugin_runtime = Some(crate::plugin::PluginRuntime::with_test_boundary(
            boundary.clone(),
        ));
        Self {
            state,
            boundary,
            host,
            admission: mc_script::HostCommandAdmission::from_manifest(&manifest),
            request: mc_script::ScriptWorldBlockSetRequest::try_new(
                "set-block",
                "minecraft:overworld",
                "minecraft:stone",
                1,
                64,
                1,
            )
            .unwrap(),
        }
    }

    fn submit(&self) {
        let mut batch = mc_script::CommandBatch::new(std::num::NonZeroUsize::new(1).unwrap());
        batch
            .try_push_authorized(
                mc_script::ScriptCommand::SetWorldBlock {
                    request: self.request.clone(),
                },
                self.admission.capabilities(),
            )
            .unwrap();
        self.host
            .try_submit_plugin_batch(&self.admission, batch)
            .unwrap();
    }

    fn question(&mut self) -> mc_script::precommit::Request {
        match self.host.recv_input_blocking() {
            Some(mc_script::ScriptHostInput::Precommit(request)) => request,
            _ => panic!("SetWorldBlock must ask the canonical build hook"),
        }
    }

    fn results(&self) -> Vec<(bool, Option<mc_script::ScriptWorldBlockSetFailure>)> {
        self.state
            .plugin_runtime
            .as_ref()
            .unwrap()
            .pending_events
            .iter()
            .filter_map(|event| {
                assert_eq!(event.target_plugin_id(), Some("builder"));
                match event.kind() {
                    mc_script::ScriptEventKind::WorldBlockSetResult {
                        applied, failure, ..
                    } => Some((*applied, *failure)),
                    _ => None,
                }
            })
            .collect()
    }
}

#[test]
fn plugin_set_world_block_asks_truthful_hook_and_only_applies_after_keep() {
    use mc_script::precommit::{HookActor, HookContext, HookDecision};
    let mut fixture = WorldBlockFixture::new(true);
    let previous = fixture.state.world.block_at(1, 64, 1).unwrap();
    let plugin_before = serde_json::to_value(&fixture.state.plugins).unwrap();
    fixture.submit();
    crate::plugin::pump(&mut fixture.state);
    assert!(fixture.results().is_empty());
    assert_eq!(fixture.state.world.block_at(1, 64, 1), Some(previous));
    assert!(fixture.state.plugin_block_writes.is_empty());
    let question = fixture.question();
    let HookContext::Build(context) = question.context() else {
        unreachable!()
    };
    assert_eq!(context.actor(), &HookActor::Plugin("builder".into()));
    assert_eq!(context.dimension(), "minecraft:overworld");
    assert_eq!(context.edits().len(), 1);
    let edit = context.edits()[0];
    assert_eq!((edit.x, edit.y, edit.z), (1, 64, 1));
    assert_eq!(edit.previous_state, previous);
    assert_eq!(edit.proposed_state, fixture.state.registry.states.stone);
    question.answer(HookDecision::Keep, Vec::new()).unwrap();
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(fixture.results(), vec![(true, None)]);
    assert_eq!(
        fixture.state.world.block_at(1, 64, 1),
        Some(edit.proposed_state)
    );
    assert_eq!(fixture.state.plugin_block_writes.len(), 1);
    assert_eq!(
        serde_json::to_value(&fixture.state.plugins).unwrap(),
        plugin_before
    );
}

#[test]
fn plugin_set_world_block_fails_closed_for_denial_error_and_invalidated_fences() {
    use mc_script::precommit::{HookDecision, HookFailure, HookStorageVersionGuards};
    for changed in ["cancel", "error", "expired", "block", "guard", "source"] {
        let mut fixture = WorldBlockFixture::new(true);
        fixture.state.plugins.apply_storage(
            "protector",
            &[ScriptStorageMutation::compare_and_swap("claim", None, "allowed").unwrap()],
        );
        let version = fixture.state.plugins.version("protector", "claim").unwrap();
        fixture.submit();
        crate::plugin::pump(&mut fixture.state);
        let question = fixture.question();
        match changed {
            "cancel" => question.answer(HookDecision::Cancel, Vec::new()).unwrap(),
            "error" => question.fail(HookFailure::Unavailable).unwrap(),
            "expired" => question.fail(HookFailure::Expired).unwrap(),
            "block" => {
                fixture
                    .state
                    .world
                    .set_block(1, 64, 1, fixture.state.registry.states.stone);
                question.answer(HookDecision::Keep, Vec::new()).unwrap();
            }
            "source" => {
                fixture.host.unregister_plugin_routes("builder");
                question.answer(HookDecision::Keep, Vec::new()).unwrap();
            }
            "guard" => {
                question
                    .answer(
                        HookDecision::Keep,
                        vec![HookStorageVersionGuards::new(
                            "protector".into(),
                            vec![ScriptPluginStorageVersionGuard::new(
                                "claim".into(),
                                version,
                            )],
                        )],
                    )
                    .unwrap();
                fixture.state.plugins.apply_storage(
                    "protector",
                    &[
                        ScriptStorageMutation::compare_and_swap("claim", Some(version), "denied")
                            .unwrap(),
                    ],
                );
            }
            _ => unreachable!(),
        }
        let before = serde_json::to_value(&fixture.state.plugins).unwrap();
        let block_before = fixture.state.world.block_at(1, 64, 1);
        crate::plugin::pump(&mut fixture.state);
        assert_eq!(fixture.results().len(), 1, "{changed}");
        assert!(!fixture.results()[0].0, "{changed}");
        assert!(fixture.results()[0].1.is_some(), "{changed}");
        assert_eq!(
            fixture.state.world.block_at(1, 64, 1),
            block_before,
            "{changed}"
        );
        assert_eq!(
            serde_json::to_value(&fixture.state.plugins).unwrap(),
            before,
            "{changed}"
        );
        assert!(fixture.state.plugin_block_writes.is_empty(), "{changed}");
    }
}

#[test]
fn pending_plugin_block_sets_share_the_bounded_admission_limit_and_overflow_refuses() {
    use mc_script::precommit::{HookDecision, MAX_PRECOMMIT_REQUESTS};
    let mut fixture = WorldBlockFixture::new(true);
    for _ in 0..=MAX_PRECOMMIT_REQUESTS {
        fixture.submit();
    }
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(
        fixture.results(),
        vec![(false, Some(mc_script::ScriptWorldBlockSetFailure::Busy))]
    );
    for _ in 0..MAX_PRECOMMIT_REQUESTS {
        fixture
            .question()
            .answer(HookDecision::Cancel, Vec::new())
            .unwrap();
    }
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(fixture.results().len(), MAX_PRECOMMIT_REQUESTS + 1);
    assert!(fixture.results().iter().all(|(applied, _)| !applied));
    assert!(fixture.state.plugin_block_writes.is_empty());
}

#[test]
fn hook_free_plugin_block_sets_keep_the_immediate_native_path() {
    let mut fixture = WorldBlockFixture::new(false);
    fixture.submit();
    crate::plugin::pump(&mut fixture.state);
    assert_eq!(fixture.results(), vec![(true, None)]);
    assert_eq!(fixture.boundary.event_queue_depth(), 0);
    assert_eq!(fixture.state.plugin_block_writes.len(), 1);
}

fn receipt_status(
    state: &mut TickState,
    owner: &str,
    id: &str,
) -> mc_script::ScriptOperationOutcome {
    let status = ScriptOperationRequest::try_new(
        "status",
        ScriptOperation::Status {
            operation_id: id.into(),
        },
    )
    .unwrap();
    operation(state, owner, &status)
}

#[test]
fn old_operation_ids_remain_deduplicated_after_checkpoint_and_wal_replay() {
    let mut state = fixture();
    state.tick = 100;
    let request = batch(
        "write",
        "durable",
        vec![ScriptStorageMutation::compare_and_swap("value", None, "saved").unwrap()],
    );
    let original = operation(&mut state, "owner", &request);
    let mut entries = Vec::new();
    crate::plugin::journal(&mut state, &mut entries);
    let base: PluginState =
        serde_json::from_slice(&serde_json::to_vec(&state.plugins).unwrap()).unwrap();
    state.tick += 1_000_000;
    assert_eq!(receipt_status(&mut state, "owner", "durable"), original);
    assert_eq!(operation(&mut state, "owner", &request), original);
    entries.clear();
    crate::plugin::journal(&mut state, &mut entries);
    assert!(
        entries.is_empty(),
        "late retries must not create another effect"
    );

    let later = batch(
        "later",
        "later",
        vec![ScriptStorageMutation::compare_and_swap("later", None, "tail").unwrap()],
    );
    operation(&mut state, "owner", &later);
    crate::plugin::journal(&mut state, &mut entries);
    let batches = [(state.tick, state.tick, entries)];
    let replay = crate::journal::Replay {
        plugins: base,
        ..Default::default()
    };
    let replay = crate::journal::replay_batches(replay, &batches);
    let replay = crate::journal::replay_batches(replay, &batches);
    state.plugins = serde_json::from_slice(&serde_json::to_vec(&replay.plugins).unwrap()).unwrap();
    let revision = state.plugins.revision();
    assert_eq!(receipt_status(&mut state, "owner", "durable"), original);
    assert_eq!(operation(&mut state, "owner", &request), original);
    let conflict = batch(
        "changed",
        "durable",
        vec![ScriptStorageMutation::compare_and_swap("duplicate", None, "forbidden").unwrap()],
    );
    assert_eq!(
        operation(&mut state, "owner", &conflict).failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
    assert!(state.plugins.record("owner", "duplicate").is_none());
    assert_eq!(state.plugins.revision(), revision);
    let mut retry_entries = Vec::new();
    crate::plugin::journal(&mut state, &mut retry_entries);
    assert!(retry_entries.is_empty());
}

#[test]
fn operation_ids_survive_later_receipt_count_and_idempotent_delta_replay() {
    let mut state = fixture();
    let first = batch(
        "first",
        "oldest",
        vec![ScriptStorageMutation::compare_and_swap("value", None, "saved").unwrap()],
    );
    let original = operation(&mut state, "first-owner", &first);
    let mut entries = Vec::new();
    crate::plugin::journal(&mut state, &mut entries);
    let base = state.plugins.clone();
    for index in 0..4096 {
        let request = batch(
            "next",
            &format!("id-{index}"),
            vec![ScriptStorageMutation::compare_and_swap("value", None, "saved").unwrap()],
        );
        operation(&mut state, "second-owner", &request);
    }
    assert_eq!(
        receipt_status(&mut state, "first-owner", "oldest"),
        original
    );
    assert_eq!(operation(&mut state, "first-owner", &first), original);
    let image = serde_json::to_value(&state.plugins).unwrap();
    assert_eq!(
        image["receipts"]
            .as_object()
            .unwrap()
            .values()
            .map(|owner| owner.as_object().unwrap().len())
            .sum::<usize>(),
        4097
    );
    entries.clear();
    crate::plugin::journal(&mut state, &mut entries);
    let [crate::journal::JournalEntry::Plugins { value }] = entries.as_slice() else {
        panic!("delta")
    };
    assert!(
        serde_json::to_value(value).unwrap()["receipts"]
            .get("first-owner")
            .is_none()
    );
    let mut restored = base;
    restored.apply_delta(value);
    restored.apply_delta(value);
    restored.validate().unwrap();
    assert_eq!(
        restored.operation_receipt("first-owner", "oldest"),
        Some(&original)
    );
    assert!(restored.operation_receipt("second-owner", "id-0").is_some());
    state.plugins = serde_json::from_slice(&serde_json::to_vec(&restored).unwrap()).unwrap();
    assert_eq!(operation(&mut state, "first-owner", &first), original);
    assert_eq!(
        receipt_status(&mut state, "first-owner", "oldest"),
        original
    );
}

#[tokio::test]
async fn observation_shedding_is_per_plugin_fair_and_never_halts_authority() {
    use mc_script::{ScriptEvent, ScriptEventKind, ScriptPlayerId};
    let mut state = fixture();
    let capacity = std::num::NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = mc_script::script_boundary_pair(capacity, capacity);
    for (id, subscription) in [("a-slow", "player.left"), ("b-fast", "player.chat")] {
        let manifest = mc_script::ScriptPluginManifest::new(
            id,
            id,
            "0.1.0",
            mc_script::COMPONENT_PLUGIN_API_VERSION,
        )
        .subscribe_event(subscription)
        .validate()
        .unwrap();
        endpoint.register_plugin_routes(&manifest).unwrap();
    }
    state.plugin_runtime = Some(crate::plugin::PluginRuntime::with_test_boundary(
        boundary.clone(),
    ));
    for _ in 0..300 {
        crate::plugin::enqueue_result(
            &mut state,
            Some(ScriptEvent::player_left(ScriptPlayerId::new(1), "left")),
        );
    }
    let context = mc_script::ScriptPlayerContext::try_new(
        uuid::Uuid::from_u128(1).to_string(),
        "Player",
        false,
        "minecraft:overworld",
        0.0,
        64.0,
        0.0,
    )
    .unwrap();
    crate::plugin::enqueue_result(
        &mut state,
        Some(ScriptEvent::player_chat_with_context(
            ScriptPlayerId::new(1),
            "fast",
            context,
        )),
    );
    assert_eq!(boundary.observation_events_dropped("a-slow"), 44);
    assert_eq!(boundary.observation_events_dropped("b-fast"), 0);
    assert!(
        state
            .plugin_runtime
            .as_ref()
            .unwrap()
            .pending_events
            .is_empty()
    );
    crate::plugin::seal_events(&mut state, 1).unwrap();
    // Even sealed observations cannot appear before the WAL-ACK clock.
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerStarted
    ));
    crate::plugin::acknowledge_events(&mut state, 1);
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerTick { tick: 1 }
    ));
    assert_eq!(
        endpoint.recv_event().await.unwrap().target_plugin_id(),
        Some("a-slow")
    );
    assert_eq!(
        endpoint.recv_event().await.unwrap().target_plugin_id(),
        Some("b-fast")
    );
    crate::plugin::seal_events(&mut state, 2).unwrap();
}

#[test]
fn required_auth_retirement_refuses_logins_without_stopping_established_players() {
    use crate::boundary::{ConnectionId, PlayerChange, PlayerInput};
    for required in [false, true] {
        let mut state = fixture();
        let established = join_native_player(&mut state);
        state.require_plugin_login = required;
        let join = |id| PlayerInput::Join {
            connection: ConnectionId(id),
            uuid: uuid::Uuid::from_u128(u128::from(id)),
            name: format!("Pending{id}"),
            permission_level: 0,
        };
        crate::tick::run_tick(&mut state, &[join(2)]);
        let capacity = std::num::NonZeroUsize::new(16).unwrap();
        let (boundary, endpoint) = mc_script::script_boundary_pair(capacity, capacity);
        let manifest = mc_script::ScriptPluginManifest::new(
            "authentication",
            "Authentication",
            "0.1.0",
            mc_script::COMPONENT_PLUGIN_API_VERSION,
        )
        .validate()
        .unwrap();
        endpoint.register_plugin_routes(&manifest).unwrap();
        let mut runtime = crate::plugin::PluginRuntime::with_test_boundary(boundary.clone());
        if required {
            runtime.required_auth_plugins.push("authentication".into());
        }
        state.plugin_runtime = Some(runtime);
        crate::plugin::observe_players(&mut state);
        assert!(state.disconnects.is_empty());
        endpoint.unregister_plugin_routes("authentication");
        crate::plugin::observe_players(&mut state);
        let refused = |state: &TickState, id| {
            state.outbox.get(&ConnectionId(id)).is_some_and(|changes| {
                changes.iter().any(|change| {
                    matches!(change,
                    PlayerChange::Disconnect { reason }
                    if reason.contains("authentication") && reason.contains("restart required"))
                })
            })
        };
        assert_eq!(refused(&state, 2), required, "pending login refusal");
        assert!(!refused(&state, 1), "established player remains connected");
        assert_eq!(
            state.connections.entity_of(ConnectionId(1)),
            Some(established)
        );
        assert_eq!(state.session_locked(ConnectionId(2)), required);
        assert_eq!(crate::plugin::seal_events(&mut state, 2), Ok(true));
        // New arrivals after retirement receive the same refusal, while ticks advance.
        let tick = state.tick;
        crate::tick::run_tick(&mut state, &[join(3)]);
        assert!(state.tick > tick);
        assert_eq!(refused(&state, 3), required);
        assert_eq!(
            state.connections.entity_of(ConnectionId(1)),
            Some(established)
        );
        assert!(!state.session_locked(ConnectionId(1)));
        // A fresh provider after operator recovery accepts new authentication work.
        endpoint.register_plugin_routes(&manifest).unwrap();
        crate::tick::run_tick(&mut state, &[join(4)]);
        assert!(!refused(&state, 4));
    }
}

#[test]
fn plugin_login_rejects_a_deployment_without_an_auth_provider() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = crate::config::CoreConfig::new(directory.path().join("world"), 42);
    config.plugin_directory = Some(directory.path().join("plugins"));
    std::fs::create_dir_all(config.plugin_directory.as_ref().unwrap()).unwrap();
    config.require_plugin_login = true;
    let error = match crate::plugin::PluginRuntime::open(&config) {
        Ok(_) => panic!("an auth-less deployment must not accept required logins"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("auth-capable package"),
        "{error}"
    );
}

#[test]
fn a_missing_plugin_directory_is_created_empty_instead_of_refusing_startup() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = crate::config::CoreConfig::new(directory.path().join("world"), 42);
    let plugins = directory.path().join("plugins");
    config.plugin_directory = Some(plugins.clone());
    assert!(!plugins.exists());
    let runtime = crate::plugin::PluginRuntime::open(&config)
        .expect("a first start without a plugins directory must succeed");
    assert!(runtime.is_some());
    assert!(plugins.is_dir(), "the empty deployment root is created");
}

#[tokio::test]
async fn durable_callbacks_survive_full_delivery_queue_and_remain_ack_gated() {
    use mc_script::{ScriptEvent, ScriptEventKind};
    let mut state = fixture();
    let capacity = std::num::NonZeroUsize::new(1).unwrap();
    let (boundary, mut endpoint) = mc_script::script_boundary_pair(capacity, capacity);
    let manifest = mc_script::ScriptPluginManifest::new(
        "owner",
        "Owner",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    state.plugin_runtime = Some(crate::plugin::PluginRuntime::with_test_boundary(
        boundary.clone(),
    ));
    let request = mc_script::ScriptPluginStorageGetRequest::try_new("read", "key").unwrap();
    let result = ScriptEvent::plugin_storage_get_result("owner", &request, None, None).unwrap();
    crate::plugin::enqueue_result(&mut state, Some(result.clone()));
    crate::plugin::seal_events(&mut state, 2).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    crate::plugin::acknowledge_events(&mut state, 1);
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerStarted
    ));
    crate::plugin::acknowledge_events(&mut state, 2);
    assert!(matches!(
        endpoint.recv_event().await.unwrap().kind(),
        ScriptEventKind::ServerTick { tick: 2 }
    ));
    // The result was retained while the clock occupied the one-entry mailbox.
    crate::plugin::acknowledge_events(&mut state, 2);
    assert_eq!(endpoint.recv_event().await.unwrap(), result);
    assert_eq!(boundary.observation_events_dropped("owner"), 0);
    for _ in 0..=mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY {
        crate::plugin::enqueue_result(&mut state, Some(result.clone()));
    }
    assert!(crate::plugin::seal_events(&mut state, 3).is_err());
}

#[test]
fn unavailable_durable_delivery_halts_but_observation_delivery_does_not() {
    let mut state = fixture();
    let capacity = std::num::NonZeroUsize::new(1).unwrap();
    let (boundary, endpoint) = mc_script::script_boundary_pair(capacity, capacity);
    let manifest = mc_script::ScriptPluginManifest::new(
        "owner",
        "Owner",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .subscribe_event("player.left")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    state.plugin_runtime = Some(crate::plugin::PluginRuntime::with_test_boundary(
        boundary.clone(),
    ));
    boundary.close_event_admission();
    for _ in 0..2048 {
        crate::plugin::enqueue_result(
            &mut state,
            Some(mc_script::ScriptEvent::player_left(
                mc_script::ScriptPlayerId::new(1),
                "left",
            )),
        );
    }
    // Closed admission retains registrations for already accepted required
    // inputs, but new observations are shed and counted for those registrations.
    assert_eq!(boundary.observation_events_dropped("owner"), 2048);
    crate::plugin::seal_events(&mut state, 1).unwrap();
    crate::plugin::acknowledge_events(&mut state, 1);
    crate::plugin::seal_events(&mut state, 1).unwrap();
    let request = mc_script::ScriptPluginStorageGetRequest::try_new("read", "key").unwrap();
    crate::plugin::enqueue_result(
        &mut state,
        Some(
            mc_script::ScriptEvent::plugin_storage_get_result("owner", &request, None, None)
                .unwrap(),
        ),
    );
    crate::plugin::seal_events(&mut state, 2).unwrap();
    crate::plugin::acknowledge_events(&mut state, 2);
    assert!(crate::plugin::seal_events(&mut state, 3).is_err());
}

#[test]
fn join_observations_do_not_poison_full_required_callback_capacity() {
    let mut state = fixture();
    join_native_player(&mut state);
    let capacity = std::num::NonZeroUsize::new(1).unwrap();
    let (boundary, endpoint) = mc_script::script_boundary_pair(capacity, capacity);
    let manifest = mc_script::ScriptPluginManifest::new(
        "owner",
        "Owner",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .subscribe_event("player.joined")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    state.plugin_runtime = Some(crate::plugin::PluginRuntime::with_test_boundary(
        boundary.clone(),
    ));
    let request = mc_script::ScriptPluginStorageGetRequest::try_new("read", "key").unwrap();
    let result =
        mc_script::ScriptEvent::plugin_storage_get_result("owner", &request, None, None).unwrap();
    for _ in 0..mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY {
        crate::plugin::enqueue_result(&mut state, Some(result.clone()));
    }
    crate::plugin::observe_players(&mut state);
    assert_eq!(
        state.plugin_runtime.as_ref().unwrap().pending_events.len(),
        mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY
    );
    assert_eq!(boundary.observation_events_dropped("owner"), 0);
    crate::plugin::seal_events(&mut state, 1).unwrap();
}

#[test]
fn plugin_teleport_dimension_rejects_mismatch_without_moving() {
    let mut state = fixture();
    let entity = join_native_player(&mut state);
    let slot = state.entities.slot_of(entity).unwrap();
    let original = state.entities.pos(slot);
    let target = mc_script::ScriptPosition::try_new(20.0, 80.0, 20.0).unwrap();
    let wrong = mc_script::ScriptPlayerTeleportRequest::try_new(
        "wrong-dimension",
        mc_script::ScriptPlayerId::new(1),
        "minecraft:the_nether",
        target,
    )
    .unwrap();
    assert_eq!(
        crate::plugin::teleport_player(&mut state, &wrong),
        Some(mc_script::ScriptPlayerTeleportFailure::DimensionMismatch)
    );
    assert_eq!(state.entities.pos(slot), original);
    let correct = mc_script::ScriptPlayerTeleportRequest::try_new(
        "correct-dimension",
        mc_script::ScriptPlayerId::new(1),
        state.dimension.name(),
        target,
    )
    .unwrap();
    assert_eq!(crate::plugin::teleport_player(&mut state, &correct), None);
    assert_eq!(state.entities.pos(slot), [20.0, 80.0, 20.0]);
}
