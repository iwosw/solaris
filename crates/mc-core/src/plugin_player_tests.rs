use std::num::NonZeroUsize;
use std::sync::Arc;

use super::*;
use mc_script::{
    COMPONENT_PLUGIN_API_VERSION, CommandBatch, HostCommandAdmission, ScriptEventKind,
    ScriptInventoryMenuItem, ScriptInventoryMenuSlot, ScriptInventoryStorageTransaction,
    ScriptPluginManifest, ScriptStorageMutation,
};

fn fixture() -> (TickState, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let terrain = mc_worldgen::TerrainGenerator::new(42, registry.blocks.clone());
    let spawn = terrain.locate_safe_spawn().unwrap();
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "Shopper".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    (state, entity)
}

async fn admit(owner: &str, command: ScriptCommand) -> AdmittedScriptCommand {
    let manifest =
        ScriptPluginManifest::new(owner, "Player test", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .declare_inventory_storage_transactions()
            .declare_player_inventory()
            .declare_inventory_menus()
            .declare_zones()
            .declare_auth()
            .validate()
            .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, endpoint) = mc_script::script_boundary_pair(one, one);
    let mut batch = CommandBatch::new(one);
    batch
        .try_push_authorized(command, &manifest.to_command_capabilities())
        .unwrap();
    endpoint.try_submit_plugin_batch(&admission, batch).unwrap();
    boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap()
}

fn item(state: &TickState, resource: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(resource).unwrap())
            .unwrap(),
        count,
    )
}

fn purchase(expected_version: Option<u64>, currency: i16, goods: i16) -> ScriptCommand {
    ScriptCommand::InventoryStorageTransaction {
        transaction: ScriptInventoryStorageTransaction::try_new(
            "purchase",
            ScriptPlayerId::new(1),
            vec![
                ScriptInventoryResourceDelta::try_new("minecraft:emerald", currency).unwrap(),
                ScriptInventoryResourceDelta::try_new("minecraft:apple", goods).unwrap(),
            ],
            vec![ScriptStorageMutation::CompareAndSwap {
                key: "shop-ledger".into(),
                expected_version,
                value: "purchased".into(),
            }],
        )
        .unwrap(),
    }
}

#[tokio::test]
async fn native_inventory_and_storage_share_one_replayable_tick_batch() {
    let (mut state, entity) = fixture();
    state.players.get_mut(&entity).unwrap().inventory.slots[0] =
        item(&state, "minecraft:emerald", 8);
    let event = execute(&mut state, admit("shop", purchase(None, -3, 2)).await).unwrap();
    assert!(matches!(
        event.kind(),
        ScriptEventKind::InventoryStorageTransactionResult {
            committed: true,
            ..
        }
    ));
    assert_eq!(event.target_plugin_id(), Some("shop"));
    assert_eq!(state.players[&entity].inventory.slots[0].count, 5);
    assert_eq!(
        state
            .plugins
            .record("shop", "shop-ledger")
            .unwrap()
            .value
            .as_deref(),
        Some("purchased")
    );
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, crate::journal::JournalEntry::Plugins { .. }))
    );
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, crate::journal::JournalEntry::Player { .. }))
    );
    let batches = [(output.tick, output.tick, entries)];
    let replay = crate::journal::replay_batches(crate::journal::Replay::default(), &batches);
    let replay = crate::journal::replay_batches(replay, &batches);
    let uuid = state
        .connections
        .uuid_of(ConnectionId(1))
        .unwrap()
        .to_string();
    assert_eq!(
        replay.players[&uuid].inventory,
        state.players[&entity].inventory.slots
    );
    assert_eq!(replay.plugins, state.plugins);
    assert_eq!(replay.players[&uuid].inventory[0].count, 5);
}

#[tokio::test]
async fn stale_storage_fence_does_not_debit_native_inventory() {
    let (mut state, entity) = fixture();
    state.players.get_mut(&entity).unwrap().inventory.slots[0] =
        item(&state, "minecraft:emerald", 8);
    let mutations = [ScriptStorageMutation::CompareAndSwap {
        key: "shop-ledger".into(),
        expected_version: None,
        value: "already-exists".into(),
    }];
    state.plugins.apply_storage("shop", &mutations);
    let inventory = state.players[&entity].inventory.clone();
    let plugins = state.plugins.clone();
    let event = execute(&mut state, admit("shop", purchase(None, -3, 2)).await).unwrap();
    assert!(matches!(
        event.kind(),
        ScriptEventKind::InventoryStorageTransactionResult {
            committed: false,
            ..
        }
    ));
    assert_eq!(state.players[&entity].inventory, inventory);
    assert_eq!(state.plugins, plugins);
}

