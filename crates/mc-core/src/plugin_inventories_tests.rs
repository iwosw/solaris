use super::*;
use crate::boundary::{EntityId, PlayerInput};
use std::sync::Arc;

fn state() -> (TickState, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let terrain = mc_worldgen::TerrainGenerator::new(42, registry.blocks.clone());
    let spawn = terrain.locate_safe_spawn().unwrap();
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(terrain),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(7),
            uuid: uuid::Uuid::from_u128(7),
            name: "inventory-test".into(),
            permission_level: 0,
        }],
    );
    let desired = state.desired_chunks();
    state.world.ensure_blocking(&desired);
    let entity = state.connections.entity_of(ConnectionId(7)).unwrap();
    (state, entity)
}
fn player() -> Endpoint {
    Endpoint::PlayerInventory { player_id: 7 }
}
fn item(state: &TickState, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:emerald").unwrap())
            .unwrap(),
        count,
    )
}
fn query(state: &mut TickState, endpoint: &Endpoint) -> InventoryImage {
    execute_operation(
        state,
        "demo",
        &ScriptOwnedInventoryOperation::Query {
            endpoint: endpoint.clone(),
            expected_revision: None,
        },
    )
    .unwrap();
    load(state, "demo", endpoint).unwrap()
}
fn plan(count: u64) -> ScriptInventoryResourcePlan {
    ScriptInventoryResourcePlan::new(vec![ScriptInventoryWorkPortion::new(
        1,
        vec![ScriptInventoryMaterial::new(
            "minecraft:emerald".into(),
            count,
        )],
    )])
}
fn reserve(
    state: &mut TickState,
    endpoint: &Endpoint,
    plan: ScriptInventoryResourcePlan,
) -> Reservation {
    let fence = query(state, endpoint).fence;
    let (_, result) = execute_operation(
        state,
        "demo",
        &ScriptOwnedInventoryOperation::Reserve {
            operation_id: uuid::Uuid::new_v4().simple().to_string(),
            endpoint: endpoint.clone(),
            resource_plan: plan,
            expected_revision: fence,
        },
    )
    .unwrap();
    let ScriptOwnedInventoryResult::Reservation {
        reservation: snapshot,
    } = result
    else {
        panic!("reservation result")
    };
    reservation(state, "demo", &snapshot.reservation_ref).unwrap()
}
fn container(state: &mut TickState, offset: i32, kind: &str) -> Endpoint {
    let spawn = state.world.world_spawn();
    let position = ScriptBlockPosition::new(spawn[0] + offset, 200, spawn[2]);
    let id = Identifier::parse("minecraft:chest").unwrap();
    let default = state.registry.blocks.block(&id).unwrap().default;
    let mut properties = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (name, value) in &mut properties {
        if name == "type" {
            *value = kind.to_owned();
        }
        if name == "facing" {
            *value = "north".to_owned();
        }
    }
    let state_id = state
        .registry
        .blocks
        .by_name_and_props(&id, &properties)
        .unwrap();
    state
        .world
        .set_block(position.x, position.y, position.z, state_id.0);
    state.block_entities.insert(
        (position.x, position.y, position.z),
        crate::tick::BlockEntityState {
            slots: vec![ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    Endpoint::Container {
        dimension: "minecraft:overworld".into(),
        position,
    }
}
fn transfer(
    state: &mut TickState,
    source: &Endpoint,
    destination: &Endpoint,
    count: u32,
    consume: Option<(&Reservation, u64, ScriptInventoryResourcePlan)>,
) -> Result<(u64, ScriptOwnedInventoryResult), Failure> {
    let expected = vec![
        ScriptInventoryExpectedRevision::new(source.clone(), query(state, source).fence),
        ScriptInventoryExpectedRevision::new(destination.clone(), query(state, destination).fence),
    ];
    let transfers = vec![ScriptOwnedItemTransfer::new(
        source.clone(),
        9,
        destination.clone(),
        0,
        count,
    )];
    let mut operation = if let Some((reservation, portion, resource_plan)) = consume {
        ScriptOwnedInventoryOperation::ConsumeReservation {
            operation_id: "consume".into(),
            actor_id: 7,
            execution_permit: None,
            reservation_ref: reservation.snapshot.reservation_ref.clone(),
            expected_reservation_revision: reservation.revision,
            portion,
            resource_plan,
            transfers,
            expected_revisions: expected,
        }
    } else {
        ScriptOwnedInventoryOperation::Transfer {
            operation_id: "transfer".into(),
            actor_id: 7,
            execution_permit: None,
            transfers,
            expected_revisions: expected,
        }
    };
    operation.canonicalize();
    execute_operation(state, "demo", &operation)
}
fn exhaust_player_image(state: &mut TickState) {
    let key = endpoint_key(state, &player());
    let mut image = image_version(state, &key).unwrap().unwrap();
    image.revision = u64::MAX;
    state.plugins.set_domain(
        OWNER,
        &format!("image/{key}"),
        serde_json::to_value(image).unwrap(),
    );
}

#[test]
fn reserve_holds_real_stock_without_removing_or_reissuing_items() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 20);
    let before = state.players[&entity].inventory.clone();
    let first = reserve(&mut state, &player(), plan(10));
    let second = reserve(&mut state, &player(), plan(10));
    assert_eq!(state.players[&entity].inventory, before);
    assert_eq!(reserved(&state, &player())["minecraft:emerald"], 20);
    let mut candidate = before.slots.clone();
    candidate[9].count = 19;
    assert!(below_player_floors(&state, entity, &candidate));
    let (_, result) = execute_operation(
        &mut state,
        "demo",
        &ScriptOwnedInventoryOperation::Release {
            operation_id: "release".into(),
            reservation_ref: first.snapshot.reservation_ref.clone(),
            expected_revision: first.revision,
        },
    )
    .unwrap();
    let ScriptOwnedInventoryResult::Reservation {
        reservation: released,
    } = result
    else {
        panic!("release")
    };
    assert!(released.released);
    assert_eq!(released.quantities[0].returned, 10);
    assert_eq!(state.players[&entity].inventory, before);
    assert_eq!(reserved(&state, &player())["minecraft:emerald"], 10);
    assert!(!below_player_floors(&state, entity, &candidate));
    assert!(
        !reservation(&state, "demo", &second.snapshot.reservation_ref)
            .unwrap()
            .snapshot
            .released
    );
}
#[test]
fn native_transfer_preserves_components_and_advances_both_fences() {
    let (mut state, entity) = state();
    let mut source = item(&state, 2);
    source.custom_name = Some("native cargo".into());
    source.extended_mut().custom_data = Some(
        mc_data::item_stack::custom_data_from_tag(mc_nbt::Tag::Compound(vec![(
            "receipt".into(),
            mc_nbt::Tag::Int(8),
        )]))
        .unwrap(),
    );
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = source.clone();
    let target = container(&mut state, 0, "single");
    let old_source = query(&mut state, &player()).fence;
    let old_target = query(&mut state, &target).fence;
    transfer(&mut state, &player(), &target, 1, None).unwrap();
    let after = load(&state, "demo", &target).unwrap();
    let mut expected = source;
    expected.count = 1;
    assert_eq!(after.slots[&0], expected);
    assert_eq!(state.players[&entity].inventory.slots[9].count, 1);
    assert!(after.fence.revision > old_target.revision);
    assert!(load(&state, "demo", &player()).unwrap().fence.revision > old_source.revision);
    assert!(!state.outbox[&ConnectionId(7)].is_empty());
}
#[test]
fn transfer_cannot_spend_a_hold_and_failure_is_whole_transaction() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 10);
    reserve(&mut state, &player(), plan(8));
    let target = container(&mut state, 0, "single");
    let source_before = state.players[&entity].inventory.clone();
    let target_before = load(&state, "demo", &target).unwrap().slots;
    assert_eq!(
        transfer(&mut state, &player(), &target, 3, None).err(),
        Some(Failure::InsufficientItems)
    );
    assert_eq!(state.players[&entity].inventory, source_before);
    assert_eq!(load(&state, "demo", &target).unwrap().slots, target_before);
    transfer(&mut state, &player(), &target, 2, None).unwrap();
    assert_eq!(state.players[&entity].inventory.slots[9].count, 8);
}
#[test]
fn consume_exact_next_nonempty_portion_or_change_nothing() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 10);
    let plan = ScriptInventoryResourcePlan::new(vec![
        ScriptInventoryWorkPortion::new(1, Vec::new()),
        ScriptInventoryWorkPortion::new(
            2,
            vec![ScriptInventoryMaterial::new("minecraft:emerald".into(), 2)],
        ),
    ]);
    let held = reserve(&mut state, &player(), plan.clone());
    let target = container(&mut state, 0, "single");
    assert_eq!(
        transfer(
            &mut state,
            &player(),
            &target,
            1,
            Some((&held, 2, plan.clone()))
        )
        .err(),
        Some(Failure::InvalidRequest)
    );
    assert_eq!(state.players[&entity].inventory.slots[9].count, 10);
    assert_eq!(
        reservation(&state, "demo", &held.snapshot.reservation_ref)
            .unwrap()
            .snapshot,
        held.snapshot
    );
    let (_, result) = transfer(&mut state, &player(), &target, 2, Some((&held, 2, plan))).unwrap();
    let ScriptOwnedInventoryResult::ReservedTransfer {
        reservation,
        inventories,
    } = result
    else {
        panic!("reserved transfer")
    };
    assert_eq!(inventories.len(), 2);
    assert_eq!(reservation.receipt_watermark, 2);
    assert_eq!(reservation.quantities[0].consumed, 2);
    assert_eq!(reservation.quantities[0].remaining, 0);
    assert_eq!(state.players[&entity].inventory.slots[9].count, 8);
    assert_eq!(load(&state, "demo", &target).unwrap().slots[&0].count, 2);
}
#[test]
fn exact_native_before_image_refuses_stale_fence() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 10);
    let old = query(&mut state, &player()).fence;
    state.players.get_mut(&entity).unwrap().inventory.slots[9].count = 9;
    let failure = execute_operation(
        &mut state,
        "demo",
        &ScriptOwnedInventoryOperation::Query {
            endpoint: player(),
            expected_revision: Some(old.revision),
        },
    )
    .err();
    assert_eq!(failure, Some(Failure::StaleRevision));
    let current = query(&mut state, &player()).fence;
    assert_ne!(current.snapshot_hash, old.snapshot_hash);
    assert!(current.revision > old.revision);
}
#[test]
fn double_chest_aliases_share_floors_and_cannot_duplicate_transfers() {
    let (mut state, _) = state();
    let left = container(&mut state, 0, "left");
    let right = container(&mut state, 1, "right");
    let Endpoint::Container { position, .. } = &left else {
        unreachable!()
    };
    let emerald = item(&state, 10);
    state
        .block_entities
        .get_mut(&(position.x, position.y, position.z))
        .unwrap()
        .slots[0] = emerald;
    let held = reserve(&mut state, &left, plan(8));
    assert_eq!(reserved(&state, &right)["minecraft:emerald"], 8);
    let left_image = query(&mut state, &left);
    let right_image = query(&mut state, &right);
    assert_eq!(left_image.fence, right_image.fence);
    let mut operation = ScriptOwnedInventoryOperation::Transfer {
        operation_id: "alias-transfer".into(),
        actor_id: 7,
        execution_permit: None,
        transfers: vec![ScriptOwnedItemTransfer::new(
            left.clone(),
            27,
            right.clone(),
            0,
            1,
        )],
        expected_revisions: vec![
            ScriptInventoryExpectedRevision::new(left.clone(), left_image.fence),
            ScriptInventoryExpectedRevision::new(right.clone(), right_image.fence),
        ],
    };
    operation.canonicalize();
    assert_eq!(
        execute_operation(&mut state, "demo", &operation).err(),
        Some(Failure::InvalidRequest)
    );
    assert_eq!(load(&state, "demo", &left).unwrap().slots, left_image.slots);
    invalidate_block(
        &mut state,
        BlockPos {
            x: position.x + 1,
            y: position.y,
            z: position.z,
        },
    );
    let released = reservation(&state, "demo", &held.snapshot.reservation_ref).unwrap();
    assert!(released.snapshot.released);
    assert_eq!(released.snapshot.quantities[0].returned, 8);
}

