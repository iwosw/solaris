use super::*;
use std::sync::Arc;

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
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
    let mut state = TickState::new(registry, world, 2, 512);
    let mut chunks = rustc_hash::FxHashMap::default();
    for x in -1..=1 {
        for z in -1..=1 {
            chunks.insert(crate::boundary::ChunkPos { x, z }, 0);
        }
    }
    state.world.ensure_blocking(&chunks);
    state.tickets.insert(
        "fixture".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 1),
    );
    state.refresh_active_chunks(state.tick + 1);
    for x in -16..32 {
        for z in -16..32 {
            state.world.set_block(x, 63, z, state.registry.states.stone);
        }
    }
    state
}
fn request(operation: ScriptOperation) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new("test-request", operation).unwrap()
}
fn spawn(state: &mut TickState, id: &str, generation: &str, x: f64) -> ScriptNpcSnapshot {
    let result = execute(
        state,
        id,
        &request(ScriptOperation::Npc {
            operation: ScriptNpcOperation::Spawn {
                operation_id: format!("spawn-{generation}"),
                generation_id: generation.to_owned(),
                position: ScriptPosition::try_new(x, 64.0, 2.5).unwrap(),
                entity_type: "minecraft:villager".into(),
            },
        }),
    );
    let ScriptOperationPayload::Npc { result } = result.payload() else {
        panic!("NPC snapshot: {result:?}");
    };
    let ScriptNpcResult::Snapshot { npc } = &**result else {
        panic!("snapshot");
    };
    npc.clone()
}
fn route(state: &mut TickState, npc: &ScriptNpcSnapshot) -> ScriptNpcRouteStatus {
    let result = execute(
        state,
        "owner",
        &request(ScriptOperation::NpcRoute {
            operation: ScriptNpcRouteOperation::Admit {
                operation_id: "route-admit".into(),
                admission: ScriptNpcRouteAdmission::new(
                    npc.handle.clone(),
                    npc.revision,
                    0,
                    "minecraft:overworld".into(),
                    vec![ScriptBlockPosition::new(10, 64, 2)],
                    200,
                    Vec::new(),
                ),
            },
        }),
    );
    let ScriptOperationPayload::NpcRoute { result } = result.payload() else {
        panic!("route status: {result:?}");
    };
    let ScriptNpcRouteResult::Status { route } = &**result else {
        panic!("status");
    };
    (**route).clone()
}

#[test]
fn route_probe_shares_tick_budget_and_retries_after_global_reset() {
    for kind in [
        crate::tick_budget::Kind::Decision,
        crate::tick_budget::Kind::PathNode,
    ] {
        let mut state = fixture();
        let npc = spawn(&mut state, "owner", "budget-route", 2.5);
        let admission = request(ScriptOperation::NpcRoute {
            operation: ScriptNpcRouteOperation::Admit {
                operation_id: "budget-route".into(),
                admission: ScriptNpcRouteAdmission::new(
                    npc.handle.clone(),
                    npc.revision,
                    0,
                    state.dimension.name().into(),
                    vec![ScriptBlockPosition::new(10, 64, 2)],
                    200,
                    Vec::new(),
                ),
            },
        });
        state.tick_budget.exhaust_for_test(kind);
        let denied = execute(&mut state, "owner", &admission);
        assert_eq!(denied.failure(), Some(ScriptOperationFailure::Blocked));
        assert_eq!(state.tick_budget.remaining(kind), 0);
        state.tick_budget.reset(state.tick + 1);
        assert_eq!(execute(&mut state, "owner", &admission).failure(), None);
    }
}

#[test]
fn order_probe_shares_tick_budget_and_preserves_blocked_route_outcome() {
    for kind in [
        crate::tick_budget::Kind::Decision,
        crate::tick_budget::Kind::PathNode,
    ] {
        let mut state = fixture();
        let npc = spawn(&mut state, "owner", "budget-order", 2.5);
        let order = request(ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::IssueOrder {
                operation_id: "budget-order".into(),
                handles: vec![npc.handle],
                expected_order_revisions: vec![0],
                order: ScriptNpcOrder::Move {
                    dimension: state.dimension.name().into(),
                    anchor: ScriptBlockPosition::new(10, 64, 2),
                    heading_degrees: 0,
                    formation: ScriptFormation::new(ScriptFormationKind::Line, 4),
                },
            },
        });
        state.tick_budget.exhaust_for_test(kind);
        let denied = execute(&mut state, "owner", &order);
        let ScriptOperationPayload::NpcOrder { result } = denied.payload() else {
            panic!("order outcome: {denied:?}");
        };
        let ScriptNpcOrderResult::Order { members, .. } = &**result else {
            panic!("order outcome: {denied:?}");
        };
        assert_eq!(members[0].state, ScriptOrderMemberState::BlockedRoute);
        assert_eq!(state.tick_budget.remaining(kind), 0);
        state.tick_budget.reset(state.tick + 1);
        let accepted = execute(&mut state, "owner", &order);
        let ScriptOperationPayload::NpcOrder { result } = accepted.payload() else {
            panic!("order outcome: {accepted:?}");
        };
        let ScriptNpcOrderResult::Order { members, .. } = &**result else {
            panic!("order outcome: {accepted:?}");
        };
        assert_eq!(members[0].state, ScriptOrderMemberState::Applied);
    }
}

#[test]
fn navigation_probe_shares_tick_budget_and_preserves_retry_target() {
    for kind in [
        crate::tick_budget::Kind::Decision,
        crate::tick_budget::Kind::PathNode,
    ] {
        let mut state = fixture();
        let npc = spawn(&mut state, "owner", "budget-navigation", 2.5);
        let slot = owned_slot(&state, "owner", &npc.handle).unwrap();
        let target = ScriptBlockPosition::new(10, 64, 2);
        state.tick_budget.exhaust_for_test(kind);
        assert!(!navigate(&mut state, slot, target));
        let villager = state.entities.row(slot).villager.as_ref().unwrap();
        assert!(villager.navigation_path.is_empty());
        assert_eq!(
            villager.navigation_target, None,
            "denial must leave the target retryable"
        );
        state.tick_budget.reset(state.tick + 1);
        assert!(navigate(&mut state, slot, target));
        assert!(
            !state
                .entities
                .row(slot)
                .villager
                .as_ref()
                .unwrap()
                .navigation_path
                .is_empty()
        );
    }
}