#[tokio::test]
async fn insufficient_stock_and_capacity_leave_both_authorities_unchanged() {
    for full in [false, true] {
        let (mut state, entity) = fixture();
        let emerald = item(&state, "minecraft:emerald", if full { 64 } else { 1 });
        let stone = item(&state, "minecraft:stone", 64);
        let player = state.players.get_mut(&entity).unwrap();
        player.inventory.slots[0] = emerald;
        if full {
            player.inventory.slots[1..36].fill(stone);
        }
        let inventory = player.inventory.clone();
        let plugins = state.plugins.clone();
        let event = execute(&mut state, admit("shop", purchase(None, -3, 2)).await).unwrap();
        assert!(matches!(
            event.kind(),
            ScriptEventKind::InventoryStorageTransactionResult {
                committed: false,
                ..
            }
        ));
        assert_eq!(state.players[&entity].inventory, inventory);
        assert_eq!(state.plugins, plugins);
    }
}

#[tokio::test]
async fn menu_click_is_targeted_and_display_items_never_enter_custody() {
    let (mut state, entity) = fixture();
    let menu = ScriptInventoryMenu::try_new(
        "shop-menu",
        "Shop",
        vec![ScriptInventoryMenuSlot::new(
            8,
            ScriptInventoryMenuItem::try_new("minecraft:apple", 1, Some("Buy apple".into()))
                .unwrap(),
        )],
    )
    .unwrap();
    execute(
        &mut state,
        admit(
            "shop",
            ScriptCommand::OpenInventoryMenu {
                player_id: ScriptPlayerId::new(1),
                menu,
            },
        )
        .await,
    );
    let native = state.players[&entity].open.as_ref().unwrap();
    let window = native.window;
    let state_id = native.state_id;
    let before = state.players[&entity].inventory.clone();
    let (consumed, event) = menu_input(
        &mut state,
        &PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window,
            state_id,
            slot: 8,
            button: 0,
            mode: ClickMode::Pickup,
        },
    );
    assert!(consumed);
    let event = event.unwrap();
    assert_eq!(event.target_plugin_id(), Some("shop"));
    assert!(
        matches!(event.kind(), ScriptEventKind::InventoryMenuClicked {
        menu_id, slot: 8, click: ScriptInventoryClick::Primary, .. } if menu_id == "shop-menu")
    );
    assert_eq!(state.players[&entity].inventory, before);
    assert!(
        state.players[&entity]
            .open
            .as_ref()
            .unwrap()
            .carried
            .is_empty()
    );
    let (_, stale) = menu_input(
        &mut state,
        &PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window,
            state_id: state_id + 1,
            slot: 8,
            button: 0,
            mode: ClickMode::Pickup,
        },
    );
    assert!(stale.is_none());
    execute(
        &mut state,
        admit(
            "foreign",
            ScriptCommand::CloseInventoryMenu {
                player_id: ScriptPlayerId::new(1),
                menu_id: "shop-menu".into(),
            },
        )
        .await,
    );
    assert!(state.players[&entity].open.is_some());
    let (consumed, _) = menu_input(
        &mut state,
        &PlayerInput::ContainerClose {
            connection: ConnectionId(1),
            window,
        },
    );
    assert!(consumed);
    assert!(state.players[&entity].open.is_none());
    assert_eq!(state.players[&entity].inventory, before);
}