#[test]
fn single_chest_hold_follows_joined_custody_and_new_half_invalidation() {
    let (mut state, _) = state();
    let single = container(&mut state, 0, "single");
    let Endpoint::Container { position, .. } = &single else {
        unreachable!()
    };
    let original = BlockPos {
        x: position.x,
        y: position.y,
        z: position.z,
    };
    let emeralds = item(&state, 8);
    state
        .block_entities
        .get_mut(&(original.x, original.y, original.z))
        .unwrap()
        .slots[0] = emeralds;
    let held = reserve(&mut state, &single, plan(6));
    assert_eq!(
        held.source_cells,
        vec![[original.x, original.y, original.z]]
    );

    container(&mut state, 0, "left");
    let right = container(&mut state, 1, "right");
    let Endpoint::Container { position, .. } = &right else {
        unreachable!()
    };
    let joined = BlockPos {
        x: position.x,
        y: position.y,
        z: position.z,
    };
    let emeralds = item(&state, 8);
    state
        .block_entities
        .get_mut(&(joined.x, joined.y, joined.z))
        .unwrap()
        .slots[0] = emeralds;
    assert_eq!(reserved(&state, &right)["minecraft:emerald"], 6);
    assert!(has_reservations_for_block(&state, joined));
    assert!(!below_block_floors(
        &state,
        original,
        &[ItemStack::EMPTY; 27]
    ));
    assert!(!below_block_floors(&state, joined, &[item(&state, 6)]));
    assert!(below_block_floors(&state, joined, &[item(&state, 5)]));
    assert!(below_block_floors(&state, joined, &[ItemStack::EMPTY; 27]));

    invalidate_block(&mut state, joined);
    let released = reservation(&state, "demo", &held.snapshot.reservation_ref).unwrap();
    assert!(released.snapshot.released);
    assert_eq!(released.snapshot.quantities[0].returned, 6);
    assert!(!has_reservations_for_block(&state, original));
    assert!(!has_reservations_for_block(&state, joined));
    assert!(!below_block_floors(&state, joined, &[ItemStack::EMPTY; 27]));
    assert_eq!(
        state.block_entities[&(joined.x, joined.y, joined.z)].slots[0].count,
        8
    );
}

