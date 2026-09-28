//! Contract-level tests for the crate root: DTO validation, boundary admission,
//! route authority and the host half of the script boundary.
//!
//! Sibling of `lib.rs`, matching the crate's `*_tests.rs` convention instead of
//! an inline `mod tests`.

use super::*;

use std::num::NonZeroU64;
use std::time::Duration;

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("test value is non-zero")
}

fn nonzero_u64(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("test value is non-zero")
}

#[test]
fn script_boundary_is_bounded_and_preserves_the_first_event() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let first = ScriptEvent::server_started();
    let second = ScriptEvent::server_tick(1);

    boundary.try_enqueue_event(first.clone()).unwrap();

    assert_eq!(
        boundary.try_enqueue_event(second),
        Err(ScriptQueueError::Full)
    );
    assert_eq!(endpoint.recv_event_blocking(), Some(first));
}

#[test]
fn closing_event_admission_rejects_new_events_and_drains_buffered_events() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let buffered = ScriptEvent::server_started();
    boundary.try_enqueue_event(buffered.clone()).unwrap();

    boundary.close_event_admission();

    assert_eq!(
        boundary.try_enqueue_event(ScriptEvent::server_tick(1)),
        Err(ScriptQueueError::Closed)
    );
    assert_eq!(endpoint.recv_event_blocking(), Some(buffered));
    assert_eq!(endpoint.recv_event_blocking(), None);
}

#[test]
fn closing_event_admission_drains_buffered_targeted_events() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_plugin_storage()
        .validate()
        .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    let request = ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap();
    let buffered = ScriptEvent::plugin_storage_get_result("shop", &request, None, None).unwrap();
    boundary.try_enqueue_event(buffered.clone()).unwrap();

    boundary.close_event_admission();

    assert_eq!(endpoint.recv_event_blocking(), Some(buffered));
    assert_eq!(endpoint.recv_event_blocking(), None);
}

#[tokio::test]
async fn targeted_event_delivery_waits_for_host_consumer_progress() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_plugin_storage()
        .validate()
        .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let request = ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap();
    let result = ScriptEvent::plugin_storage_get_result("shop", &request, None, None).unwrap();
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    let mut delivery = Box::pin(boundary.enqueue_targeted_event(result));
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    assert!(matches!(
        Future::poll(Pin::as_mut(&mut delivery), &mut cx),
        Poll::Pending
    ));

    assert!(
        matches!(endpoint.recv_event().await, Some(event) if event.event_name() == "server.started")
    );
    assert!(matches!(
        Future::poll(Pin::as_mut(&mut delivery), &mut cx),
        Poll::Ready(Ok(()))
    ));
    assert!(
        matches!(endpoint.recv_event().await, Some(event) if event.event_name() == "plugin.storage.get_result")
    );
}

#[test]
fn targeted_event_admitted_before_replacement_is_not_delivered_afterwards() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(2), nonzero(1));
    let initial = ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_plugin_storage()
        .validate()
        .unwrap();
    endpoint.register_plugin_routes(&initial).unwrap();
    let request = ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap();
    boundary
        .try_enqueue_event(
            ScriptEvent::plugin_storage_get_result("shop", &request, None, None).unwrap(),
        )
        .unwrap();

    let replacement =
        ScriptPluginManifest::new("shop", "Shop", "0.2.0", COMPONENT_PLUGIN_API_VERSION)
            .declare_plugin_storage()
            .validate()
            .unwrap();
    endpoint
        .commit_reload(&[replacement], Vec::new(), || {})
        .unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();

    assert!(matches!(
        endpoint.recv_event_blocking(),
        Some(event) if event.event_name() == "server.started"
    ));
}

#[tokio::test]
async fn targeted_event_delivery_exits_when_host_receiver_is_closed() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    drop(endpoint);
    let request = ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap();
    let result = ScriptEvent::plugin_storage_get_result("shop", &request, None, None).unwrap();

    assert!(matches!(
        boundary.enqueue_targeted_event(result).await,
        Err(ScriptQueueError::Closed)
    ));
}

#[tokio::test]
async fn required_event_delivery_waits_for_receiver_capacity_notification() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let first = ScriptEvent::server_started();
    let second = ScriptEvent::server_tick(1);
    boundary.try_enqueue_event(first.clone()).unwrap();
    let (receiver_ready_tx, receiver_ready_rx) = tokio::sync::oneshot::channel();
    let (release_receiver_tx, release_receiver_rx) = tokio::sync::oneshot::channel();
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        receiver_ready_tx.send(()).unwrap();
        release_receiver_rx.await.unwrap();
        let first = endpoint.recv_event().await;
        let second = endpoint.recv_event().await;
        received_tx.send((first, second)).unwrap();
    });

    let delivery = boundary.enqueue_required_event(second.clone());
    tokio::pin!(delivery);
    tokio::select! {
        result = &mut delivery => panic!("required delivery completed without capacity: {result:?}"),
        result = receiver_ready_rx => result.unwrap(),
    }

    release_receiver_tx.send(()).unwrap();
    delivery.await.unwrap();
    assert_eq!(received_rx.await.unwrap(), (Some(first), Some(second)));
}

#[tokio::test]
async fn required_event_delivery_wakes_closed_when_saturated_receiver_closes() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let (receiver_ready_tx, receiver_ready_rx) = tokio::sync::oneshot::channel();
    let (close_receiver_tx, close_receiver_rx) = tokio::sync::oneshot::channel();
    let (receiver_closed_tx, receiver_closed_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        receiver_ready_tx.send(()).unwrap();
        close_receiver_rx.await.unwrap();
        drop(endpoint);
        receiver_closed_tx.send(()).unwrap();
    });

    let delivery = boundary.enqueue_required_event(ScriptEvent::server_tick(1));
    tokio::pin!(delivery);
    tokio::select! {
        result = &mut delivery => panic!("required delivery completed before closure: {result:?}"),
        result = receiver_ready_rx => result.unwrap(),
    }

    close_receiver_tx.send(()).unwrap();
    receiver_closed_rx.await.unwrap();
    assert_eq!(delivery.await, Err(ScriptQueueError::Closed));
}

#[test]
fn script_event_queue_errors_stay_below_the_result_size_threshold() {
    assert!(
        std::mem::size_of::<ScriptQueueError>() <= 1,
        "queue errors must contain only bounded state metadata"
    );
}

#[test]
fn event_dtos_are_stable_snapshots_without_host_handles() {
    let event = ScriptEvent::player_joined_with_context(
        ScriptPlayerId::new(42),
        ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0),
    );

    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerJoined {
            player_id,
            username,
            ..
        } if *player_id == ScriptPlayerId::new(42) && username == "kaiser"
    ));
    assert_eq!(ScriptPlayerId::new(42).value(), 42);
    assert_eq!(ScriptEntityId::new(99).value(), 99);
}

#[test]
fn player_block_broken_event_is_a_validated_post_commit_snapshot() {
    let context = ScriptPlayerContext::new(
        "123e4567-e89b-12d3-a456-426614174000",
        "kaiser",
        true,
        12.25,
        70.0,
        -4.5,
    );
    let event = ScriptEvent::try_player_block_broken_with_context(
        ScriptPlayerId::new(42),
        context.clone(),
        "minecraft:overworld",
        "minecraft:deepslate/diamond_ore",
        3,
        -64,
        -9,
        ScriptGameMode::Survival,
    )
    .unwrap();

    assert_eq!(event.event_name(), "player.block_broken");
    assert_eq!(event.target_plugin_id(), None);
    assert_eq!(event.validate(), Ok(()));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerBlockBroken {
            player_id,
            context: event_context,
            dimension,
            block_id,
            x,
            y,
            z,
            game_mode,
        } if *player_id == ScriptPlayerId::new(42)
            && event_context == &context
            && dimension == "minecraft:overworld"
            && block_id == "minecraft:deepslate/diamond_ore"
            && (*x, *y, *z) == (3, -64, -9)
            && *game_mode == ScriptGameMode::Survival
    ));
}