#[tokio::test]
async fn protected_zones_restore_and_enforce_owner_operator_and_dimension() {
    let (mut state, actor) = fixture();
    let allowed = uuid::Uuid::new_v4().to_string();
    let zone = ScriptAxisAlignedZone::try_new_with_protection(
        "claim",
        "minecraft:overworld",
        ScriptPosition::try_new(0.0, 0.0, 0.0).unwrap(),
        ScriptPosition::try_new(9.0, 100.0, 9.0).unwrap(),
        Some(ScriptZoneProtection::try_actor_or_operator(&allowed).unwrap()),
    )
    .unwrap();
    let event = execute(
        &mut state,
        admit("towns", ScriptCommand::UpsertZone { zone }).await,
    )
    .unwrap();
    assert!(matches!(
        event.kind(),
        ScriptEventKind::ZoneCommandResult { accepted: true, .. }
    ));
    let at = BlockPos { x: 4, y: 64, z: 4 };
    assert!(!allows_block(&state, actor, at));
    state.plugin_players = PlayerAuthority::default();
    let encoded = serde_json::to_vec(&state.plugins).unwrap();
    state.plugins = serde_json::from_slice(&encoded).unwrap();
    restore(&mut state).unwrap();
    assert!(!allows_block(&state, actor, at));
    assert!(!allows_plugin_bounds(
        &state,
        "builder",
        "minecraft:overworld",
        [9, 64, 9],
        [12, 80, 12]
    ));
    assert!(allows_plugin_bounds(
        &state,
        "towns",
        "minecraft:overworld",
        [9, 64, 9],
        [12, 80, 12]
    ));
    assert!(allows_plugin_bounds(
        &state,
        "builder",
        "minecraft:overworld",
        [10, 64, 10],
        [12, 80, 12]
    ));
    assert!(allows_plugin_bounds(
        &state,
        "builder",
        "minecraft:the_nether",
        [4, 64, 4],
        [5, 80, 5]
    ));
    state.plugin_players.observe_permission(ConnectionId(1), 2);
    assert!(allows_block(&state, actor, at));
    state.plugin_players.observe_permission(ConnectionId(1), 0);
    assert!(allows_block(&state, actor, BlockPos { x: 10, ..at }));
    state.dimension = crate::dimensions::Dimension::Nether;
    assert!(allows_block(&state, actor, at));
    state.dimension = crate::dimensions::Dimension::Overworld;
    execute(
        &mut state,
        admit(
            "towns",
            ScriptCommand::RemoveZone {
                zone_id: "claim".into(),
            },
        )
        .await,
    );
    assert!(allows_block(&state, actor, at));
    assert!(state.plugins.get_domain("towns", ZONES).is_none());
    let zone = ScriptAxisAlignedZone::try_new(
        "event-only",
        "minecraft:overworld",
        ScriptPosition::try_new(0.0, 0.0, 0.0).unwrap(),
        ScriptPosition::try_new(9.0, 100.0, 9.0).unwrap(),
    )
    .unwrap();
    execute(
        &mut state,
        admit("towns", ScriptCommand::UpsertZone { zone }).await,
    );
    assert!(allows_plugin_bounds(
        &state,
        "builder",
        "minecraft:overworld",
        [4, 64, 4],
        [5, 80, 5]
    ));
}

#[tokio::test]
async fn player_inventory_answers_unknown_resource_without_partial_mutation() {
    let (mut state, entity) = fixture();
    let before = state.players[&entity].inventory.clone();
    let command = ScriptCommand::PlayerInventoryTransaction {
        transaction: mc_script::ScriptPlayerInventoryTransaction::try_new(
            "unknown",
            ScriptPlayerId::new(1),
            vec![
                ScriptInventoryResourceDelta::try_new("minecraft:apple", 2).unwrap(),
                ScriptInventoryResourceDelta::try_new("example:missing", 1).unwrap(),
            ],
        )
        .unwrap(),
    };
    let event = execute(&mut state, admit("shop", command).await).unwrap();
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerInventoryTransactionResult {
            failure: Some(ScriptPlayerInventoryFailure::UnknownResource),
            ..
        }
    ));
    assert_eq!(event.target_plugin_id(), Some("shop"));
    assert_eq!(state.players[&entity].inventory, before);
}

#[test]
fn loader_ready_requires_live_session_and_is_discarded_on_leave() {
    let (mut state, _) = fixture();
    note_loader_ready(&mut state, ConnectionId(999));
    assert!(
        !state
            .plugin_players
            .loader_ready
            .contains(&ConnectionId(999))
    );
    note_loader_ready(&mut state, ConnectionId(1));
    assert!(state.plugin_players.loader_ready.contains(&ConnectionId(1)));
    menu_input(
        &mut state,
        &PlayerInput::Leave {
            connection: ConnectionId(1),
        },
    );
    assert!(!state.plugin_players.loader_ready.contains(&ConnectionId(1)));
}