#[test]
fn joined_chest_admission_fails_closed_without_partner_storage() {
    let (mut state, _) = state();
    let single = container(&mut state, 0, "single");
    let Endpoint::Container { position, .. } = &single else {
        unreachable!()
    };
    let original = BlockPos {
        x: position.x,
        y: position.y,
        z: position.z,
    };
    let emeralds = item(&state, 8);
    state
        .block_entities
        .get_mut(&(original.x, original.y, original.z))
        .unwrap()
        .slots[0] = emeralds;
    let held = reserve(&mut state, &single, plan(6));
    container(&mut state, 0, "left");
    let right = container(&mut state, 1, "right");
    let Endpoint::Container { position, .. } = right else {
        unreachable!()
    };
    let joined = BlockPos {
        x: position.x,
        y: position.y,
        z: position.z,
    };
    state.block_entities.remove(&(joined.x, joined.y, joined.z));
    assert!(has_reservations_for_block(&state, joined));
    assert!(below_block_floors(&state, original, &[item(&state, 8)]));
    invalidate_block(&mut state, joined);
    assert!(
        reservation(&state, "demo", &held.snapshot.reservation_ref)
            .unwrap()
            .snapshot
            .released
    );
}
#[test]
fn placement_query_includes_zero_room_for_source_slot() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 4);
    let image = query(&mut state, &player());
    let expected = ScriptInventoryExpectedRevision::new(player(), image.fence);
    let (_, result) = execute_operation(
        &mut state,
        "demo",
        &ScriptOwnedInventoryOperation::PlacementRoom {
            source: expected.clone(),
            source_slot: 9,
            destination: expected,
        },
    )
    .unwrap();
    let ScriptOwnedInventoryResult::PlacementRoom { slots, .. } = result else {
        panic!("placement room")
    };
    assert_eq!(slots.len(), 36);
    assert_eq!(slots.iter().find(|s| s.slot == 9).unwrap().room, 0);
    assert!(slots.iter().any(|s| s.slot != 9 && s.room > 0));
}
#[test]
fn corrupt_accounting_fails_native_startup_validation() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 8);
    let held = reserve(&mut state, &player(), plan(8));
    validate(&state).unwrap();
    let mut corrupt = held;
    corrupt.snapshot.quantities[0].remaining = 9;
    save_reservation(&mut state, corrupt).unwrap();
    assert!(validate(&state).is_err());
}
#[test]
fn player_hold_remaps_by_uuid_after_connection_replacement() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 8);
    let held = reserve(&mut state, &player(), plan(8));
    state.connections.remove_by_connection(ConnectionId(7));
    state.connections.insert(
        ConnectionId(70),
        entity,
        uuid::Uuid::from_u128(7),
        "rejoined".into(),
    );
    state.players.get_mut(&entity).unwrap().connection = ConnectionId(70);
    let loaded = reservation(&state, "demo", &held.snapshot.reservation_ref).unwrap();
    assert_eq!(
        loaded.snapshot.endpoint,
        Endpoint::PlayerInventory { player_id: 70 }
    );
    assert_eq!(
        reserved(&state, &loaded.snapshot.endpoint)["minecraft:emerald"],
        8
    );
    assert!(has_reservations_for_player(&state, entity));
}