#[test]
fn player_block_broken_rejects_invalid_resource_identifiers() {
    let context = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
    let oversized_block_id = format!("minecraft:{}", "a".repeat(MAX_SCRIPT_RESOURCE_ID_BYTES));
    for (dimension, block_id) in [
        ("overworld", "minecraft:stone"),
        ("minecraft:overworld", "minecraft:Stone"),
        ("minecraft:overworld", oversized_block_id.as_str()),
    ] {
        assert!(
            ScriptEvent::try_player_block_broken_with_context(
                ScriptPlayerId::new(42),
                context.clone(),
                dimension,
                block_id,
                0,
                64,
                0,
                ScriptGameMode::Creative,
            )
            .is_err()
        );
    }
}

#[test]
fn player_block_placed_event_is_a_validated_snapshot() {
    let context = ScriptPlayerContext::new(
        "123e4567-e89b-12d3-a456-426614174000",
        "kaiser",
        true,
        12.25,
        70.0,
        -4.5,
    );
    let event = ScriptEvent::try_player_block_placed_with_context(
        ScriptPlayerId::new(42),
        context.clone(),
        "minecraft:overworld",
        "minecraft:oak_log",
        3,
        -64,
        -9,
        ScriptGameMode::Survival,
    )
    .unwrap();

    assert_eq!(event.event_name(), "player.block_placed");
    assert_eq!(event.target_plugin_id(), None);
    assert_eq!(event.validate(), Ok(()));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerBlockPlaced {
            player_id,
            context: event_context,
            dimension,
            block_id,
            x,
            y,
            z,
            game_mode,
        } if *player_id == ScriptPlayerId::new(42)
            && event_context == &context
            && dimension == "minecraft:overworld"
            && block_id == "minecraft:oak_log"
            && (*x, *y, *z) == (3, -64, -9)
            && *game_mode == ScriptGameMode::Survival
    ));
}

#[test]
fn player_block_placed_rejects_invalid_resource_identifiers() {
    let context = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
    let oversized_block_id = format!("minecraft:{}", "a".repeat(MAX_SCRIPT_RESOURCE_ID_BYTES));
    for (dimension, block_id) in [
        ("overworld", "minecraft:stone"),
        ("minecraft:overworld", "minecraft:Stone"),
        ("minecraft:overworld", oversized_block_id.as_str()),
    ] {
        assert!(
            ScriptEvent::try_player_block_placed_with_context(
                ScriptPlayerId::new(42),
                context.clone(),
                dimension,
                block_id,
                0,
                64,
                0,
                ScriptGameMode::Creative,
            )
            .is_err()
        );
    }
}

#[test]
fn block_break_game_modes_are_closed_stable_strings() {
    assert_eq!(ScriptGameMode::Survival.as_str(), "survival");
    assert_eq!(ScriptGameMode::Creative.as_str(), "creative");
    assert_eq!(ScriptGameMode::Adventure.as_str(), "adventure");
}

#[test]
fn player_item_crafted_event_is_a_validated_snapshot_without_integer_caps() {
    let context = ScriptPlayerContext::new(
        "123e4567-e89b-12d3-a456-426614174000",
        "kaiser",
        true,
        12.25,
        70.0,
        -4.5,
    );
    let event = ScriptEvent::try_player_item_crafted_with_context(
        ScriptPlayerId::new(42),
        context.clone(),
        "minecraft:overworld",
        "minecraft:oak_planks",
        u64::from(u32::MAX) + 1,
        u32::MAX,
        ScriptCraftingSource::CraftingTable,
        ScriptGameMode::Adventure,
    )
    .unwrap();

    assert_eq!(event.event_name(), "player.item_crafted");
    assert_eq!(event.target_plugin_id(), None);
    assert_eq!(event.validate(), Ok(()));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerItemCrafted {
            player_id,
            context: event_context,
            dimension,
            item_id,
            count,
            craft_count,
            source,
            game_mode,
        } if *player_id == ScriptPlayerId::new(42)
            && event_context == &context
            && dimension == "minecraft:overworld"
            && item_id == "minecraft:oak_planks"
            && *count == u64::from(u32::MAX) + 1
            && *craft_count == u32::MAX
            && *source == ScriptCraftingSource::CraftingTable
            && *game_mode == ScriptGameMode::Adventure
    ));
}

#[test]
fn player_item_crafted_rejects_invalid_ids_and_zero_counts() {
    let context = ScriptPlayerContext::new("player-42", "kaiser", false, 0.0, 64.0, 0.0);
    for (dimension, item_id, count, craft_count) in [
        ("overworld", "minecraft:stick", 1, 1),
        ("minecraft:overworld", "minecraft:Stick", 1, 1),
        ("minecraft:overworld", "minecraft:stick", 0, 1),
        ("minecraft:overworld", "minecraft:stick", 1, 0),
    ] {
        assert!(
            ScriptEvent::try_player_item_crafted_with_context(
                ScriptPlayerId::new(42),
                context.clone(),
                dimension,
                item_id,
                count,
                craft_count,
                ScriptCraftingSource::Inventory,
                ScriptGameMode::Survival,
            )
            .is_err()
        );
    }
}

#[test]
fn crafting_sources_are_closed_stable_strings() {
    assert_eq!(ScriptCraftingSource::Inventory.as_str(), "inventory");
    assert_eq!(
        ScriptCraftingSource::CraftingTable.as_str(),
        "crafting_table"
    );
}

#[test]
fn queued_player_context_keeps_the_pose_observed_at_publication() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let mut latest_accepted_pose = (12.25, 70.0, -4.5);
    boundary
        .try_enqueue_event(ScriptEvent::player_chat_with_context(
            ScriptPlayerId::new(42),
            "snapshot",
            ScriptPlayerContext::new(
                "123e4567-e89b-12d3-a456-426614174000",
                "kaiser",
                true,
                latest_accepted_pose.0,
                latest_accepted_pose.1,
                latest_accepted_pose.2,
            ),
        ))
        .unwrap();

    latest_accepted_pose = (23.5, 71.0, 8.75);
    assert_eq!(latest_accepted_pose, (23.5, 71.0, 8.75));

    let event = endpoint.recv_event_blocking().unwrap();
    let ScriptEventKind::PlayerChat { context, .. } = event.kind() else {
        panic!("expected player chat event");
    };
    assert_eq!(context.uuid(), "123e4567-e89b-12d3-a456-426614174000");
    assert_eq!(context.username(), "kaiser");
    assert!(context.operator());
    assert_eq!((context.x(), context.y(), context.z()), (12.25, 70.0, -4.5));
}

#[test]
fn command_batch_reports_full_without_dropping_existing_commands() {
    let mut batch = CommandBatch::new(nonzero(1));
    let first = ScriptCommand::BroadcastChatMessage {
        message: "first".to_owned(),
    };
    let second = ScriptCommand::BroadcastChatMessage {
        message: "second".to_owned(),
    };

    batch.try_push(first.clone()).unwrap();

    assert_eq!(
        batch.try_push(second),
        Err(CommandBatchError::Full { limit: nonzero(1) })
    );
    assert_eq!(batch.commands(), &[first]);
}