#[test]
fn zone_bounds_are_inclusive_and_dimension_exact() {
    let zone = ScriptAxisAlignedZone::try_new(
        "bounds",
        "minecraft:overworld",
        ScriptPosition::try_new(-1.0, 0.0, -1.0).unwrap(),
        ScriptPosition::try_new(1.0, 2.0, 1.0).unwrap(),
    )
    .unwrap();
    assert!(contains(&zone, "minecraft:overworld", [-1.0, 0.0, -1.0]));
    assert!(contains(&zone, "minecraft:overworld", [1.0, 2.0, 1.0]));
    assert!(!contains(&zone, "minecraft:overworld", [1.001, 2.0, 1.0]));
    assert!(!contains(&zone, "minecraft:the_nether", [0.0, 1.0, 0.0]));
}

#[tokio::test]
async fn player_effects_route_to_child_world_with_one_global_plugin_authority() {
    let (mut root, _) = fixture();
    crate::tick::run_tick(
        &mut root,
        &[PlayerInput::Leave {
            connection: ConnectionId(1),
        }],
    );
    let (mut child, actor) = fixture();
    child.dimension = crate::dimensions::Dimension::Nether;
    child.players.get_mut(&actor).unwrap().inventory.slots[0] =
        item(&child, "minecraft:emerald", 8);
    root.other_dimensions
        .insert(crate::dimensions::Dimension::Nether, Box::new(child));
    let event = execute(&mut root, admit("shop", purchase(None, -3, 2)).await).unwrap();
    assert!(matches!(
        event.kind(),
        ScriptEventKind::InventoryStorageTransactionResult {
            committed: true,
            ..
        }
    ));
    assert_eq!(
        root.plugins
            .record("shop", "shop-ledger")
            .unwrap()
            .value
            .as_deref(),
        Some("purchased")
    );
    let child = &root.other_dimensions[&crate::dimensions::Dimension::Nether];
    assert_eq!(child.players[&actor].inventory.slots[0].count, 5);
    assert!(child.plugins.record("shop", "shop-ledger").is_none());
    let menu = ScriptInventoryMenu::try_new("child-shop", "Shop", vec![]).unwrap();
    execute(
        &mut root,
        admit(
            "shop",
            ScriptCommand::OpenInventoryMenu {
                player_id: ScriptPlayerId::new(1),
                menu,
            },
        )
        .await,
    );
    assert!(root.plugin_players.menus.contains_key(&ConnectionId(1)));
    assert!(
        root.other_dimensions[&crate::dimensions::Dimension::Nether].players[&actor]
            .open
            .is_some()
    );
    execute(
        &mut root,
        admit(
            "shop",
            ScriptCommand::CloseInventoryMenu {
                player_id: ScriptPlayerId::new(1),
                menu_id: "child-shop".into(),
            },
        )
        .await,
    );
    assert!(!root.plugin_players.menus.contains_key(&ConnectionId(1)));
    assert!(
        root.other_dimensions[&crate::dimensions::Dimension::Nether].players[&actor]
            .open
            .is_none()
    );
}

#[tokio::test]
async fn unlock_session_removes_the_lock_only_when_the_fence_is_enabled() {
    for required in [false, true] {
        let (mut state, _) = fixture();
        state.require_plugin_login = required;
        state.locked_sessions.insert(ConnectionId(1));
        // Journaled plugin command path: admitted through the boundary, then
        // applied by the native owner.
        let admitted = admit(
            "auth",
            ScriptCommand::UnlockSession {
                player_id: ScriptPlayerId::new(1),
            },
        )
        .await;
        assert!(crate::plugin_player::execute(&mut state, admitted).is_none());
        assert_eq!(
            state.locked_sessions.contains(&ConnectionId(1)),
            !required,
            "config off is a no-op; config on removes the exact session"
        );
    }
}