#[test]
fn sparse_player_costs_preserve_floors_without_candidate_stack_images() {
    let (mut state, entity) = state();
    let emeralds = item(&state, 8);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = emeralds;
    let offhand = item(&state, 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[40] = offhand;
    reserve(&mut state, &player(), plan(6));
    assert!(!below_player_debit_floors(&state, entity, &[(9, 2)], None));
    assert!(below_player_debit_floors(&state, entity, &[(9, 3)], None));
    assert!(!below_player_debit_floors(
        &state,
        entity,
        &[(9, 1), (9, 1)],
        None,
    ));
    assert!(below_player_debit_floors(
        &state,
        entity,
        &[(9, 1), (9, 2)],
        None,
    ));
    assert!(below_player_debit_floors(&state, entity, &[], Some(9)));
    assert!(!below_player_debit_floors(&state, entity, &[], Some(40)));
    assert!(!below_player_debit_floors(&state, entity, &[(40, 1)], None));
    assert!(below_player_debit_floors(&state, entity, &[(10, 1)], None));
    assert!(below_player_debit_floors(&state, entity, &[(41, 1)], None));
    assert!(below_player_debit_floors(&state, entity, &[], Some(41)));
    assert_eq!(state.players[&entity].inventory.slots[9].count, 8);
    assert_eq!(reserved(&state, &player())["minecraft:emerald"], 6);
}

#[test]
fn sparse_cost_preflight_fails_closed_on_capacity_and_malformed_metadata() {
    let (mut state, entity) = state();
    let emeralds = item(&state, 8);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = emeralds;
    reserve(&mut state, &player(), plan(6));
    exhaust_player_image(&mut state);
    assert!(below_player_debit_floors(&state, entity, &[(9, 1)], None));
    state
        .plugins
        .set_domain(OWNER, KEY, serde_json::Value::Null);
    assert!(below_player_debit_floors(&state, entity, &[(9, 1)], None));
    assert_eq!(state.players[&entity].inventory.slots[9].count, 8);
}

#[test]
fn inactive_physical_shield_cooldown_does_not_make_reservation_busy() {
    let (mut state, entity) = state();
    let emeralds = item(&state, 8);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = emeralds;
    let slot = state.entities.slot_of(entity).unwrap();
    let key = format!("defense/{}", state.entities.row(slot).uuid);
    state.plugins.set_domain(
        "__native_physical",
        &key,
        serde_json::json!({ "active": null, "disabled_until": state.tick + 20 }),
    );
    let control = crate::physical_defense::item_use(&state, slot).unwrap();
    assert!(control.equipment_slot.is_none());
    let held = reserve(&mut state, &player(), plan(6));
    assert_eq!(held.snapshot.quantities[0].remaining, 6);
    assert_eq!(state.players[&entity].inventory.slots[9].count, 8);
}

fn claim(
    owner: EntityId,
    slot: u16,
    item: u32,
    count: i32,
    take: i32,
    put_item: Option<u32>,
    put_count: i32,
) -> crate::intent::InvClaim {
    crate::intent::InvClaim {
        owner,
        slot,
        expect_item: item,
        expect_count: count,
        take,
        put_item,
        put_count,
    }
}

#[test]
fn intent_floors_project_debits_replacement_and_reordering_exactly() {
    let (mut state, entity) = state();
    let emeralds = item(&state, 8);
    let id = emeralds.item_id;
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = emeralds;
    let offhand = item(&state, 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[40] = offhand;
    reserve(&mut state, &player(), plan(6));
    assert!(below_intent_floors(
        &state,
        &[claim(entity, 9, id, 8, 3, None, 0)]
    ));
    assert!(!below_intent_floors(
        &state,
        &[claim(entity, 9, id, 8, 3, Some(id), 6)]
    ));
    // Replacement of the five-item remainder by one item is not 5 + 1.
    assert!(below_intent_floors(
        &state,
        &[claim(entity, 9, id, 8, 3, Some(id), 1)]
    ));
    assert!(!below_intent_floors(
        &state,
        &[
            claim(entity, 9, id, 8, 8, None, 0),
            claim(entity, 10, 0, 0, 0, Some(id), 6),
        ]
    ));
    assert!(below_intent_floors(
        &state,
        &[
            claim(entity, 9, id, 8, 1, None, 0),
            claim(entity, 9, id, 8, 2, None, 0),
        ]
    ));
    assert!(!below_intent_floors(
        &state,
        &[
            claim(entity, 9, id, 8, 1, None, 0),
            claim(entity, 9, id, 8, 2, Some(id), 6),
        ]
    ));
    assert!(!below_intent_floors(
        &state,
        &[claim(entity, 40, id, 1, 1, None, 0)]
    ));
    assert_eq!(state.players[&entity].inventory.slots[9].count, 8);
    assert!(state.players[&entity].inventory.slots[10].is_empty());
}

#[test]
fn intent_floor_projection_checks_every_owner_independently() {
    let (mut state, first) = state();
    let emeralds = item(&state, 8);
    let id = emeralds.item_id;
    state.players.get_mut(&first).unwrap().inventory.slots[9] = emeralds;
    reserve(&mut state, &player(), plan(6));
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(8),
            uuid: uuid::Uuid::from_u128(8),
            name: "second-holder".into(),
            permission_level: 0,
        }],
    );
    let second = state.connections.entity_of(ConnectionId(8)).unwrap();
    let emeralds = item(&state, 5);
    state.players.get_mut(&second).unwrap().inventory.slots[9] = emeralds;
    reserve(
        &mut state,
        &Endpoint::PlayerInventory { player_id: 8 },
        plan(3),
    );
    assert!(below_intent_floors(
        &state,
        &[
            claim(first, 9, id, 8, 2, None, 0),
            claim(second, 9, id, 5, 3, None, 0),
        ]
    ));
    assert!(!below_intent_floors(
        &state,
        &[
            claim(first, 9, id, 8, 2, None, 0),
            claim(second, 9, id, 5, 2, None, 0),
        ]
    ));
    assert_eq!(state.players[&first].inventory.slots[9].count, 8);
    assert_eq!(state.players[&second].inventory.slots[9].count, 5);
}