#[test]
fn foreign_reachability_probe_charges_root_pool_not_dormant_dimension_budget() {
    use crate::dimensions::Dimension;
    for kind in [
        crate::tick_budget::Kind::Decision,
        crate::tick_budget::Kind::PathNode,
    ] {
        let mut state = fixture();
        let mut nether = fixture();
        nether.dimension = Dimension::Nether;
        let dormant = nether.tick_budget.remaining(kind);
        state
            .other_dimensions
            .insert(Dimension::Nether, Box::new(nether));
        state.tick_budget.exhaust_for_test(kind);
        assert_eq!(
            reachable(
                &mut state,
                Dimension::Nether.name(),
                [2, 64, 2],
                [10, 64, 2]
            ),
            Ok(false)
        );
        assert_eq!(
            state.other_dimensions[&Dimension::Nether]
                .tick_budget
                .remaining(kind),
            dormant
        );
        state.tick_budget.reset(state.tick + 1);
        let before = state.tick_budget.remaining(kind);
        assert_eq!(
            reachable(
                &mut state,
                Dimension::Nether.name(),
                [2, 64, 2],
                [10, 64, 2]
            ),
            Ok(true)
        );
        assert!(state.tick_budget.remaining(kind) < before);
        assert_eq!(
            state.other_dimensions[&Dimension::Nether]
                .tick_budget
                .remaining(kind),
            dormant
        );
    }
}
#[test]
fn stable_spawn_identity_has_one_native_body_and_cross_owner_custody_refuses() {
    let mut state = fixture();
    let first = spawn(&mut state, "owner", "generation", 2.5);
    let again = spawn(&mut state, "owner", "generation", 9.5);
    assert_eq!(first.entity_uuid, again.entity_uuid);
    assert_eq!(first.revision, again.revision);
    assert_eq!(state.entities.len(), 1);
    let outcome = execute(
        &mut state,
        "rival",
        &request(ScriptOperation::Npc {
            operation: ScriptNpcOperation::Spawn {
                operation_id: "rival".into(),
                generation_id: "generation".into(),
                position: ScriptPosition::try_new(9.5, 64.0, 2.5).unwrap(),
                entity_type: "minecraft:villager".into(),
            },
        }),
    );
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));
    assert_eq!(state.entities.len(), 1);
    let slot = owned_slot(&state, "owner", &first.handle).unwrap();
    assert!(state.entities.row(slot).persistent);
    assert!(state.entities.row(slot).controls.no_ai);
    assert!(state.plugin_spawned.contains(&state.entities.body(slot).id));
    validate(&state).unwrap();
}
#[test]
fn release_preserves_physical_entity_and_restores_previous_native_flags() {
    let mut state = fixture();
    let npc = spawn(&mut state, "owner", "release", 2.5);
    let slot = owned_slot(&state, "owner", &npc.handle).unwrap();
    let outcome = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::Npc {
            operation: ScriptNpcOperation::Release {
                operation_id: "release".into(),
                handle: npc.handle.clone(),
                expected_revision: npc.revision,
            },
        }),
    );
    assert_eq!(outcome.failure(), None);
    assert!(!state.entities.row(slot).controls.no_ai);
    assert!(!state.entities.row(slot).persistent);
    assert!(state.entities.body(slot).alive);
    assert!(custodian(&state, state.entities.row(slot).uuid).is_none());
    assert_eq!(state.entities.len(), 1);
    validate(&state).unwrap();
}
#[test]
fn route_admission_and_start_never_manufacture_arrival_and_require_epoch_permits() {
    let mut state = fixture();
    let npc = spawn(&mut state, "owner", "traveller", 2.5);
    let route = route(&mut state, &npc);
    assert_eq!(route.state, ScriptNpcRouteState::Prepared);
    assert!(route.arrival.is_none());
    assert_eq!(
        validate_permit(&state, "owner", &npc.handle, None),
        Err(ScriptOperationFailure::Busy)
    );
    let permit = ScriptNpcExecutionPermit::new(route.route_ref.clone(), route.executor_epoch);
    validate_permit(&state, "owner", &npc.handle, Some(&permit)).unwrap();
    let result = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::NpcRoute {
            operation: ScriptNpcRouteOperation::Start {
                operation_id: "start-route".into(),
                route_ref: route.route_ref.clone(),
                expected_epoch: route.executor_epoch,
                cargo_operation_id: None,
            },
        }),
    );
    assert_eq!(result.failure(), None);
    state.tick += 1;
    advance(&mut state);
    let result = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::NpcRoute {
            operation: ScriptNpcRouteOperation::Query {
                route_ref: route.route_ref.clone(),
            },
        }),
    );
    let ScriptOperationPayload::NpcRoute { result } = result.payload() else {
        panic!("route");
    };
    let ScriptNpcRouteResult::Status { route } = &**result else {
        panic!("status");
    };
    assert_eq!(route.state, ScriptNpcRouteState::Running);
    assert_eq!(route.cursor, 0);
    assert!(route.arrival.is_none());
    assert_eq!(route.pose.as_ref().unwrap().x, 2.5);
    validate(&state).unwrap();
}
#[test]
fn native_birth_provenance_not_proximity_authorizes_child_custody() {
    let mut state = fixture();
    let a = spawn(&mut state, "owner", "parent-a", 2.5);
    let b = spawn(&mut state, "owner", "parent-b", 3.5);
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let child = state.spawn_mob(type_id, [4.5, 64.0, 2.5], 20.0);
    let slot = state.entities.slot_of(child).unwrap();
    let child_uuid = state.entities.row(slot).uuid;
    state.entities.row_mut(slot).villager.as_mut().unwrap().age = -24_000;
    let mut parent_handles = [a.handle.clone(), b.handle.clone()];
    parent_handles.sort();
    let operation = ScriptNpcOperation::AdoptNativeChild {
        operation_id: "claim-child".into(),
        entity_uuid: child_uuid.to_string(),
        parent_handles: parent_handles.clone(),
        expected_entity_revision: 0,
    };
    let rejected = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::Npc {
            operation: operation.clone(),
        }),
    );
    assert_eq!(rejected.failure(), Some(ScriptOperationFailure::Forbidden));
    let birth = state.entities.row_mut(slot).villager.as_mut().unwrap();
    birth.birth_origin = Some(ScriptNativeBirthOrigin::new(
        [a.entity_uuid.clone(), b.entity_uuid.clone()],
        0,
        state.tick,
    ));
    birth.birth_sequence = Some(state.tick * 65_536 + 1);
    let claimed = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::Npc { operation }),
    );
    assert_eq!(claimed.failure(), None);
    assert_eq!(state.entities.len(), 3);
    validate(&state).unwrap();
}
#[test]
fn missing_native_tool_pauses_work_without_fake_units_or_deltas() {
    let mut state = fixture();
    let npc = spawn(&mut state, "owner", "worker", 2.5);
    let result = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::AssignWork {
                operation_id: "mine".into(),
                handle: npc.handle.clone(),
                work: ScriptNpcWorkOrder::Mine {
                    area: ScriptWorkArea::new(
                        "minecraft:overworld".into(),
                        ScriptBlockPosition::new(4, 63, 2),
                        ScriptBlockPosition::new(4, 63, 2),
                    ),
                    tool: "minecraft:iron_pickaxe".into(),
                },
                work_units: 1,
                expected_revision: 0,
            },
        }),
    );
    let ScriptOperationPayload::NpcOrder { result } = result.payload() else {
        panic!("work");
    };
    let ScriptNpcOrderResult::Work { assignment } = &**result else {
        panic!("assignment");
    };
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingTool));
    assert_eq!(assignment.work_units_done, 0);
    assert!(assignment.changes.is_empty());
    assert!(state.plugin_block_writes.is_empty());
    validate(&state).unwrap();
}
#[test]
fn malformed_native_ledger_refuses_restore_not_empty_success() {
    let mut state = fixture();
    state.plugins.set_domain(
        "owner",
        &format!("{RECORD_PREFIX}invalid"),
        serde_json::json!({"entity_uuid":"bogus"}),
    );
    assert!(validate(&state).is_err());
}