#[test]
fn manifest_validation_rejects_an_obsolete_requested_api_version() {
    let requested = ScriptApiVersion::new(0, 6, 0);
    let manifest = ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", requested);

    assert_eq!(
        manifest.validate(),
        Err(ScriptPluginManifestError::UnsupportedScriptApiVersion {
            requested,
            supported: COMPONENT_PLUGIN_API_VERSION,
        })
    );
}

#[test]
fn component_contract_exposes_extended_events() {
    for event_name in [
        "player.block_broken",
        "player.block_placed",
        "player.item_crafted",
        "player.item_picked_up",
        "player.entity_killed",
        "player.died",
        "plugin.storage.get_result",
        "plugin.storage.cas_result",
        "plugin.storage.delete_result",
        "inventory.menu.clicked",
        "inventory.storage_transaction.result",
        "player.zone_entered",
        "player.zone_exited",
        "zone.command_result",
        "player.teleport_result",
    ] {
        assert!(is_supported_event_name(event_name), "missing {event_name}");
    }

    let manifest =
        ScriptPluginManifest::new("pickup", "Pickup", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .subscribe_event(" PLAYER.ITEM_PICKED_UP ")
            .validate()
            .unwrap();
    assert_eq!(
        manifest.event_subscriptions()[0].event_name(),
        "player.item_picked_up"
    );
}

#[test]
fn spawn_entity_capability_is_exact_and_manifest_bounded() {
    let invalid = ScriptPluginManifest::new(
        "spawn-test",
        "Spawn Test",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_spawn_entity_type("pig");
    assert!(matches!(
        invalid.validate(),
        Err(ScriptPluginManifestError::InvalidSpawnEntityType { .. })
    ));

    for invalid_type in [
        "minecraft:Pig".to_owned(),
        ":pig".to_owned(),
        "minecraft:".to_owned(),
        format!("minecraft:{}", "a".repeat(MAX_SCRIPT_RESOURCE_ID_BYTES)),
    ] {
        let invalid = ScriptPluginManifest::new(
            "spawn-test",
            "Spawn Test",
            "0.1.0",
            COMPONENT_PLUGIN_API_VERSION,
        )
        .declare_spawn_entity_type(invalid_type);
        assert!(matches!(
            invalid.validate(),
            Err(ScriptPluginManifestError::InvalidSpawnEntityType { .. })
                | Err(ScriptPluginManifestError::FieldTooLong {
                    field: "spawn entity type",
                    ..
                })
        ));
    }

    let duplicate = ScriptPluginManifest::new(
        "spawn-test",
        "Spawn Test",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_spawn_entity_type("minecraft:pig")
    .declare_spawn_entity_type("minecraft:pig");
    assert!(matches!(
        duplicate.validate(),
        Err(ScriptPluginManifestError::DuplicateSpawnEntityType { .. })
    ));

    let mut bounded = ScriptPluginManifest::new(
        "spawn-test",
        "Spawn Test",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    );
    for index in 0..=MAX_SPAWN_ENTITY_TYPES {
        bounded = bounded.declare_spawn_entity_type(format!("minecraft:test_{index}"));
    }
    assert!(matches!(
        bounded.validate(),
        Err(ScriptPluginManifestError::TooManySpawnEntityTypes { .. })
    ));

    let position = ScriptPosition::try_new(1.25, 64.0, -2.5).unwrap();
    let pig = ScriptCommand::SpawnEntity {
        request_id: "pig".to_owned(),
        actor: ScriptPlayerId::new(7),
        entity_type: "minecraft:pig".to_owned(),
        position,
    };
    let cow = ScriptCommand::SpawnEntity {
        request_id: "cow".to_owned(),
        actor: ScriptPlayerId::new(7),
        entity_type: "minecraft:cow".to_owned(),
        position,
    };
    let capabilities = CommandCapabilities::none().allow_spawn_entity_type("minecraft:pig");
    let mut batch = CommandBatch::new(nonzero(2));

    assert_eq!(
        pig.required_capability_kind(),
        Some(ScriptCommandCapabilityKind::SpawnEntity)
    );
    batch
        .try_push_authorized(pig.clone(), &capabilities)
        .unwrap();
    assert_eq!(
        batch.try_push_authorized(cow, &capabilities),
        Err(CommandBatchError::PermissionDenied {
            capability: ScriptCommandCapabilityKind::SpawnEntity,
        })
    );
    assert_eq!(batch.commands(), std::slice::from_ref(&pig));
}

#[test]
fn manifest_validation_rejects_invalid_plugin_ids() {
    let blank_id = ScriptPluginManifest::new(" ", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION);
    assert_eq!(
        blank_id.validate(),
        Err(ScriptPluginManifestError::BlankPluginId)
    );

    let invalid_id =
        ScriptPluginManifest::new("Day Time", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION);
    assert_eq!(
        invalid_id.validate(),
        Err(ScriptPluginManifestError::InvalidPluginId {
            plugin_id: "Day Time".to_owned(),
        })
    );
}

#[test]
fn manifest_validation_accepts_and_deduplicates_safe_player_command_roots() {
    let manifest = ScriptPluginManifest::new(
        "greetings",
        "Greetings",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("hello")
    .declare_player_command_root("hello")
    .declare_player_command_root("warp_home-2");

    assert_eq!(
        manifest.validate().unwrap().player_command_roots(),
        &["hello".to_owned(), "warp_home-2".to_owned()]
    );
}

#[test]
fn manifest_validation_enforces_player_command_root_byte_limit() {
    let boundary_root = "a".repeat(MAX_PLAYER_COMMAND_ROOT_BYTES);
    let boundary = ScriptPluginManifest::new(
        "greetings",
        "Greetings",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root(&boundary_root);
    assert_eq!(
        boundary.validate().unwrap().player_command_roots(),
        &[boundary_root]
    );

    let over_limit_root = "a".repeat(MAX_PLAYER_COMMAND_ROOT_BYTES + 1);
    let over_limit = ScriptPluginManifest::new(
        "greetings",
        "Greetings",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root(&over_limit_root);
    assert_eq!(
        over_limit.validate(),
        Err(ScriptPluginManifestError::FieldTooLong {
            field: "player command root",
            max_bytes: MAX_PLAYER_COMMAND_ROOT_BYTES,
        })
    );
}

#[test]
fn manifest_validation_rejects_unsafe_and_reserved_player_command_roots() {
    for root in ["Hello", "/hello", "hello there", "hello.world"] {
        let manifest = ScriptPluginManifest::new(
            "greetings",
            "Greetings",
            "0.1.0",
            COMPONENT_PLUGIN_API_VERSION,
        )
        .declare_player_command_root(root);

        assert_eq!(
            manifest.validate(),
            Err(ScriptPluginManifestError::InvalidPlayerCommandRoot {
                root: root.to_owned(),
            })
        );
    }
    assert_eq!(
        ScriptPluginManifest::new(
            "greetings",
            "Greetings",
            "0.1.0",
            COMPONENT_PLUGIN_API_VERSION
        )
        .declare_player_command_root("")
        .validate(),
        Err(ScriptPluginManifestError::EmptyField {
            field: "player command root",
        })
    );

    for root in ["gamemode", "defaultgamemode", "tp", "teleport"] {
        let manifest = ScriptPluginManifest::new(
            "greetings",
            "Greetings",
            "0.1.0",
            COMPONENT_PLUGIN_API_VERSION,
        )
        .declare_player_command_root(root);

        assert_eq!(
            manifest.validate(),
            Err(ScriptPluginManifestError::ReservedPlayerCommandRoot {
                root: root.to_owned(),
            })
        );
    }
}

#[test]
fn player_command_event_is_an_immutable_targeted_snapshot() {
    let event = ScriptEvent::try_player_command_with_context(
        "greetings",
        ScriptPlayerId::new(42),
        ScriptPlayerContext::new("player-42", "Alex", false, 0.0, 64.0, 0.0),
        "hello",
        "one  two ",
    )
    .unwrap();

    assert_eq!(event.event_name(), "player.command");
    assert_eq!(event.target_plugin_id(), Some("greetings"));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerCommand {
            player_id,
            username,
            root,
            arguments,
            ..
        } if *player_id == ScriptPlayerId::new(42)
            && username == "Alex"
            && root == "hello"
            && arguments == "one  two "
    ));
}

#[test]
fn player_command_boundary_reports_full_and_closed_without_retaining_events() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let manifest = ScriptPluginManifest::new(
        "greetings",
        "Greetings",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("hello")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();

    assert_eq!(boundary.player_command_roots(), vec!["hello".to_owned()]);
    let context = || ScriptPlayerContext::new("player-7", "Alex", false, 0.0, 64.0, 0.0);
    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(7),
            context(),
            "missing arg",
        ),
        Ok(PlayerCommandAdmission::NotOwned)
    );
    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(7),
            context(),
            "/hello one  two ",
        ),
        Ok(PlayerCommandAdmission::Enqueued)
    );
    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(7),
            context(),
            "hello later",
        ),
        Err(ScriptQueueError::Full)
    );
    assert!(matches!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent {
            kind: ScriptEventKind::PlayerCommand { username, root, arguments, .. },
            ..
        }) if username == "Alex" && root == "hello" && arguments == "one  two "
    ));
    drop(endpoint);
    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(7),
            context(),
            "hello after-close",
        ),
        Err(ScriptQueueError::Closed)
    );
    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(7),
            context(),
            "hello after-clear",
        ),
        Ok(PlayerCommandAdmission::NotOwned)
    );
}