#[test]
fn intent_floor_projection_fails_closed_on_source_capacity_and_decode() {
    let (mut state, entity) = state();
    let emeralds = item(&state, 8);
    let id = emeralds.item_id;
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = emeralds;
    reserve(&mut state, &player(), plan(6));
    exhaust_player_image(&mut state);
    let debit = claim(entity, 9, id, 8, 1, None, 0);
    assert!(below_intent_floors(&state, &[debit]));
    state
        .plugins
        .set_domain(OWNER, KEY, serde_json::Value::Null);
    assert!(below_intent_floors(&state, &[debit]));
    assert_eq!(state.players[&entity].inventory.slots[9].count, 8);
}

#[test]
fn borrowed_player_replacements_check_final_identity_stock_and_exclude_equipment() {
    let (mut state, entity) = state();
    let emeralds = item(&state, 8);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = emeralds;
    reserve(&mut state, &player(), plan(6));
    let six = item(&state, 6);
    let five = item(&state, 5);
    let empty = ItemStack::EMPTY;
    assert!(!below_player_replacement_floors(
        &state,
        entity,
        &[(9, &six)]
    ));
    assert!(below_player_replacement_floors(
        &state,
        entity,
        &[(9, &five)]
    ));
    assert!(!below_player_replacement_floors(
        &state,
        entity,
        &[(9, &empty), (10, &six)],
    ));
    assert!(below_player_replacement_floors(
        &state,
        entity,
        &[(9, &empty), (40, &six)],
    ));
    assert!(!below_player_replacement_floors(
        &state,
        entity,
        &[(36, &six)]
    ));
    assert!(!below_player_replacement_floors(
        &state,
        entity,
        &[(9, &five), (9, &six)],
    ));
    assert!(below_player_replacement_floors(
        &state,
        entity,
        &[(9, &six), (9, &five)],
    ));
    let stone = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap(),
        8,
    );
    assert!(below_player_replacement_floors(
        &state,
        entity,
        &[(9, &stone)]
    ));
    assert!(below_player_replacement_floors(
        &state,
        entity,
        &[(41, &six)]
    ));
    assert_eq!(state.players[&entity].inventory.slots[9].count, 8);
    assert!(state.players[&entity].inventory.slots[10].is_empty());
}

#[test]
fn replacement_floor_projection_fails_closed_on_capacity_and_decode() {
    let (mut state, entity) = state();
    let emeralds = item(&state, 8);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = emeralds;
    reserve(&mut state, &player(), plan(6));
    let seven = item(&state, 7);
    exhaust_player_image(&mut state);
    assert!(below_player_replacement_floors(
        &state,
        entity,
        &[(9, &seven)]
    ));
    state
        .plugins
        .set_domain(OWNER, KEY, serde_json::Value::Null);
    assert!(below_player_replacement_floors(
        &state,
        entity,
        &[(9, &seven)]
    ));
    assert_eq!(state.players[&entity].inventory.slots[9].count, 8);
}

fn release_hold(
    state: &mut TickState,
    held: &Reservation,
    operation_id: &str,
) -> Result<(u64, ScriptOwnedInventoryResult), Failure> {
    execute_operation(
        state,
        "demo",
        &ScriptOwnedInventoryOperation::Release {
            operation_id: operation_id.into(),
            reservation_ref: held.snapshot.reservation_ref.clone(),
            expected_revision: held.revision,
        },
    )
}