#[test]
fn indexed_death_transition_journals_only_its_record_and_replays_once() {
    let mut state = fixture();
    let dying = spawn(&mut state, "owner", "dying", 2.5);
    let survivor = spawn(&mut state, "owner", "survivor", 4.5);
    for index in 0..128 {
        state.spawn_mob(state.registry.cow_type_id, [index as f64, 64.0, 8.5], 10.0);
    }
    let mut initial = Vec::new();
    crate::plugin::journal(&mut state, &mut initial);
    let mut replayed = state.plugins.clone();
    let survivor_key = format!("{RECORD_PREFIX}{}", survivor.handle);
    let survivor_image = state.plugins.get_domain("owner", &survivor_key).cloned();
    let slot = state
        .entities
        .slot_of_uuid(uuid::Uuid::parse_str(&dying.entity_uuid).unwrap())
        .unwrap();
    state.entities.body_mut(slot).alive = false;
    assert_eq!(
        state.entities.slot_of_uuid(state.entities.row(slot).uuid),
        None
    );
    assert_eq!(
        state
            .entities
            .slot_of_uuid_any(state.entities.row(slot).uuid),
        Some(slot)
    );
    advance(&mut state);
    let all = records(&state, "owner").unwrap();
    assert_eq!(all[&dying.handle].disposition, ScriptNpcLifecycle::Dead);
    assert_eq!(
        all[&survivor.handle].disposition,
        ScriptNpcLifecycle::AliveUnloaded
    );
    assert_eq!(
        state.plugins.get_domain("owner", &survivor_key),
        survivor_image.as_ref()
    );
    let mut entries = Vec::new();
    crate::plugin::journal(&mut state, &mut entries);
    let [crate::journal::JournalEntry::Plugins { value }] = entries.as_slice() else {
        panic!("one native plugin delta");
    };
    let encoded = serde_json::to_value(value).unwrap();
    let changed = encoded["domains"]["owner"].as_object().unwrap();
    assert_eq!(changed.len(), 2);
    assert!(changed.contains_key(&format!("{RECORD_PREFIX}{}", dying.handle)));
    assert!(changed.contains_key("npc/revision"));
    assert!(!changed.contains_key(&survivor_key));
    replayed.apply_delta(value);
    replayed.apply_delta(value);
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&state.plugins).unwrap(),
    );
    validate(&state).unwrap();
}

#[test]
fn obsolete_whole_map_npc_image_fails_closed_instead_of_losing_custody() {
    let mut state = fixture();
    spawn(&mut state, "owner", "current", 2.5);
    state
        .plugins
        .set_domain("owner", RECORDS, serde_json::json!({}));
    assert!(records(&state, "owner").is_err());
    assert!(validate(&state).is_err());
}
#[test]
fn native_shape_ray_refuses_work_through_real_wall() {
    let mut state = fixture();
    assert!(clear_segment(
        &state,
        [2.5, 65.6, 2.5],
        [5.5, 65.0, 2.5],
        None
    ));
    state.world.set_block(4, 65, 2, state.registry.states.stone);
    assert!(!clear_segment(
        &state,
        [2.5, 65.6, 2.5],
        [5.5, 65.0, 2.5],
        None
    ));
}