#[test]
fn owned_player_command_arguments_are_bounded_without_panicking_or_queuing_rejections() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(1));
    let manifest =
        ScriptPluginManifest::new("owner", "Owner", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .declare_player_command_root("owned")
            .validate()
            .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    let context = ScriptPlayerContext::try_new("player-7", "Alex", false, 0.0, 64.0, 0.0).unwrap();

    let accepted = format!("owned {}", "a".repeat(MAX_SCRIPT_CHAT_MESSAGE_BYTES));
    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(7),
            context.clone(),
            &accepted,
        ),
        Ok(PlayerCommandAdmission::Enqueued)
    );
    let event = endpoint.recv_event_blocking().unwrap();
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerCommand { arguments, .. }
            if arguments.len() == MAX_SCRIPT_CHAT_MESSAGE_BYTES
    ));

    for rejected_len in [MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1, 32_767] {
        let rejected = format!("owned {}", "b".repeat(rejected_len));
        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                context.clone(),
                &rejected,
            ),
            Ok(PlayerCommandAdmission::OwnedRejected {
                error: ScriptDtoError::ValueTooLong {
                    field: "player command arguments",
                    max_bytes: MAX_SCRIPT_CHAT_MESSAGE_BYTES,
                    actual_bytes: rejected_len,
                },
            })
        );

        assert_eq!(
            boundary.try_enqueue_player_command_with_context(
                ScriptPlayerId::new(7),
                context.clone(),
                "owned sentinel",
            ),
            Ok(PlayerCommandAdmission::Enqueued)
        );
        let event = endpoint.recv_event_blocking().unwrap();
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerCommand { arguments, .. } if arguments == "sentinel"
        ));
    }

    let constructor_error = ScriptEvent::try_player_command_with_context(
        "owner",
        ScriptPlayerId::new(7),
        context,
        "owned",
        "x".repeat(MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1),
    )
    .unwrap_err();
    assert_eq!(
        constructor_error,
        ScriptDtoError::ValueTooLong {
            field: "player command arguments",
            max_bytes: MAX_SCRIPT_CHAT_MESSAGE_BYTES,
            actual_bytes: MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1,
        }
    );
}

#[test]
fn operator_player_commands_require_verified_context_and_are_denied_before_a_full_queue() {
    let manifest = ScriptPluginManifest::new(
        "admin-day",
        "Admin Day",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_operator_command_root("adminday")
    .validate()
    .unwrap();
    assert!(manifest.player_command_roots().is_empty());
    assert_eq!(manifest.operator_command_roots(), ["adminday"]);
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    endpoint.register_plugin_routes(&manifest).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();

    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(7),
            ScriptPlayerContext::new("player-7", "Alex", false, 0.0, 64.0, 0.0),
            "adminday",
        ),
        Ok(PlayerCommandAdmission::PermissionDenied)
    );
    assert_eq!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent::server_started()),
        "denied command must not be enqueued behind a full queue"
    );
    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(7),
            ScriptPlayerContext::new("verified-id", "Alex", true, 0.0, 0.0, 0.0),
            "adminday",
        ),
        Ok(PlayerCommandAdmission::Enqueued)
    );
}

#[test]
fn player_command_registration_enforces_aggregate_root_limit_atomically() {
    let mut boundary_manifest = ScriptPluginManifest::new(
        "boundary",
        "Boundary",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    );
    for index in 0..MAX_PLAYER_COMMAND_ROOTS {
        boundary_manifest =
            boundary_manifest.declare_player_command_root(format!("command{index}"));
    }
    let boundary_manifest = boundary_manifest.validate().unwrap();
    let over_limit_manifest = ScriptPluginManifest::new(
        "over-limit",
        "Over Limit",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("one_more")
    .validate()
    .unwrap();
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));

    endpoint.register_plugin_routes(&boundary_manifest).unwrap();
    assert_eq!(
        boundary.player_command_roots().len(),
        MAX_PLAYER_COMMAND_ROOTS
    );
    assert_eq!(
        endpoint.register_plugin_routes(&over_limit_manifest),
        Err(ScriptRouteRegistrationError::RootLimitExceeded {
            limit: MAX_PLAYER_COMMAND_ROOTS,
            requested: MAX_PLAYER_COMMAND_ROOTS + 1,
        })
    );
    assert_eq!(
        boundary.player_command_roots().len(),
        MAX_PLAYER_COMMAND_ROOTS,
        "over-limit registration must not partially mutate ownership"
    );
}

#[test]
fn manifest_validation_normalizes_event_subscriptions_and_dependencies() {
    let manifest =
        ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .with_load_phase(ScriptPluginLoadPhase::Startup)
            .subscribe_event(" Player.Chat ")
            .subscribe_event("server.tick")
            .declare_dependency(" Economy ", ScriptPluginDependencyRelation::Required)
            .declare_dependency("chat-tools", ScriptPluginDependencyRelation::Optional)
            .declare_dependency("spawn-protect", ScriptPluginDependencyRelation::LoadBefore);

    let validated = manifest.validate().unwrap();

    assert_eq!(validated.load_phase(), ScriptPluginLoadPhase::Startup);
    assert_eq!(
        validated.event_subscriptions(),
        &[
            ScriptEventSubscription::new("player.chat".to_owned()),
            ScriptEventSubscription::new("server.tick".to_owned()),
        ]
    );
    assert_eq!(
        validated.dependencies(),
        &[
            ScriptPluginDependency::new(
                "economy".to_owned(),
                ScriptPluginDependencyRelation::Required
            ),
            ScriptPluginDependency::new(
                "chat-tools".to_owned(),
                ScriptPluginDependencyRelation::Optional
            ),
            ScriptPluginDependency::new(
                "spawn-protect".to_owned(),
                ScriptPluginDependencyRelation::LoadBefore,
            ),
        ]
    );
}