fn journal_delta(state: &mut TickState) -> crate::plugin::PluginDelta {
    let mut entries = Vec::new();
    crate::plugin::journal(state, &mut entries);
    let crate::journal::JournalEntry::Plugins { value } = entries.pop().expect("plugin delta")
    else {
        panic!("plugin journal entry");
    };
    assert!(entries.is_empty());
    *value
}

#[test]
fn no_plugin_custody_returns_before_resolving_a_dimension_endpoint() {
    let (mut state, entity) = state();
    assert!(state.plugins.domain_entries(OWNER).next().is_none());
    let unloaded = BlockPos {
        x: i32::MAX,
        y: i32::MAX,
        z: i32::MAX,
    };
    assert!(!has_reservations_for_block(&state, unloaded));
    assert!(!below_block_floors(&state, unloaded, &[]));
    assert!(!has_reservations_for_player(&state, entity));
    assert!(state.plugins.domain_entries(OWNER).next().is_none());
    // A different native domain is not inventory custody.
    state
        .plugins
        .set_domain("@unrelated-native", "record", serde_json::Value::Null);
    assert!(!has_reservations_for_block(&state, unloaded));
    assert!(!below_block_floors(&state, unloaded, &[]));
}

#[test]
fn custody_reads_only_the_requested_sources_and_images() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 8);
    reserve(&mut state, &player(), plan(6));
    let unrelated = container(&mut state, 4, "single");
    let Endpoint::Container { position, .. } = &unrelated else {
        unreachable!()
    };
    let pos = BlockPos {
        x: position.x,
        y: position.y,
        z: position.z,
    };
    let emeralds = item(&state, 8);
    state
        .block_entities
        .get_mut(&(pos.x, pos.y, pos.z))
        .unwrap()
        .slots[0] = emeralds;
    let unrelated_hold = reserve(&mut state, &unrelated, plan(6));
    state.plugins.set_domain(
        OWNER,
        &format!("reservation/{}", unrelated_hold.snapshot.reservation_ref),
        serde_json::Value::Null,
    );
    for key in image_keys(&state, &unrelated) {
        state
            .plugins
            .set_domain(OWNER, &format!("image/{key}"), serde_json::Value::Null);
    }
    assert_eq!(reserved(&state, &player())["minecraft:emerald"], 6);
    assert!(has_reservations_for_player(&state, entity));
    assert!(!below_player_debit_floors(&state, entity, &[(9, 2)], None));
    assert!(below_player_debit_floors(&state, entity, &[(9, 3)], None));
    assert!(has_reservations_for_block(&state, pos));
    assert!(below_block_floors(&state, pos, &[item(&state, 8)]));
    assert!(validate(&state).is_err());
    let destination = container(&mut state, 8, "single");
    transfer(&mut state, &player(), &destination, 2, None).unwrap();
    assert_eq!(state.players[&entity].inventory.slots[9].count, 6);
    assert_eq!(
        load(&state, "demo", &destination).unwrap().slots[&0].count,
        2
    );
}

#[test]
fn reservation_mutations_journal_only_changed_records_and_index_tombstones() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 12);
    let mut first = reserve(&mut state, &player(), plan(4));
    let second = reserve(&mut state, &player(), plan(4));
    journal_delta(&mut state);
    let mut replayed = state.plugins.clone();
    first.snapshot.bound_to = Some("structure-1".into());
    first.revision += 1;
    save_reservation(&mut state, first.clone()).unwrap();
    let delta = journal_delta(&mut state);
    let json = serde_json::to_value(&delta).unwrap();
    let changes = json["domains"][OWNER].as_object().unwrap();
    assert_eq!(changes.len(), 1);
    assert!(changes.contains_key(&format!("reservation/{}", first.snapshot.reservation_ref)));
    replayed.apply_delta(&delta);
    replayed.apply_delta(&delta);
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&state.plugins).unwrap()
    );

    let expected: BTreeSet<_> = index_keys(&first)
        .into_iter()
        .chain(index_keys(&second))
        .chain([
            format!("reservation/{}", first.snapshot.reservation_ref),
            format!("reservation/{}", second.snapshot.reservation_ref),
            KEY.into(),
            "retired/0".into(),
            "retired/1".into(),
        ])
        .collect();
    invalidate_player(&mut state, entity);
    let delta = journal_delta(&mut state);
    let json = serde_json::to_value(&delta).unwrap();
    let changes = json["domains"][OWNER].as_object().unwrap();
    assert_eq!(changes.keys().cloned().collect::<BTreeSet<_>>(), expected);
    for key in index_keys(&first).into_iter().chain(index_keys(&second)) {
        assert_eq!(changes[&key], serde_json::json!("Removed"));
    }
    assert!(
        !changes
            .keys()
            .any(|key| key.starts_with("image/") || key == "ledger")
    );
    replayed.apply_delta(&delta);
    replayed.apply_delta(&delta);
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&state.plugins).unwrap()
    );
    let released = reservation(&state, "demo", &first.snapshot.reservation_ref).unwrap();
    assert!(released.snapshot.released);
    assert!(released.snapshot.bound_to.is_none());
    assert_eq!(released.snapshot.quantities[0].returned, 4);
    assert!(!has_reservations_for_player(&state, entity));
    validate(&state).unwrap();
}