#[test]
fn actual_vanilla_recipe_consumes_native_input_after_deadline_and_ack_does_not_reissue() {
    let mut state = fixture();
    let npc = spawn(&mut state, "owner", "craft-worker", 2.5);
    let slot = owned_slot(&state, "owner", &npc.handle).unwrap();
    let log = mc_data::recipes::RecipeResult {
        item: mc_data::Identifier::parse("minecraft:oak_log").unwrap(),
        count: 1,
        stew_effects: Vec::new(),
    }
    .to_stack(&state.registry.items, &state.registry.item_facts)
    .unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .inventory[0] = Some(log);
    let station = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:crafting_table").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(4, 64, 2, station);
    let work = ScriptNpcWorkOrder::Craft {
        recipe: "minecraft:oak_planks".into(),
        count: 1,
        station: ScriptWorkArea::new(
            "minecraft:overworld".into(),
            ScriptBlockPosition::new(4, 64, 2),
            ScriptBlockPosition::new(4, 64, 2),
        ),
    };
    let accepted = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::AssignWork {
                operation_id: "craft-planks".into(),
                handle: npc.handle.clone(),
                work,
                work_units: 1,
                expected_revision: 0,
            },
        }),
    );
    assert_eq!(accepted.failure(), None);
    let due = records(&state, "owner").unwrap()[&npc.handle]
        .controls
        .work
        .as_ref()
        .unwrap()
        .due_tick
        .unwrap();
    state.tick = due - 1;
    advance(&mut state);
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory[0]
            .as_ref()
            .unwrap()
            .count,
        1
    );
    assert!(
        records(&state, "owner").unwrap()[&npc.handle]
            .controls
            .work
            .as_ref()
            .unwrap()
            .receipts
            .is_empty()
    );
    state.tick = due;
    advance(&mut state);
    let all = records(&state, "owner").unwrap();
    let assignment = all[&npc.handle].controls.work.as_ref().unwrap();
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(assignment.receipts.len(), 1);
    let receipt = &assignment.receipts[0];
    receipt.validate().unwrap();
    assert_eq!(receipt.duration_ticks, 20);
    assert_eq!(receipt.simulation_tick, due);
    assert_eq!(receipt.inputs[0].item.resource_id, "minecraft:oak_log");
    assert_eq!(receipt.inputs[0].item.count, 1);
    assert_eq!(receipt.outputs[0].item.resource_id, "minecraft:oak_planks");
    assert_eq!(receipt.outputs[0].item.count, 4);
    assert_ne!(receipt.before[0].fence, receipt.after[0].fence);
    let expected = all[&npc.handle].controls.revision;
    let acknowledged = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::NpcOrder {
            operation: ScriptNpcOrderOperation::AckWorkReceipts {
                operation_id: "ack-planks".into(),
                handle: npc.handle.clone(),
                admitted_operation_id: "craft-planks".into(),
                through_sequence: 1,
                expected_revision: expected,
            },
        }),
    );
    assert_eq!(acknowledged.failure(), None);
    state.tick += 20;
    advance(&mut state);
    let all = records(&state, "owner").unwrap();
    let assignment = all[&npc.handle].controls.work.as_ref().unwrap();
    assert!(assignment.receipts.is_empty());
    assert_eq!(assignment.acknowledged_sequence, 1);
    assert_eq!(assignment.work_units_done, 1);
    let output = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .inventory[0]
        .as_ref()
        .unwrap();
    assert_eq!(
        state
            .registry
            .items
            .name_of(output.item_id)
            .unwrap()
            .as_str(),
        "minecraft:oak_planks"
    );
    assert_eq!(output.count, 4);
    validate(&state).unwrap();
}

#[test]
fn native_death_moves_sole_cargo_to_real_loot_once_and_preserves_source_fence() {
    let mut state = fixture();
    let npc = spawn(&mut state, "owner", "death-worker", 2.5);
    let slot = owned_slot(&state, "owner", &npc.handle).unwrap();
    let item = |name: &str, count| {
        mc_data::recipes::RecipeResult {
            item: mc_data::Identifier::parse(name).unwrap(),
            count,
            stew_effects: Vec::new(),
        }
        .to_stack(&state.registry.items, &state.registry.item_facts)
        .unwrap()
    };
    let cargo = item("minecraft:oak_log", 2);
    let mut helmet = item("minecraft:iron_helmet", 1);
    helmet.damage = Some(3);
    let row = state.entities.row_mut(slot);
    row.villager.as_mut().unwrap().inventory[0] = Some(cargo);
    Arc::make_mut(&mut row.appearance).equipment[5] = Some(helmet);
    crate::plugin_inventories::synchronize_entity_custody(&mut state, slot).unwrap();
    let source =
        crate::plugin_inventories::native_npc_inventory_revision(&state, "owner", &npc.handle)
            .unwrap();
    let uuid = state.entities.row(slot).uuid;
    state.entities.body_mut(slot).alive = false;
    record_death(&mut state, uuid);
    let all = records(&state, "owner").unwrap();
    let record = &all[&npc.handle];
    let receipt = record.controls.death_cargo.as_ref().unwrap();
    assert_eq!(record.disposition, ScriptNpcLifecycle::Dead);
    assert_eq!(receipt.source_inventory_revision, source);
    assert_eq!(receipt.loot.len(), 2);
    assert!(
        receipt
            .loot
            .iter()
            .any(|loot| loot.inventory == ScriptNpcDeathInventory::Equipment
                && loot.slot == 2
                && loot.item.damage == Some(3))
    );
    for loot in &receipt.loot {
        assert!(
            state
                .entities
                .slot_of_uuid(uuid::Uuid::parse_str(&loot.entity_uuid).unwrap())
                .is_some()
        );
    }
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .iter()
            .all(Option::is_none)
    );
    assert!(
        state
            .entities
            .row(slot)
            .appearance
            .equipment
            .iter()
            .all(Option::is_none)
    );
    assert!(
        crate::plugin_inventories::native_npc_inventory_revision(&state, "owner", &npc.handle)
            .unwrap()
            > source
    );
    let receipt_ref = receipt.receipt_ref.clone();
    let entities = state.entities.len();
    let spawned = state.plugin_spawned.len();
    state.tick += 20;
    record_death(&mut state, uuid);
    assert_eq!(state.entities.len(), entities);
    assert_eq!(state.plugin_spawned.len(), spawned);
    assert_eq!(
        records(&state, "owner").unwrap()[&npc.handle]
            .controls
            .death_cargo
            .as_ref()
            .unwrap()
            .receipt_ref,
        receipt_ref
    );
    validate(&state).unwrap();
}