#[test]
fn manifest_validation_rejects_unsafe_event_subscriptions() {
    let invalid =
        ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .subscribe_event("player.inventory.clicked");
    assert_eq!(
        invalid.validate(),
        Err(ScriptPluginManifestError::InvalidEventName {
            event_name: "player.inventory.clicked".to_owned(),
        })
    );

    let duplicate =
        ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .subscribe_event("player.chat")
            .subscribe_event(" Player.Chat ");
    assert_eq!(
        duplicate.validate(),
        Err(ScriptPluginManifestError::DuplicateEventSubscription {
            event_name: "player.chat".to_owned(),
        })
    );
}

#[test]
fn manifest_validation_rejects_unsafe_dependency_declarations() {
    let blank =
        ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .declare_dependency(" ", ScriptPluginDependencyRelation::Required);
    assert_eq!(
        blank.validate(),
        Err(ScriptPluginManifestError::BlankDependencyPluginId)
    );

    let invalid =
        ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .declare_dependency("Economy Tools", ScriptPluginDependencyRelation::Required);
    assert_eq!(
        invalid.validate(),
        Err(ScriptPluginManifestError::InvalidDependencyPluginId {
            plugin_id: "economy tools".to_owned(),
        })
    );

    let self_dependency =
        ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .declare_dependency("Daytime", ScriptPluginDependencyRelation::Required);
    assert_eq!(
        self_dependency.validate(),
        Err(ScriptPluginManifestError::SelfDependency {
            plugin_id: "daytime".to_owned(),
        })
    );

    let duplicate =
        ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .declare_dependency("Economy", ScriptPluginDependencyRelation::Required)
            .declare_dependency(" economy ", ScriptPluginDependencyRelation::Optional);
    assert_eq!(
        duplicate.validate(),
        Err(ScriptPluginManifestError::DuplicateDependency {
            plugin_id: "economy".to_owned(),
        })
    );
}

#[test]
fn trusted_host_derives_command_capabilities_from_validated_manifest() {
    let manifest =
        ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .declare_world_time();
    let validated = manifest.validate().unwrap();
    let capabilities = validated.to_command_capabilities();

    let world_time_command = ScriptCommand::SetWorldTime {
        request: ScriptWorldTimeSetRequest::try_new("day", 1_000).unwrap(),
    };
    let world_time = world_time_command.required_capability().unwrap();
    assert!(capabilities.allows(world_time));

    let entity_damage_command = ScriptCommand::DamageEntity {
        request: ScriptEntityDamageRequest::try_new("hit", ScriptEntityId::new(7), 1.0).unwrap(),
    };
    let entity_damage = entity_damage_command.required_capability().unwrap();
    assert!(!capabilities.allows(entity_damage));
}

#[test]
fn runtime_controls_reserve_fuel_memory_timeout_and_shutdown() {
    let controls = RuntimeControls::unrestricted()
        .with_fuel(nonzero_u64(100))
        .with_memory_bytes(nonzero(4096))
        .with_timeout(Duration::from_millis(50))
        .with_shutdown_requested();

    assert_eq!(controls.fuel(), Some(nonzero_u64(100)));
    assert_eq!(controls.memory_bytes(), Some(nonzero(4096)));
    assert_eq!(controls.timeout(), Some(Duration::from_millis(50)));
    assert!(controls.shutdown_requested());
}

#[test]
fn runtime_contract_accepts_event_reference_and_returns_bounded_batch() {
    struct EchoRuntime;

    impl ScriptRuntime for EchoRuntime {
        fn handle_event(
            &mut self,
            event: &ScriptEvent,
            context: RuntimeContext<'_>,
        ) -> RuntimeResult<CommandBatch> {
            let mut batch = context.command_batch();
            if let ScriptEventKind::PlayerChat {
                player_id, message, ..
            } = event.kind()
            {
                batch
                    .try_push(ScriptCommand::SendChatMessage {
                        player_id: *player_id,
                        message: format!("echo: {message}"),
                    })
                    .expect("context-created batch has remaining capacity");
            }
            Ok(batch)
        }
    }

    let controls = RuntimeControls::unrestricted();
    let context = RuntimeContext::new(&controls, nonzero(1));
    let event = ScriptEvent::player_chat_with_context(
        ScriptPlayerId::new(7),
        "hello",
        ScriptPlayerContext::new("player-7", "Alex", false, 0.0, 64.0, 0.0),
    );

    let commands = EchoRuntime
        .handle_event(&event, context)
        .unwrap()
        .into_commands();

    assert_eq!(
        commands,
        vec![ScriptCommand::SendChatMessage {
            player_id: ScriptPlayerId::new(7),
            message: "echo: hello".to_owned(),
        }]
    );
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerChat {
            player_id, message, ..
        } if *player_id == ScriptPlayerId::new(7) && message == "hello"
    ));
}

#[test]
fn plugin_capabilities_reject_undeclared_storage_and_transaction_requests() {
    let mut batch = CommandBatch::new(nonzero(2));
    let storage = ScriptCommand::PluginStorageGet {
        request: ScriptPluginStorageGetRequest::try_new("read-balance", "balance:player-7")
            .unwrap(),
    };
    let transaction = ScriptCommand::InventoryStorageTransaction {
        transaction: ScriptInventoryStorageTransaction::try_new(
            "buy-1",
            ScriptPlayerId::new(7),
            vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
            vec![
                ScriptStorageMutation::compare_and_swap("balance:player-7", Some(2), "9").unwrap(),
            ],
        )
        .unwrap(),
    };

    assert_eq!(
        batch.try_push_authorized(storage, &CommandCapabilities::none()),
        Err(CommandBatchError::PermissionDenied {
            capability: ScriptCommandCapabilityKind::PluginStorage,
        })
    );
    assert_eq!(
        batch.try_push_authorized(transaction, &CommandCapabilities::none()),
        Err(CommandBatchError::PermissionDenied {
            capability: ScriptCommandCapabilityKind::InventoryStorageTransactions,
        })
    );
    assert!(batch.commands().is_empty());
}

#[test]
fn transaction_revalidates_directly_constructed_storage_mutations() {
    for mutation in [
        ScriptStorageMutation::CompareAndSwap {
            key: String::new(),
            expected_version: None,
            value: "value".to_owned(),
        },
        ScriptStorageMutation::CompareAndSwap {
            key: "key".to_owned(),
            expected_version: None,
            value: String::new(),
        },
        ScriptStorageMutation::Delete {
            key: "x".repeat(MAX_PLUGIN_STORAGE_KEY_BYTES + 1),
            expected_version: None,
        },
    ] {
        assert!(
            ScriptInventoryStorageTransaction::try_new(
                "transaction",
                ScriptPlayerId::new(7),
                vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap(),],
                vec![mutation],
            )
            .is_err()
        );
    }
}

#[test]
fn public_command_submission_denies_privileged_raw_commands_and_provenance_replay() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(2), nonzero(2));
    assert!(
        endpoint
            .try_submit_command(ScriptCommand::PluginStorageGet {
                request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
            })
            .is_err()
    );

    let replay = ScriptCommand::HostAttached {
        provenance: ScriptCommandProvenance::for_host_plugin(Arc::from("owner"), 1),
        request: Arc::new(ScriptCommand::BroadcastChatMessage {
            message: "owned".to_owned(),
        }),
    };
    let mut forged_batch = CommandBatch::new(nonzero(1));
    assert_eq!(
        forged_batch.try_push(replay.clone()),
        Err(CommandBatchError::ProvenanceRejected)
    );
    assert!(endpoint.try_submit_command(replay).is_err());
    assert!(boundary.try_recv_command_for_test().is_none());
}