#[test]
fn terminal_ring_prunes_released_and_consumed_records_across_restore_and_replay() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 10);
    let live = reserve(&mut state, &player(), plan(1));
    let pinned = reserve(&mut state, &player(), plan(1));
    commit_reserved_debit(
        &mut state,
        "demo",
        pinned.clone(),
        &BTreeMap::from([("minecraft:emerald".into(), 1)]),
        "structure-pin",
    )
    .unwrap();
    let released = reserve(&mut state, &player(), plan(1));
    let mut stale = released.clone();
    stale.revision += 1;
    let before = serde_json::to_value(&state.plugins).unwrap();
    assert_eq!(
        release_hold(&mut state, &stale, "stale").err(),
        Some(Failure::StaleRevision)
    );
    assert_eq!(serde_json::to_value(&state.plugins).unwrap(), before);
    let release_request = ScriptOperationRequest::try_new(
        "release",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Release {
                operation_id: "durable-release".into(),
                reservation_ref: released.snapshot.reservation_ref.clone(),
                expected_revision: released.revision,
            },
        },
    )
    .unwrap();
    let release_outcome = crate::plugin::operation(&mut state, "demo", &release_request);
    assert!(release_outcome.failure().is_none());
    assert_eq!(
        release_hold(&mut state, &released, "again").err(),
        Some(Failure::StaleRevision)
    );
    let consumed = reserve(&mut state, &player(), plan(1));
    let destination = container(&mut state, 4, "single");
    transfer(
        &mut state,
        &player(),
        &destination,
        1,
        Some((&consumed, 1, plan(1))),
    )
    .unwrap();
    let consumed_status = reservation(&state, "demo", &consumed.snapshot.reservation_ref).unwrap();
    assert!(!consumed_status.snapshot.released);
    assert_eq!(consumed_status.snapshot.quantities[0].remaining, 0);
    assert_eq!(consumed_status.snapshot.quantities[0].consumed, 1);
    journal_delta(&mut state);
    let checkpoint = serde_json::to_vec(&state.plugins).unwrap();
    state.plugins = serde_json::from_slice(&checkpoint).unwrap();
    validate(&state).unwrap();
    let mut replayed = state.plugins.clone();
    for index in 0..MAX_TERMINAL_RESERVATIONS {
        let held = reserve(&mut state, &player(), plan(1));
        release_hold(&mut state, &held, &format!("later-{index}")).unwrap();
    }
    let delta = journal_delta(&mut state);
    let json = serde_json::to_value(&delta).unwrap();
    for reference in [
        &released.snapshot.reservation_ref,
        &consumed.snapshot.reservation_ref,
    ] {
        assert_eq!(
            json["domains"][OWNER][format!("reservation/{reference}")],
            serde_json::json!("Removed")
        );
        assert_eq!(
            reservation(&state, "demo", reference).err(),
            Some(Failure::NotFound)
        );
    }
    replayed.apply_delta(&delta);
    replayed.apply_delta(&delta);
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&state.plugins).unwrap()
    );
    state.plugins = serde_json::from_slice(&serde_json::to_vec(&replayed).unwrap()).unwrap();
    validate(&state).unwrap();
    let meta = metadata(&state).unwrap();
    assert_eq!(meta.protected_count, 2);
    assert_eq!(meta.retired_count, MAX_TERMINAL_RESERVATIONS);
    assert_eq!(
        state.plugins.domain_prefix(OWNER, "reservation/").count(),
        2 + MAX_TERMINAL_RESERVATIONS
    );
    assert_eq!(endpoint_reservations(&state, &player()).unwrap().len(), 1);
    let pinned = reservation(&state, "demo", &pinned.snapshot.reservation_ref).unwrap();
    assert_eq!(pinned.snapshot.bound_to.as_deref(), Some("structure-pin"));
    assert_eq!(pinned.snapshot.quantities[0].remaining, 0);
    let before = serde_json::to_value(&state.plugins).unwrap();
    assert_eq!(
        crate::plugin::operation(&mut state, "demo", &release_request),
        release_outcome
    );
    assert_eq!(serde_json::to_value(&state.plugins).unwrap(), before);

    state.connections.remove_by_connection(ConnectionId(7));
    state.connections.insert(
        ConnectionId(70),
        entity,
        uuid::Uuid::from_u128(7),
        "rejoined".into(),
    );
    state.players.get_mut(&entity).unwrap().connection = ConnectionId(70);
    let loaded = reservation(&state, "demo", &live.snapshot.reservation_ref).unwrap();
    assert_eq!(
        loaded.snapshot.endpoint,
        Endpoint::PlayerInventory { player_id: 70 }
    );
    assert_eq!(
        reserved(&state, &loaded.snapshot.endpoint)["minecraft:emerald"],
        1
    );
    assert!(has_reservations_for_player(&state, entity));
    let stock = state.players[&entity].inventory.clone();
    invalidate_player(&mut state, entity);
    assert_eq!(state.players[&entity].inventory, stock);
    assert!(!has_reservations_for_player(&state, entity));
    assert_eq!(metadata(&state).unwrap().protected_count, 0);
    assert!(
        reservation(&state, "demo", &pinned.snapshot.reservation_ref)
            .unwrap()
            .snapshot
            .bound_to
            .is_none()
    );
    validate(&state).unwrap();
}