#[test]
fn route_handoff_keeps_one_physical_body_and_stale_geometry_stops_without_arrival() {
    let mut state = fixture();
    let npc = spawn(&mut state, "owner", "handoff-worker", 2.5);
    let initial = route(&mut state, &npc);
    let started = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::NpcRoute {
            operation: ScriptNpcRouteOperation::Start {
                operation_id: "start-handoff".into(),
                route_ref: initial.route_ref.clone(),
                expected_epoch: initial.executor_epoch,
                cargo_operation_id: None,
            },
        }),
    );
    assert_eq!(started.failure(), None);
    let handoff = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::NpcRoute {
            operation: ScriptNpcRouteOperation::Handoff {
                operation_id: "handoff".into(),
                route_ref: initial.route_ref.clone(),
                expected_epoch: initial.executor_epoch,
                mode: ScriptNpcExecutorMode::RemotePhysical,
            },
        }),
    );
    assert_eq!(handoff.failure(), None);
    let ScriptOperationPayload::NpcRoute { result } = handoff.payload() else {
        panic!("route");
    };
    let ScriptNpcRouteResult::Status { route: handed } = &**result else {
        panic!("status");
    };
    assert_eq!(handed.executor_epoch, initial.executor_epoch + 1);
    assert_eq!(handed.pose.as_ref().unwrap().x, 2.5);
    assert_eq!(state.entities.len(), 1);
    let old_permit =
        ScriptNpcExecutionPermit::new(initial.route_ref.clone(), initial.executor_epoch);
    assert_eq!(
        validate_permit(&state, "owner", &npc.handle, Some(&old_permit)),
        Err(ScriptOperationFailure::StaleRevision)
    );
    state.world.set_block(4, 64, 2, state.registry.states.stone);
    state.tick += 1;
    advance(&mut state);
    let query = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::NpcRoute {
            operation: ScriptNpcRouteOperation::Query {
                route_ref: initial.route_ref,
            },
        }),
    );
    let ScriptOperationPayload::NpcRoute { result } = query.payload() else {
        panic!("route");
    };
    let ScriptNpcRouteResult::Status { route: blocked } = &**result else {
        panic!("status");
    };
    assert_eq!(blocked.state, ScriptNpcRouteState::Blocked);
    assert_eq!(
        blocked.blocked_reason,
        Some(ScriptNpcRouteBlockReason::StaleGeometry)
    );
    assert!(blocked.arrival.is_none());
    assert_eq!(blocked.cursor, 0);
    assert_eq!(blocked.pose.as_ref().unwrap().x, 2.5);
    let slot = owned_slot(&state, "owner", &npc.handle).unwrap();
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .navigation_path
            .is_empty()
    );
    validate(&state).unwrap();
}

#[test]
fn long_native_route_segment_uses_exact_native_search_not_poi_neighbour_success() {
    let mut state = fixture();
    let npc = spawn(&mut state, "owner", "long-worker", 2.5);
    let chunks = (0..=8)
        .map(|x| (crate::boundary::ChunkPos { x, z: 0 }, 0))
        .collect();
    state.world.ensure_blocking(&chunks);
    for x in 32..144 {
        for z in 0..8 {
            state.world.set_block(x, 63, z, state.registry.states.stone);
        }
    }
    let result = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::NpcRoute {
            operation: ScriptNpcRouteOperation::Admit {
                operation_id: "long-admit".into(),
                admission: ScriptNpcRouteAdmission::new(
                    npc.handle.clone(),
                    npc.revision,
                    0,
                    "minecraft:overworld".into(),
                    vec![ScriptBlockPosition::new(110, 64, 2)],
                    2000,
                    Vec::new(),
                ),
            },
        }),
    );
    assert_eq!(result.failure(), None);
    assert_eq!(state.entities.len(), 1);
    validate(&state).unwrap();
}

#[test]
fn custody_index_and_future_birth_cursor_refuse_instead_of_fabricating_empty_results() {
    let mut state = fixture();
    let a = spawn(&mut state, "owner", "cursor-a", 2.5);
    let b = spawn(&mut state, "owner", "cursor-b", 3.5);
    let mut parents = vec![a.handle.clone(), b.handle];
    parents.sort();
    let future = (state.tick + 1) * 65_536;
    let result = execute(
        &mut state,
        "owner",
        &request(ScriptOperation::Npc {
            operation: ScriptNpcOperation::QueryBirths {
                parent_handles: parents,
                cursor: Some(future),
                limit: 4,
            },
        }),
    );
    assert_eq!(
        result.failure(),
        Some(ScriptOperationFailure::InvalidRequest)
    );
    state
        .plugins
        .set_domain(NATIVE, "npc/custody", serde_json::json!({}));
    assert!(validate(&state).is_err());
}

struct WorkEmpty {
    air: u32,
    dimension: crate::dimensions::Dimension,
}
impl mc_world::ChunkGenerator for WorkEmpty {
    fn generate(&self, position: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty_with_geometry(
            position,
            mc_world::BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
            self.dimension.geometry(),
        )
    }
}