#[test]
fn sealed_admission_rechecks_manifest_capabilities_and_rejects_the_whole_batch() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(2));
    let manifest =
        ScriptPluginManifest::new("plain", "Plain", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .validate()
            .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let forged_capabilities = CommandCapabilities::none().allow_plugin_storage();
    let mut batch = CommandBatch::new(nonzero(1));
    batch
        .try_push_authorized(
            ScriptCommand::PluginStorageGet {
                request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
            },
            &forged_capabilities,
        )
        .unwrap();

    assert!(matches!(
        endpoint.try_submit_plugin_batch(&admission, batch),
        Err(ScriptBatchSubmissionError::Rejected {
            error: CommandBatchError::PermissionDenied {
                capability: ScriptCommandCapabilityKind::PluginStorage,
            },
            ..
        })
    ));
    assert!(boundary.try_recv_command_for_test().is_none());
}

#[test]
fn saturated_command_queue_rejects_an_atomic_batch_without_publishing_a_prefix() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(2));
    let existing = ScriptCommand::BroadcastChatMessage {
        message: "existing".to_owned(),
    };
    endpoint.try_submit_command(existing.clone()).unwrap();

    let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_plugin_storage()
        .validate()
        .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let capabilities = manifest.to_command_capabilities();
    let mut batch = CommandBatch::new(nonzero(2));
    for request_id in ["first", "second"] {
        batch
            .try_push_authorized(
                ScriptCommand::PluginStorageGet {
                    request: ScriptPluginStorageGetRequest::try_new(request_id, "balance").unwrap(),
                },
                &capabilities,
            )
            .unwrap();
    }

    assert!(matches!(
        endpoint.try_submit_plugin_batch(&admission, batch),
        Err(ScriptBatchSubmissionError::Full(rejected))
            if rejected.commands().len() == 2
    ));
    assert_eq!(boundary.try_recv_command_for_test(), Some(existing));
    assert_eq!(boundary.try_recv_command_for_test(), None);
}

#[test]
fn bounded_dtos_reject_oversized_ids_values_invalid_bounds_and_resources() {
    let oversized_id = "x".repeat(MAX_SCRIPT_ID_BYTES + 1);
    assert!(matches!(
        ScriptAxisAlignedZone::try_new(
            &oversized_id,
            "minecraft:overworld",
            ScriptPosition::try_new(0.0, 0.0, 0.0).unwrap(),
            ScriptPosition::try_new(1.0, 1.0, 1.0).unwrap(),
        ),
        Err(ScriptDtoError::ValueTooLong { .. })
    ));
    assert!(matches!(
        ScriptStorageMutation::compare_and_swap(
            "balance",
            None,
            "x".repeat(MAX_PLUGIN_STORAGE_VALUE_BYTES + 1),
        ),
        Err(ScriptDtoError::ValueTooLong { .. })
    ));
    assert!(matches!(
        ScriptAxisAlignedZone::try_new(
            "shop",
            "minecraft:Overworld",
            ScriptPosition::try_new(2.0, 0.0, 0.0).unwrap(),
            ScriptPosition::try_new(1.0, 1.0, 1.0).unwrap(),
        ),
        Err(ScriptDtoError::InvalidBounds)
    ));
    assert!(matches!(
        ScriptInventoryMenuItem::try_new("invalid", 1, None),
        Err(ScriptDtoError::InvalidResourceId { .. })
    ));
}

#[test]
fn zone_protection_is_explicit_normalized_and_plugin_agnostic() {
    let protection =
        ScriptZoneProtection::try_actor_or_operator("12345678-1234-5678-1234-567812345678")
            .unwrap();
    assert_eq!(
        protection.allowed_actor_uuid(),
        "12345678123456781234567812345678"
    );
    assert!(protection.allows_actor("12345678123456781234567812345678", false));
    assert!(protection.allows_actor("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", true));
    assert!(!protection.allows_actor("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", false));
    assert!(matches!(
        ScriptZoneProtection::try_actor_or_operator("claim-owner"),
        Err(ScriptDtoError::InvalidId {
            field: "protection actor uuid",
            ..
        })
    ));
}

#[test]
fn targeted_results_validate_correlation_fields_and_coherent_outcomes() {
    assert!(matches!(
        ScriptPluginStorageGetRequest::try_new("", "balance"),
        Err(ScriptDtoError::EmptyValue { .. })
    ));
    assert!(matches!(
        ScriptPluginStorageCompareAndSwapRequest::try_new("write", "balance", None, ""),
        Err(ScriptDtoError::EmptyValue { .. })
    ));
    let get = ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap();
    let cas = ScriptPluginStorageCompareAndSwapRequest::try_new("write", "balance", Some(3), "9")
        .unwrap();
    let delete = ScriptPluginStorageDeleteRequest::try_new("delete", "balance", Some(3)).unwrap();
    assert!(matches!(
        ScriptEvent::plugin_storage_get_result("owner", &get, Some("9".into()), None),
        Err(ScriptDtoError::InconsistentResult { .. })
    ));
    assert!(matches!(
        ScriptEvent::plugin_storage_get_result(
            "owner",
            &get,
            Some("x".repeat(MAX_PLUGIN_STORAGE_VALUE_BYTES + 1)),
            Some(1),
        ),
        Err(ScriptDtoError::ValueTooLong { .. })
    ));
    assert!(matches!(
        ScriptEvent::plugin_storage_cas_result("owner", &cas, true, None),
        Err(ScriptDtoError::InconsistentResult { .. })
    ));
    assert!(matches!(
        ScriptEvent::plugin_storage_delete_result("owner", &delete, true, None),
        Err(ScriptDtoError::InconsistentResult { .. })
    ));

    let event =
        ScriptEvent::plugin_storage_get_result("owner.plugin", &get, Some("9".into()), Some(4))
            .unwrap();
    assert_eq!(event.target_plugin_id(), Some("owner.plugin"));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PluginStorageGetResult {
            request_id,
            key,
            value: Some(value),
            version: Some(4),
            failure: None,
        } if request_id == "read" && key == "balance" && value == "9"
    ));
}

#[test]
fn inventory_menu_and_transaction_reject_duplicate_ids_and_saturation() {
    let item = ScriptInventoryMenuItem::try_new("minecraft:apple", 1, None).unwrap();
    assert!(matches!(
        ScriptInventoryMenu::try_new(
            "catalog",
            "Catalog",
            vec![
                ScriptInventoryMenuSlot::new(0, item.clone()),
                ScriptInventoryMenuSlot::new(0, item),
            ],
        ),
        Err(ScriptDtoError::DuplicateId { .. })
    ));

    let mut inventory = Vec::new();
    for index in 0..=MAX_INVENTORY_STORAGE_MUTATIONS {
        inventory.push(
            ScriptInventoryResourceDelta::try_new(format!("minecraft:item_{index}"), 1).unwrap(),
        );
    }
    assert!(matches!(
        ScriptInventoryStorageTransaction::try_new(
            "saturated",
            ScriptPlayerId::new(7),
            inventory,
            vec![ScriptStorageMutation::compare_and_swap("balance", None, "0").unwrap()],
        ),
        Err(ScriptDtoError::TooManyEntries { .. })
    ));
    let duplicate = ScriptInventoryStorageTransaction::try_new(
        "duplicate",
        ScriptPlayerId::new(7),
        vec![
            ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap(),
            ScriptInventoryResourceDelta::try_new("minecraft:apple", -1).unwrap(),
        ],
        vec![ScriptStorageMutation::compare_and_swap("balance", None, "0").unwrap()],
    );
    assert!(matches!(duplicate, Err(ScriptDtoError::DuplicateId { .. })));
}