#[test]
fn protected_reservation_cap_includes_consumed_structure_pins_and_returns_busy_atomically() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 2);
    let pinned = reserve(&mut state, &player(), plan(1));
    commit_reserved_debit(
        &mut state,
        "demo",
        pinned.clone(),
        &BTreeMap::from([("minecraft:emerald".into(), 1)]),
        "structure-pin",
    )
    .unwrap();
    let pinned = reservation(&state, "demo", &pinned.snapshot.reservation_ref).unwrap();
    for index in 1..MAX_PROTECTED_RESERVATIONS {
        let mut other = pinned.clone();
        other.snapshot.reservation_ref = format!("pinned-{index}");
        other.snapshot.bound_to = Some(format!("structure-{index}"));
        save_reservation(&mut state, other).unwrap();
    }
    validate(&state).unwrap();
    assert_eq!(
        metadata(&state).unwrap().protected_count,
        MAX_PROTECTED_RESERVATIONS
    );
    assert!(!has_reservations_for_player(&state, entity));
    let fence = load(&state, "demo", &player()).unwrap().fence;
    let operation = ScriptOwnedInventoryOperation::Reserve {
        operation_id: "one-too-many".into(),
        endpoint: player(),
        resource_plan: plan(1),
        expected_revision: fence,
    };
    let before = serde_json::to_value(&state.plugins).unwrap();
    assert_eq!(
        execute_operation(&mut state, "demo", &operation).err(),
        Some(Failure::Busy)
    );
    assert_eq!(serde_json::to_value(&state.plugins).unwrap(), before);
    assert_eq!(state.players[&entity].inventory.slots[9].count, 1);
    let mut unpinned = pinned;
    unpinned.snapshot.bound_to = None;
    save_reservation(&mut state, unpinned).unwrap();
    reserve(&mut state, &player(), plan(1));
    assert_eq!(
        metadata(&state).unwrap().protected_count,
        MAX_PROTECTED_RESERVATIONS
    );
    assert_eq!(metadata(&state).unwrap().retired_count, 1);
    assert!(has_reservations_for_player(&state, entity));
    validate(&state).unwrap();
}

#[test]
fn native_restore_rejects_legacy_ledger_missing_indexes_and_broken_terminal_ring() {
    let (mut state, entity) = state();
    state.plugins.set_domain(
        OWNER,
        "ledger",
        serde_json::json!({
            "revision": 0, "images": {}, "reservations": {}
        }),
    );
    assert!(validate(&state).is_err());
    assert!(below_player_debit_floors(&state, entity, &[], None));
    state.plugins.delete_domain(OWNER, "ledger");
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 4);
    let held = reserve(&mut state, &player(), plan(2));
    let valid = state.plugins.clone();
    let key = index_keys(&held)
        .into_iter()
        .find(|key| key.starts_with("source/"))
        .unwrap();
    state.plugins.delete_domain(OWNER, &key);
    assert!(validate(&state).is_err());
    state.plugins = valid.clone();
    state.plugins.set_domain(
        OWNER,
        &key.replace(&held.snapshot.reservation_ref, "orphan"),
        serde_json::Value::Bool(true),
    );
    assert!(validate(&state).is_err());
    assert!(below_player_debit_floors(&state, entity, &[(9, 1)], None));
    state.plugins = valid;
    release_hold(&mut state, &held, "release").unwrap();
    validate(&state).unwrap();
    state.plugins.delete_domain(OWNER, "retired/0");
    assert!(validate(&state).is_err());
}

#[test]
fn corrupt_retention_victim_refuses_consumption_before_any_inventory_or_record_mutation() {
    let (mut state, entity) = state();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = item(&state, 6);
    let held = reserve(&mut state, &player(), plan(2));
    let archive = reserve(&mut state, &player(), plan(1));
    release_hold(&mut state, &archive, "release").unwrap();
    let archive = reservation(&state, "demo", &archive.snapshot.reservation_ref).unwrap();
    for index in 1..MAX_TERMINAL_RESERVATIONS {
        let mut other = archive.clone();
        other.snapshot.reservation_ref = format!("terminal-{index}");
        save_reservation(&mut state, other).unwrap();
    }
    validate(&state).unwrap();
    state
        .plugins
        .set_domain(OWNER, "retired/0", serde_json::Value::Null);
    let destination = container(&mut state, 4, "single");
    let source_fence = load(&state, "demo", &player()).unwrap().fence;
    let destination_image = load(&state, "demo", &destination).unwrap();
    let mut operation = ScriptOwnedInventoryOperation::ConsumeReservation {
        operation_id: "consume".into(),
        actor_id: 7,
        execution_permit: None,
        reservation_ref: held.snapshot.reservation_ref.clone(),
        expected_reservation_revision: held.revision,
        portion: 1,
        resource_plan: plan(2),
        transfers: vec![ScriptOwnedItemTransfer::new(
            player(),
            9,
            destination.clone(),
            0,
            2,
        )],
        expected_revisions: vec![
            ScriptInventoryExpectedRevision::new(player(), source_fence),
            ScriptInventoryExpectedRevision::new(destination.clone(), destination_image.fence),
        ],
    };
    operation.canonicalize();
    let before = serde_json::to_value(&state.plugins).unwrap();
    let inventory = state.players[&entity].inventory.clone();
    assert_eq!(
        execute_operation(&mut state, "demo", &operation).err(),
        Some(Failure::RuntimeUnavailable)
    );
    assert_eq!(serde_json::to_value(&state.plugins).unwrap(), before);
    assert_eq!(state.players[&entity].inventory, inventory);
    assert_eq!(
        load(&state, "demo", &destination).unwrap().slots,
        destination_image.slots
    );
    assert_eq!(
        reservation(&state, "demo", &held.snapshot.reservation_ref)
            .unwrap()
            .snapshot,
        held.snapshot
    );
}