struct PhysicalWorkFixture {
    state: TickState,
    boundary: mc_script::ScriptBoundary,
    host: mc_script::ScriptHostEndpoint,
    npc: ScriptNpcSnapshot,
    slot: EntitySlot,
}
impl PhysicalWorkFixture {
    fn new(kind: &str, hooked: bool, dimension: crate::dimensions::Dimension) -> Self {
        use mc_script::precommit::{HookFailurePolicy, HookKind, HookRegistration};
        let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
        let world = crate::world::ChunkWorld::new(
            42,
            Arc::new(WorkEmpty {
                air: registry.states.air,
                dimension,
            }),
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [0, 64, 0],
            dimension.geometry(),
        );
        let mut state = TickState::new(registry, world, 2, 512);
        state.dimension = dimension;
        let chunks = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| (crate::boundary::ChunkPos { x, z }, 0)))
            .collect();
        state.world.ensure_blocking(&chunks);
        state.tickets.insert(
            "fixture".into(),
            (crate::boundary::ChunkPos { x: 0, z: 0 }, 1),
        );
        state.refresh_active_chunks(state.tick + 1);
        for x in -16..32 {
            for z in -16..32 {
                state.world.set_block(x, 63, z, state.registry.states.stone);
            }
        }
        let (boundary, host) = mc_script::script_boundary_pair(
            std::num::NonZeroUsize::new(128).unwrap(),
            std::num::NonZeroUsize::new(128).unwrap(),
        );
        let manifest = ScriptPluginManifest::new(
            "owner",
            "Physical worker",
            "1",
            COMPONENT_PLUGIN_API_VERSION,
        )
        .declare_npc_work()
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
        let npc = spawn(&mut state, "owner", "physical-worker", 2.5);
        let slot = owned_slot(&state, "owner", &npc.handle).unwrap();
        let block_state = |name: &str, props: &[(&str, &str)]| {
            let id = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
            let props = props
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect::<Vec<_>>();
            state
                .registry
                .blocks
                .by_name_and_props(&id, &props)
                .unwrap()
                .0
        };
        let area = ScriptWorkArea::new(
            dimension.name().into(),
            ScriptBlockPosition::new(4, 64, 2),
            ScriptBlockPosition::new(4, 64, 2),
        );
        let (work, tool, target, floor) = match kind {
            "mine" => (
                ScriptNpcWorkOrder::Mine {
                    area,
                    tool: "minecraft:iron_pickaxe".into(),
                },
                "minecraft:iron_pickaxe",
                state.registry.states.stone,
                state.registry.states.stone,
            ),
            "cut" => (
                ScriptNpcWorkOrder::CutTree {
                    area,
                    tool: "minecraft:iron_axe".into(),
                },
                "minecraft:iron_axe",
                block_state("oak_log", &[("axis", "y")]),
                state.registry.states.stone,
            ),
            "harvest" => (
                ScriptNpcWorkOrder::Harvest {
                    area,
                    tool: "minecraft:iron_hoe".into(),
                },
                "minecraft:iron_hoe",
                block_state("wheat", &[("age", "7")]),
                block_state("farmland", &[("moisture", "7")]),
            ),
            "replant" => (
                ScriptNpcWorkOrder::Replant {
                    area,
                    seed: "minecraft:wheat_seeds".into(),
                    tool: "minecraft:iron_hoe".into(),
                },
                "minecraft:iron_hoe",
                state.registry.states.air,
                block_state("farmland", &[("moisture", "7")]),
            ),
            _ => panic!("unknown physical work fixture"),
        };
        state.world.set_block(4, 63, 2, floor);
        state.world.set_block(4, 64, 2, target);
        let item = |name: &str, count| {
            let id = state
                .registry
                .items
                .id_of(&mc_data::Identifier::parse(name).unwrap())
                .unwrap();
            mc_data::ItemStack::new(id, count)
        };
        let tool_stack = item(tool, 1);
        let seeds = item("minecraft:wheat_seeds", 2);
        let inventory = &mut state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .inventory;
        inventory[0] = Some(tool_stack);
        inventory[1] = Some(seeds);
        let assigned = execute(
            &mut state,
            "owner",
            &request(ScriptOperation::NpcOrder {
                operation: ScriptNpcOrderOperation::AssignWork {
                    operation_id: "physical-work".into(),
                    handle: npc.handle.clone(),
                    work: work.clone(),
                    work_units: 2,
                    expected_revision: 0,
                },
            }),
        );
        assert_eq!(assigned.failure(), None, "{assigned:?}");
        let assignment = records(&state, "owner").unwrap()[&npc.handle]
            .controls
            .work
            .clone()
            .unwrap();
        assert!(assignment.reason.is_none(), "{kind}: {assignment:?}");
        state.tick = assignment.due_tick.unwrap();
        Self {
            state,
            boundary,
            host,
            npc,
            slot,
        }
    }

    fn inventory(&self) -> [Option<mc_data::ItemStack>; 8] {
        self.state
            .entities
            .row(self.slot)
            .villager
            .as_ref()
            .unwrap()
            .inventory
            .clone()
    }
    fn assignment(&self) -> ScriptWorkAssignment {
        records(&self.state, "owner").unwrap()[&self.npc.handle]
            .controls
            .work
            .clone()
            .unwrap()
    }
    fn question(&mut self) -> mc_script::precommit::Request {
        match self.host.recv_input_blocking() {
            Some(mc_script::ScriptHostInput::Precommit(request)) => request,
            _ => panic!("physical NPC work must ask a before-build question"),
        }
    }
}

fn physical_stock(
    state: &TickState,
    inventory: &[Option<mc_data::ItemStack>; 8],
) -> BTreeMap<String, i64> {
    let mut stock = BTreeMap::new();
    for stack in inventory.iter().flatten().filter(|stack| !stack.is_empty()) {
        let item = crate::plugin_inventories::to_script_item(state, stack).unwrap();
        *stock.entry(item.resource_id).or_default() += i64::from(stack.count);
    }
    stock
}