#[tokio::test]
async fn host_attaches_plugin_provenance_to_component_command_submissions() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_plugin_storage()
        .validate()
        .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let mut batch = CommandBatch::new(nonzero(1));
    batch
        .try_push_authorized(
            ScriptCommand::PluginStorageGet {
                request: ScriptPluginStorageGetRequest::try_new("read", "balance:player-7")
                    .unwrap(),
            },
            &manifest.to_command_capabilities(),
        )
        .unwrap();
    endpoint.try_submit_plugin_batch(&admission, batch).unwrap();

    let Some(ScriptCommand::HostAttached {
        provenance,
        request,
    }) = boundary.recv_command().await
    else {
        panic!("host-attached command missing");
    };
    assert_eq!(provenance.plugin_id(), "shop");
    assert!(matches!(*request, ScriptCommand::PluginStorageGet { .. }));
}

#[test]
fn raw_command_admission_rejects_every_unbounded_string_variant() {
    let (_boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(8));
    for command in [
        ScriptCommand::SendChatMessage {
            player_id: ScriptPlayerId::new(1),
            message: "x".repeat(MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1),
        },
        ScriptCommand::BroadcastChatMessage {
            message: "x".repeat(MAX_SCRIPT_CHAT_MESSAGE_BYTES + 1),
        },
        ScriptCommand::DisconnectPlayer {
            player_id: ScriptPlayerId::new(1),
            reason: "x".repeat(MAX_SCRIPT_DISCONNECT_REASON_BYTES + 1),
        },
        ScriptCommand::SpawnEntity {
            request_id: "spawn".to_owned(),
            actor: ScriptPlayerId::new(1),
            entity_type: "x".repeat(MAX_SCRIPT_RESOURCE_ID_BYTES + 1),
            position: ScriptPosition::try_new(0.0, 64.0, 0.0).unwrap(),
        },
        ScriptCommand::CloseInventoryMenu {
            player_id: ScriptPlayerId::new(1),
            menu_id: "x".repeat(MAX_SCRIPT_ID_BYTES + 1),
        },
        ScriptCommand::RemoveZone {
            zone_id: "x".repeat(MAX_SCRIPT_ID_BYTES + 1),
        },
    ] {
        assert!(matches!(
            endpoint.try_submit_command(command),
            Err(ScriptCommandSubmissionError::InvalidCommand { .. })
        ));
    }
}

#[test]
fn public_validation_errors_do_not_retain_or_render_rejected_input() {
    let rejected = "secret".repeat(200_000);
    let dto_error = ScriptPluginStorageGetRequest::try_new(&rejected, "balance").unwrap_err();
    let dto_debug = format!("{dto_error:?}");
    assert!(dto_debug.len() < 256);
    assert!(!dto_debug.contains("secret"));

    let (_boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let command_error = endpoint
        .try_submit_command(ScriptCommand::BroadcastChatMessage { message: rejected })
        .unwrap_err();
    let command_debug = format!("{command_error:?}");
    assert!(command_debug.len() < 256);
    assert!(!command_debug.contains("secret"));

    let denied_entity = "minecraft:secret_entity";
    let mut batch = CommandBatch::new(nonzero(1));
    let denied = batch
        .try_push_authorized(
            ScriptCommand::SpawnEntity {
                request_id: "spawn".to_owned(),
                actor: ScriptPlayerId::new(1),
                entity_type: denied_entity.to_owned(),
                position: ScriptPosition::try_new(0.0, 64.0, 0.0).unwrap(),
            },
            &CommandCapabilities::none(),
        )
        .unwrap_err();
    assert_eq!(
        denied,
        CommandBatchError::PermissionDenied {
            capability: ScriptCommandCapabilityKind::SpawnEntity,
        }
    );
    assert!(!format!("{denied:?}").contains(denied_entity));
}

#[test]
fn manifest_preflight_rejects_oversized_identity_and_collections() {
    assert!(matches!(
        ScriptPluginManifest::new(
            "x".repeat(MAX_PLUGIN_ID_BYTES + 1),
            "Plugin",
            "0.1.0",
            COMPONENT_PLUGIN_API_VERSION,
        )
        .validate(),
        Err(ScriptPluginManifestError::FieldTooLong {
            field: "plugin id",
            ..
        })
    ));

    let mut manifest =
        ScriptPluginManifest::new("bounded", "Plugin", "0.1.0", COMPONENT_PLUGIN_API_VERSION);
    for index in 0..=MAX_MANIFEST_EVENT_SUBSCRIPTIONS {
        manifest = manifest.subscribe_event(format!("event.{index}"));
    }
    assert!(matches!(
        manifest.validate(),
        Err(ScriptPluginManifestError::TooManyEntries {
            field: "event subscriptions",
            ..
        })
    ));
}

#[tokio::test]
async fn host_admission_ticket_is_exact_and_one_shot() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_plugin_storage()
        .validate()
        .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let mut batch = CommandBatch::new(nonzero(1));
    batch
        .try_push_authorized(
            ScriptCommand::PluginStorageGet {
                request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
            },
            &manifest.to_command_capabilities(),
        )
        .unwrap();
    endpoint.try_submit_plugin_batch(&admission, batch).unwrap();
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 1);

    let command = boundary.recv_command().await.unwrap();
    let replay = command.clone();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 0);
    assert_eq!(admitted.plugin_id(), "shop");
    let result = admitted
        .plugin_storage_get_result(Some("9"), Some(1))
        .unwrap();
    assert_eq!(result.target_plugin_id(), Some("shop"));
    assert!(matches!(
        boundary.accept_host_command(replay),
        Err(ScriptCommandAcceptanceError::UnknownOrConsumed)
    ));
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 0);
}

#[tokio::test]
async fn dropping_last_host_attached_clone_cancels_admission_and_recovers_capacity() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_plugin_storage()
        .validate()
        .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let make_batch = |request_id: &str| {
        let mut batch = CommandBatch::new(nonzero(1));
        batch
            .try_push_authorized(
                ScriptCommand::PluginStorageGet {
                    request: ScriptPluginStorageGetRequest::try_new(request_id, "balance").unwrap(),
                },
                &manifest.to_command_capabilities(),
            )
            .unwrap();
        batch
    };

    endpoint
        .try_submit_plugin_batch(&admission, make_batch("first"))
        .unwrap();
    let command = boundary.recv_command().await.unwrap();
    let replay = command.clone();
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 1);

    drop(command);
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 1);
    drop(replay);
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 0);

    endpoint
        .try_submit_plugin_batch(&admission, make_batch("replacement"))
        .unwrap();
    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    assert_eq!(admitted.plugin_id(), "shop");
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 0);
}

#[test]
fn dropping_command_receiver_cancels_every_queued_admission() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(2));
    let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_plugin_storage()
        .validate()
        .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let mut batch = CommandBatch::new(nonzero(2));
    for request_id in ["first", "second"] {
        batch
            .try_push_authorized(
                ScriptCommand::PluginStorageGet {
                    request: ScriptPluginStorageGetRequest::try_new(request_id, "balance").unwrap(),
                },
                &manifest.to_command_capabilities(),
            )
            .unwrap();
    }
    endpoint.try_submit_plugin_batch(&admission, batch).unwrap();
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 2);

    drop(boundary);

    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 0);
}