#[tokio::test]
async fn unavailable_loader_item_authority_returns_correlated_failure_without_items() {
    let (mut state, actor) = fixture();
    let before = state.players[&actor].inventory.clone();
    let request = mc_script::ScriptLoaderItemGrantRequest::try_new(
        "item-grant",
        ScriptPlayerId::new(1),
        "shop:display-block",
        1,
    )
    .unwrap();
    let event = execute(
        &mut state,
        admit("shop", ScriptCommand::GrantLoaderBlockItem { request }).await,
    )
    .unwrap();
    assert_eq!(event.target_plugin_id(), Some("shop"));
    assert!(
        matches!(event.kind(), ScriptEventKind::LoaderItemGrantResult {
        request_id, failure: Some(mc_script::ScriptLoaderItemGrantFailure::RuntimeUnavailable),
        .. } if request_id == "item-grant")
    );
    assert_eq!(state.players[&actor].inventory, before);
}

#[tokio::test]
async fn reserved_source_floor_refusal_preserves_inventory_kv_and_versions_atomically() {
    let (mut state, actor) = fixture();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] =
        item(&state, "minecraft:emerald", 8);
    state.plugins.apply_storage(
        "shop",
        &[ScriptStorageMutation::CompareAndSwap {
            key: "shop-ledger".into(),
            expected_version: None,
            value: "before".into(),
        }],
    );
    let version = state.plugins.version("shop", "shop-ledger").unwrap();
    let endpoint = ScriptInventoryEndpoint::PlayerInventory { player_id: 1 };
    let fence = crate::plugin_inventories::load(&state, "shop", &endpoint)
        .unwrap()
        .fence;
    let request = mc_script::ScriptOperationRequest::try_new(
        "reserve-stock",
        mc_script::ScriptOperation::Inventory {
            operation: mc_script::ScriptOwnedInventoryOperation::Reserve {
                operation_id: "held-stock".into(),
                endpoint,
                resource_plan: mc_script::ScriptInventoryResourcePlan::new(vec![
                    mc_script::ScriptInventoryWorkPortion::new(
                        1,
                        vec![mc_script::ScriptInventoryMaterial::new(
                            "minecraft:emerald".into(),
                            6,
                        )],
                    ),
                ]),
                expected_revision: fence,
            },
        },
    )
    .unwrap();
    let reserved = crate::plugin_inventories::execute(&mut state, "shop", &request);
    assert_eq!(reserved.state(), mc_script::ScriptOperationState::Committed);
    let inventory = state.players[&actor].inventory.clone();
    let plugins = state.plugins.clone();

    let event = execute(
        &mut state,
        admit("shop", purchase(Some(version), -3, 2)).await,
    )
    .unwrap();
    assert_eq!(event.target_plugin_id(), Some("shop"));
    assert!(
        matches!(event.kind(), ScriptEventKind::InventoryStorageTransactionResult {
        request_id, committed: false } if request_id == "purchase")
    );
    assert_eq!(state.players[&actor].inventory, inventory);
    assert_eq!(state.plugins, plugins);
    assert_eq!(state.plugins.version("shop", "shop-ledger"), Some(version));

    let transaction = mc_script::ScriptPlayerInventoryTransaction::try_new(
        "held-debit",
        ScriptPlayerId::new(1),
        vec![ScriptInventoryResourceDelta::try_new("minecraft:emerald", -3).unwrap()],
    )
    .unwrap();
    let event = execute(
        &mut state,
        admit(
            "shop",
            ScriptCommand::PlayerInventoryTransaction { transaction },
        )
        .await,
    )
    .unwrap();
    assert_eq!(event.target_plugin_id(), Some("shop"));
    assert!(
        matches!(event.kind(), ScriptEventKind::PlayerInventoryTransactionResult {
        request_id, failure: Some(ScriptPlayerInventoryFailure::InsufficientResource),
        .. } if request_id == "held-debit")
    );
    assert_eq!(state.players[&actor].inventory, inventory);
    assert_eq!(state.plugins, plugins);
    assert_eq!(state.plugins.version("shop", "shop-ledger"), Some(version));

    // Only the held floor is protected: spending the two free units succeeds.
    let event = execute(
        &mut state,
        admit("shop", purchase(Some(version), -2, 2)).await,
    )
    .unwrap();
    assert!(matches!(
        event.kind(),
        ScriptEventKind::InventoryStorageTransactionResult {
            committed: true,
            ..
        }
    ));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 6);
    assert_eq!(
        state
            .plugins
            .record("shop", "shop-ledger")
            .unwrap()
            .value
            .as_deref(),
        Some("purchased")
    );
    assert!(state.plugins.version("shop", "shop-ledger").unwrap() > version);
}