#[test]
fn every_physical_npc_work_kind_freezes_loot_and_debits_only_after_keep() {
    use mc_script::precommit::{HookActor, HookContext, HookDecision};
    for kind in ["mine", "cut", "harvest", "replant"] {
        let mut fixture =
            PhysicalWorkFixture::new(kind, true, crate::dimensions::Dimension::Overworld);
        let original = fixture.inventory();
        let previous = fixture.state.world.block_at(4, 64, 2).unwrap();
        let mut expected = physical_stock(&fixture.state, &original);
        if kind == "replant" {
            *expected.get_mut("minecraft:wheat_seeds").unwrap() -= 1;
        } else {
            let drops = crate::block_drop_context::roll(
                &fixture.state,
                crate::boundary::BlockPos { x: 4, y: 64, z: 2 },
                previous,
                original[0].as_ref().unwrap(),
                fixture.state.tick,
            )
            .unwrap();
            for drop in drops {
                let item =
                    crate::plugin_inventories::to_script_item(&fixture.state, &drop).unwrap();
                *expected.entry(item.resource_id).or_default() += i64::from(drop.count);
            }
        }
        advance(&mut fixture.state);
        assert_eq!(
            fixture.inventory(),
            original,
            "{kind}: pending must preserve tool/materials"
        );
        assert_eq!(fixture.assignment().work_units_done, 0, "{kind}");
        assert_eq!(
            records(&fixture.state, "owner").unwrap()[&fixture.npc.handle].work_cursor,
            0
        );
        assert!(fixture.state.plugin_block_writes.is_empty());
        let question = fixture.question();
        let HookContext::Build(context) = question.context() else {
            unreachable!()
        };
        assert_eq!(context.actor(), &HookActor::Plugin("owner".into()));
        assert_eq!(context.edits().len(), 1);
        assert_eq!(context.edits()[0].previous_state, previous);
        let proposed = context.edits()[0].proposed_state;
        advance(&mut fixture.state);
        assert_eq!(
            fixture.boundary.event_queue_depth(),
            0,
            "{kind}: no repeated questions"
        );
        fixture.state.tick += 5;
        question.answer(HookDecision::Keep, Vec::new()).unwrap();
        advance(&mut fixture.state);
        assert_eq!(
            fixture.state.world.block_at(4, 64, 2),
            Some(proposed),
            "{kind}"
        );
        assert_eq!(fixture.assignment().work_units_done, 1, "{kind}");
        assert_eq!(
            fixture.inventory()[0].as_ref().unwrap().damage,
            Some(1),
            "{kind}"
        );
        assert_eq!(
            physical_stock(&fixture.state, &fixture.inventory()),
            expected,
            "{kind}: loot is not rerolled on the later tick"
        );
        assert_eq!(
            records(&fixture.state, "owner").unwrap()[&fixture.npc.handle].work_cursor,
            1
        );
        assert_eq!(fixture.state.plugin_block_writes.len(), 1);
        advance(&mut fixture.state);
        assert_eq!(fixture.assignment().work_units_done, 1);
        assert_eq!(fixture.state.plugin_block_writes.len(), 1);
    }
}

#[test]
fn physical_npc_work_denial_and_stale_block_inventory_cursor_fences_preserve_effects() {
    use mc_script::precommit::HookDecision;
    for kind in ["mine", "cut", "harvest", "replant"] {
        for denied in ["cancel", "block", "inventory", "cursor"] {
            let mut fixture =
                PhysicalWorkFixture::new(kind, true, crate::dimensions::Dimension::Overworld);
            advance(&mut fixture.state);
            let question = fixture.question();
            match denied {
                "block" => {
                    fixture
                        .state
                        .world
                        .set_block(4, 64, 2, fixture.state.registry.states.dirt);
                }
                "inventory" => {
                    fixture
                        .state
                        .entities
                        .row_mut(fixture.slot)
                        .villager
                        .as_mut()
                        .unwrap()
                        .inventory[1]
                        .as_mut()
                        .unwrap()
                        .count += 1;
                }
                "cursor" => {
                    let mut all = records(&fixture.state, "owner").unwrap();
                    all.get_mut(&fixture.npc.handle).unwrap().work_cursor = 1;
                    save_record(&mut fixture.state, "owner", &all[&fixture.npc.handle]);
                }
                _ => {}
            }
            let inventory = fixture.inventory();
            let block = fixture.state.world.block_at(4, 64, 2);
            question
                .answer(
                    if denied == "cancel" {
                        HookDecision::Cancel
                    } else {
                        HookDecision::Keep
                    },
                    Vec::new(),
                )
                .unwrap();
            advance(&mut fixture.state);
            assert_eq!(fixture.inventory(), inventory, "{kind}/{denied}");
            assert_eq!(
                fixture.state.world.block_at(4, 64, 2),
                block,
                "{kind}/{denied}"
            );
            assert_eq!(fixture.assignment().work_units_done, 0, "{kind}/{denied}");
            assert!(fixture.assignment().changes.is_empty());
            assert!(fixture.assignment().receipts.is_empty());
            assert!(fixture.state.plugin_block_writes.is_empty());
        }
    }
}

#[test]
fn replant_rechecks_its_native_support_before_seed_debit() {
    use mc_script::precommit::HookDecision;
    let mut fixture =
        PhysicalWorkFixture::new("replant", true, crate::dimensions::Dimension::Overworld);
    advance(&mut fixture.state);
    let question = fixture.question();
    fixture
        .state
        .world
        .set_block(4, 63, 2, fixture.state.registry.states.stone);
    let inventory = fixture.inventory();
    question.answer(HookDecision::Keep, Vec::new()).unwrap();
    advance(&mut fixture.state);
    assert_eq!(fixture.inventory(), inventory);
    assert_eq!(fixture.assignment().work_units_done, 0);
    assert!(fixture.assignment().changes.is_empty());
    assert!(fixture.state.plugin_block_writes.is_empty());
}

#[test]
fn resident_inactive_native_work_waits_for_activation() {
    let mut fixture =
        PhysicalWorkFixture::new("mine", false, crate::dimensions::Dimension::Overworld);
    let before = fixture.inventory();
    let block = fixture.state.world.block_at(4, 64, 2);
    fixture
        .state
        .world
        .set_active_chunks(Default::default(), fixture.state.tick);
    advance(&mut fixture.state);
    assert!(fixture.state.world.has_chunk(0, 0));
    assert_eq!(fixture.assignment().work_units_done, 0);
    assert_eq!(fixture.inventory(), before);
    assert_eq!(fixture.state.world.block_at(4, 64, 2), block);
    assert!(fixture.state.plugin_block_writes.is_empty());
    fixture.state.world.set_active_chunks(
        [crate::boundary::ChunkPos { x: 0, z: 0 }]
            .into_iter()
            .collect(),
        fixture.state.tick,
    );
    advance(&mut fixture.state);
    assert_eq!(fixture.assignment().work_units_done, 1);
    assert_eq!(fixture.state.plugin_block_writes.len(), 1);
}