#[tokio::test]
async fn admitted_storage_failures_are_explicit_for_every_request_variant() {
    let commands = [
        ScriptCommand::PluginStorageGet {
            request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
        },
        ScriptCommand::PluginStorageCompareAndSwap {
            request: ScriptPluginStorageCompareAndSwapRequest::try_new(
                "write", "balance", None, "9",
            )
            .unwrap(),
        },
        ScriptCommand::PluginStorageDelete {
            request: ScriptPluginStorageDeleteRequest::try_new("delete", "balance", Some(1))
                .unwrap(),
        },
    ];

    for (index, command) in commands.into_iter().enumerate() {
        let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
        let manifest =
            ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
                .declare_plugin_storage()
                .validate()
                .unwrap();
        let admission = HostCommandAdmission::from_manifest(&manifest);
        let mut batch = CommandBatch::new(nonzero(1));
        batch
            .try_push_authorized(command, &manifest.to_command_capabilities())
            .unwrap();
        endpoint.try_submit_plugin_batch(&admission, batch).unwrap();

        let admitted = boundary
            .accept_host_command(boundary.recv_command().await.unwrap())
            .unwrap();
        let event = admitted
            .plugin_storage_failure_result(ScriptPluginStorageFailure::Unavailable)
            .unwrap();
        assert_eq!(event.target_plugin_id(), Some("shop"));
        assert!(matches!(
            (index, event.kind()),
            (
                0,
                ScriptEventKind::PluginStorageGetResult {
                    value: None,
                    version: None,
                    failure: Some(ScriptPluginStorageFailure::Unavailable),
                    ..
                },
            ) | (
                1,
                ScriptEventKind::PluginStorageCasResult {
                    applied: false,
                    version: None,
                    failure: Some(ScriptPluginStorageFailure::Unavailable),
                    ..
                },
            ) | (
                2,
                ScriptEventKind::PluginStorageDeleteResult {
                    deleted: false,
                    version: None,
                    failure: Some(ScriptPluginStorageFailure::Unavailable),
                    ..
                },
            )
        ));
    }
}

#[tokio::test]
async fn host_admission_rejects_request_substitution_and_consumes_the_ticket() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let manifest = ScriptPluginManifest::new("shop", "Shop", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_plugin_storage()
        .validate()
        .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let mut batch = CommandBatch::new(nonzero(1));
    batch
        .try_push_authorized(
            ScriptCommand::PluginStorageGet {
                request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
            },
            &manifest.to_command_capabilities(),
        )
        .unwrap();
    endpoint.try_submit_plugin_batch(&admission, batch).unwrap();
    let original = boundary.recv_command().await.unwrap();
    let ScriptCommand::HostAttached { provenance, .. } = original.clone() else {
        panic!("host-attached command missing");
    };
    let substituted = ScriptCommand::HostAttached {
        provenance,
        request: Arc::new(ScriptCommand::PluginStorageGet {
            request: ScriptPluginStorageGetRequest::try_new("read", "balance").unwrap(),
        }),
    };

    assert_eq!(
        boundary.accept_host_command(substituted),
        Err(ScriptCommandAcceptanceError::RequestMismatch)
    );
    assert_eq!(
        boundary.accept_host_command(original),
        Err(ScriptCommandAcceptanceError::UnknownOrConsumed)
    );
}

#[tokio::test]
async fn unconsumed_host_tickets_are_bounded_and_recover_after_drop_or_acceptance() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(256));
    let manifest =
        ScriptPluginManifest::new("bounded", "Bounded", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .validate()
            .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let mut unconsumed = Vec::new();
    for batch_index in 0..8 {
        let mut batch = CommandBatch::new(nonzero(32));
        for command_index in 0..32 {
            batch
                .try_push(ScriptCommand::BroadcastChatMessage {
                    message: format!("{batch_index}:{command_index}"),
                })
                .unwrap();
        }
        endpoint.try_submit_plugin_batch(&admission, batch).unwrap();
        for _ in 0..32 {
            unconsumed.push(boundary.recv_command().await.unwrap());
        }
    }
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 256);

    let mut overflow = CommandBatch::new(nonzero(1));
    overflow
        .try_push(ScriptCommand::BroadcastChatMessage {
            message: "overflow".to_owned(),
        })
        .unwrap();
    assert!(matches!(
        endpoint.try_submit_plugin_batch(&admission, overflow),
        Err(ScriptBatchSubmissionError::Rejected {
            error: CommandBatchError::AdmissionUnavailable,
            ..
        })
    ));

    drop(unconsumed.pop().unwrap());
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 255);
    let mut recovered_after_drop = CommandBatch::new(nonzero(1));
    recovered_after_drop
        .try_push(ScriptCommand::BroadcastChatMessage {
            message: "recovered-after-drop".to_owned(),
        })
        .unwrap();
    endpoint
        .try_submit_plugin_batch(&admission, recovered_after_drop)
        .unwrap();
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 256);

    boundary
        .accept_host_command(unconsumed.pop().unwrap())
        .unwrap();
    assert_eq!(endpoint.host_admissions_pending_len_for_test(), 255);
    let mut recovered_after_accept = CommandBatch::new(nonzero(1));
    recovered_after_accept
        .try_push(ScriptCommand::BroadcastChatMessage {
            message: "recovered-after-accept".to_owned(),
        })
        .unwrap();
    endpoint
        .try_submit_plugin_batch(&admission, recovered_after_accept)
        .unwrap();
}

#[test]
fn poisoned_plugin_route_authority_is_cleared_and_permanently_disabled() {
    let (_boundary, endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let manifest =
        ScriptPluginManifest::new("owner", "Owner", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
            .declare_player_command_root("owned")
            .validate()
            .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    endpoint.poison_plugin_routes_for_test();

    assert!(endpoint.plugin_route_roots_for_test().is_empty());
    assert_eq!(
        endpoint.register_plugin_routes(&manifest),
        Err(ScriptRouteRegistrationError::AuthorityPoisoned)
    );
    assert!(!endpoint.plugin_route_owner_for_test("owned"));
}

#[test]
fn player_context_rejects_oversized_identity_and_nonfinite_coordinates() {
    assert!(matches!(
        ScriptPlayerContext::try_new(
            "x".repeat(MAX_SCRIPT_PLAYER_UUID_BYTES + 1),
            "Alex",
            false,
            0.0,
            64.0,
            0.0,
        ),
        Err(ScriptDtoError::ValueTooLong {
            field: "player uuid",
            ..
        })
    ));
    assert!(matches!(
        ScriptPlayerContext::try_new("uuid", "Alex", false, f64::NAN, 64.0, 0.0),
        Err(ScriptDtoError::InvalidBounds)
    ));
}

#[test]
fn component_api_version_requires_the_current_contract_version() {
    assert_eq!(COMPONENT_PLUGIN_API_VERSION, ScriptApiVersion::new(0, 7, 0));
    for requested in [
        ScriptApiVersion::new(0, 0, 0),
        ScriptApiVersion::new(0, 4, 9),
        ScriptApiVersion::new(0, 5, 1),
        ScriptApiVersion::new(0, 6, 1),
        ScriptApiVersion::new(0, 6, 0),
        ScriptApiVersion::new(1, 0, 0),
    ] {
        assert!(!supports_plugin_api_version(
            requested,
            COMPONENT_PLUGIN_API_VERSION
        ));
        assert_eq!(
            ScriptPluginManifest::new("daytime", "Daytime", "0.1.0", requested).validate(),
            Err(ScriptPluginManifestError::UnsupportedScriptApiVersion {
                requested,
                supported: COMPONENT_PLUGIN_API_VERSION,
            })
        );
    }
}