#[test]
fn hook_free_physical_npc_work_keeps_its_native_deadline_commit_path() {
    for kind in ["mine", "cut", "harvest", "replant"] {
        let mut fixture =
            PhysicalWorkFixture::new(kind, false, crate::dimensions::Dimension::Overworld);
        advance(&mut fixture.state);
        assert_eq!(fixture.assignment().work_units_done, 1, "{kind}");
        assert_eq!(fixture.state.plugin_block_writes.len(), 1);
        assert_eq!(fixture.boundary.event_queue_depth(), 0);
    }
}

#[test]
fn root_pending_work_retirement_resolves_nether_and_end_bodies_and_real_cancellation() {
    use mc_script::precommit::{HookAnswerError, HookDecision};
    for dimension in [
        crate::dimensions::Dimension::Nether,
        crate::dimensions::Dimension::End,
    ] {
        for cancel in [false, true] {
            let mut child = PhysicalWorkFixture::new("mine", true, dimension);
            let mut root = fixture();
            advance(&mut child.state);
            let question = child.question();
            if cancel {
                let expected_revision = records(&child.state, "owner").unwrap()[&child.npc.handle]
                    .controls
                    .revision;
                let canceled = execute(
                    &mut child.state,
                    "owner",
                    &request(ScriptOperation::NpcOrder {
                        operation: ScriptNpcOrderOperation::CancelWork {
                            operation_id: "cancel-child".into(),
                            handle: child.npc.handle.clone(),
                            expected_revision,
                        },
                    }),
                );
                assert_eq!(canceled.failure(), None);
            }
            std::mem::swap(&mut root.plugins, &mut child.state.plugins);
            std::mem::swap(&mut root.plugin_runtime, &mut child.state.plugin_runtime);
            std::mem::swap(&mut root.plugin_players, &mut child.state.plugin_players);
            root.other_dimensions
                .insert(dimension, Box::new(child.state));
            crate::plugin::pump(&mut root);
            let answered = question.answer(HookDecision::Keep, Vec::new());
            if cancel {
                assert_eq!(answered, Err(HookAnswerError::Abandoned));
            } else {
                assert_eq!(
                    answered,
                    Ok(()),
                    "root must retain a live child-dimension decision"
                );
            }
            let mut world = root.other_dimensions.remove(&dimension).unwrap();
            std::mem::swap(&mut root.plugins, &mut world.plugins);
            std::mem::swap(&mut root.plugin_runtime, &mut world.plugin_runtime);
            std::mem::swap(&mut root.plugin_players, &mut world.plugin_players);
            advance(&mut world);
            let work = records(&world, "owner").unwrap()[&child.npc.handle]
                .controls
                .work
                .clone()
                .unwrap();
            assert_eq!(work.work_units_done, if cancel { 0 } else { 1 });
            assert_eq!(world.plugin_block_writes.len(), if cancel { 0 } else { 1 });
            assert!(root.plugin_block_writes.is_empty());
        }
    }
}

#[test]
fn native_birth_pages_keep_cursors_when_bodies_are_removed() {
    fn page(
        state: &mut TickState,
        handles: &[String],
        after: Option<u64>,
    ) -> (Vec<ScriptNativeBirthObservation>, Option<u64>, u64) {
        let outcome = execute(
            state,
            "owner",
            &request(ScriptOperation::Npc {
                operation: ScriptNpcOperation::QueryBirths {
                    parent_handles: handles.to_vec(),
                    cursor: after,
                    limit: 1,
                },
            }),
        );
        let ScriptOperationPayload::Npc { result } = outcome.payload() else {
            panic!("birth query failed: {outcome:?}");
        };
        let ScriptNpcResult::BirthPage {
            births,
            cursor,
            resume_cursor,
            ..
        } = &**result
        else {
            panic!("birth page");
        };
        (births.clone(), *cursor, *resume_cursor)
    }
    let mut state = fixture();
    let a = spawn(&mut state, "owner", "page-parent-a", 2.5);
    let b = spawn(&mut state, "owner", "page-parent-b", 3.5);
    let handles = vec![a.handle, b.handle];
    let parents = [a.entity_uuid, b.entity_uuid];
    let child_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let birth = |state: &mut TickState, ordinal| {
        let id = state.spawn_mob(child_type, [4.5, 64.0, 2.5], 20.0);
        let slot = state.entities.slot_of(id).unwrap();
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.birth_origin = Some(ScriptNativeBirthOrigin::new(parents.clone(), 0, state.tick));
        v.birth_sequence = Some(state.tick * 65_536 + ordinal);
        (slot, state.entities.row(slot).uuid.to_string())
    };
    state.tick = 20;
    let (first, first_uuid) = birth(&mut state, 1);
    let (second, second_uuid) = birth(&mut state, 2);
    let (births, cursor, first_resume) = page(&mut state, &handles, None);
    assert_eq!(births.len(), 1);
    assert_eq!(births[0].entity_uuid, first_uuid);
    assert_eq!(cursor, Some(first_resume));
    let (births, cursor, resume) = page(&mut state, &handles, cursor);
    assert_eq!(births[0].entity_uuid, second_uuid);
    assert!(cursor.is_none());
    assert!(resume > first_resume);
    // Remove the higher dense slot first; dense compaction must not affect keys.
    state.entities.remove(second);
    state.entities.remove(first);
    let (births, _, retained_resume) = page(&mut state, &handles, Some(resume));
    assert!(births.is_empty());
    assert_eq!(retained_resume, resume);
    state.tick += 1;
    let (_, next_uuid) = birth(&mut state, 1);
    let (births, _, next_resume) = page(&mut state, &handles, Some(resume));
    assert_eq!(births[0].entity_uuid, next_uuid);
    assert!(next_resume > resume);
    assert!(state.plugins.get_domain(NATIVE, "npc/births").is_none());
}
