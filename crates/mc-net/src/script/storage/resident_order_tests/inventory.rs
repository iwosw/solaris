//! Resident inventory contracts: canonical endpoint hauling and harvest/mine deposit recovery.
//!
//! Moved out of `resident_order_tests.rs` by CP-REFACTOR-040 sub-slice 5.
//! Child of `resident_order_tests`: explicit imports only, no glob. The edge crop is
//! seeded by the contract that needs it via `seed_edge_crop`.
//!
//! Fixture variants used here (`mining_resident`) stay in shared support; the edge crop is

use mc_entity::Vec3;
use mc_script::ScriptResidentWorkOrder;
use mc_script::ScriptWorkPauseReason;

use super::{
    Fixture, OWNER, SURFACE_Y, area, carried_from, carry, carry_request, gear, seed_gear,
    seed_gear_in, state_of, work_of, work_request,
};
use crate::script::storage::PluginStorageMutationError;
use crate::script::storage::StorageFaultPoint;
use std::collections::BTreeMap;
/// (A08) Haul moves real items between the resident's canonical endpoints and
/// reports `missing_input` when the source is empty.
#[tokio::test]
async fn haul_moves_items_between_canonical_resident_endpoints() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    // The frozen DTO canonicalises the endpoints into enum order, so the
    // durable work is always `equipment -> carry`.
    let haul = ScriptResidentWorkOrder::Haul {
        source: mc_script::ScriptInventoryEndpoint::ResidentEquipment {
            handle: handle.clone(),
        },
        destination: mc_script::ScriptInventoryEndpoint::ResidentCarry {
            handle: handle.clone(),
        },
        item: None,
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("haul-1", &handle, haul.clone(), 4, 0),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingInput));
    assert!(assignment.changes.is_empty());

    let revision = seed_gear_in(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:oak_log", 3)],
        true,
    );
    assert_eq!(
        gear(&storage, &handle),
        vec![("minecraft:oak_log".to_owned(), 3)],
        "seed landed in the canonical slots"
    );
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("haul-2", &handle, haul, 4, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert!(assignment.work_units_done >= 3, "{assignment:?}");
    assert_eq!(assignment.reason, None);
    let changes = assignment
        .changes
        .iter()
        .map(|change| (change.item_id.as_str(), change.delta))
        .collect::<BTreeMap<_, _>>();
    let moved = u32::try_from(*changes.get("minecraft:oak_log").expect("haul delta")).unwrap();
    assert_eq!(u64::from(moved), assignment.work_units_done);
    let record = storage
        .resident_orders()
        .record(&handle)
        .expect("the moved items are durable");
    assert_eq!(record.equipment[0], None);
    assert_eq!(
        record.carry[0].as_ref().map(|stack| stack.count),
        Some(moved)
    );
}

/// (CP-003) A haul that names an item moves only that item: the stack the order
/// did not ask for stays in the source, and the receipt reports the item that
/// really moved.
#[tokio::test]
async fn a_named_item_haul_leaves_every_other_stack_where_it_is() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    // Two stacks are in the canonical source, and the order names the second
    // one, so storage order alone would move the wrong item.
    let revision = seed_gear_in(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:oak_log", 3), ("minecraft:wheat", 5)],
        true,
    );
    let haul = ScriptResidentWorkOrder::Haul {
        source: mc_script::ScriptInventoryEndpoint::ResidentEquipment {
            handle: handle.clone(),
        },
        destination: mc_script::ScriptInventoryEndpoint::ResidentCarry {
            handle: handle.clone(),
        },
        item: Some("minecraft:wheat".to_owned()),
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("haul-named", &handle, haul, 8, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(assignment.work_units_done, 5, "the whole wheat stack moved");
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<Vec<_>>(),
        vec![("minecraft:wheat", 5)],
        "the receipt names the item the order asked for"
    );
    // Read each endpoint on its own: the concatenated view cannot tell which
    // side a stack sits on.
    let record = storage
        .resident_orders()
        .record(&handle)
        .expect("the move is durable");
    assert_eq!(
        record.equipment[0]
            .as_ref()
            .map(|stack| stack.item_id.as_str()),
        Some("minecraft:oak_log"),
        "the stack the order did not ask for stayed in the source"
    );
    assert_eq!(
        record.carry[0]
            .as_ref()
            .map(|stack| (stack.item_id.as_str(), stack.count)),
        Some(("minecraft:wheat", 5)),
        "the named item landed in the destination"
    );
    assert_eq!(record.equipment[1], None, "no third stack appeared");
}

/// (CP-003) An order that names an item the source does not hold pauses as
/// `missing_input` and moves nothing, even when the source is not empty.
#[tokio::test]
async fn a_named_item_haul_of_an_absent_item_moves_nothing() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear_in(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:oak_log", 3)],
        true,
    );
    let haul = ScriptResidentWorkOrder::Haul {
        source: mc_script::ScriptInventoryEndpoint::ResidentEquipment {
            handle: handle.clone(),
        },
        destination: mc_script::ScriptInventoryEndpoint::ResidentCarry {
            handle: handle.clone(),
        },
        item: Some("minecraft:wheat".to_owned()),
    };
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("haul-absent", &handle, haul, 4, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingInput));
    assert_eq!(assignment.work_units_done, 0);
    assert!(assignment.changes.is_empty());
    assert_eq!(
        gear(&storage, &handle),
        vec![("minecraft:oak_log".to_owned(), 3)],
        "the source is exactly as it was"
    );
}

/// (R1) The harvested crop ends up in the worker's own cargo, and the receipt
/// states exactly what it holds: a receipt delta with no owner was the defect.
#[tokio::test]
async fn harvest_deposits_the_real_crop_into_the_worker_cargo() {
    let fixture = Fixture::new(false);
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-cargo",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                4,
                revision,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert!(assignment.work_units_done >= 1);
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air")),
        "the crop really left the world"
    );

    let carried: Vec<(String, u32)> = carry(&storage, &handle);
    let wheat = carried
        .iter()
        .find(|(item, _)| item == "minecraft:wheat")
        .map(|(_, count)| *count)
        .unwrap_or(0);
    assert!(
        wheat >= 1,
        "the worker owns the crop it harvested: {carried:?}"
    );
    for change in assignment.changes.iter().filter(|change| change.delta > 0) {
        let held = carried
            .iter()
            .filter(|(item, _)| *item == change.item_id)
            .map(|(_, count)| i64::from(*count))
            .sum::<i64>();
        assert!(
            held >= change.delta,
            "the receipt claims {} of {} but the cargo holds {held}",
            change.delta,
            change.item_id
        );
    }

    // The observable read path agrees with the record: a plugin asks for the
    // resident's carry and gets these stacks.
    let snapshot = fixture
        .runtime
        .execute_owned_inventory(&mut storage, OWNER, &carry_request(&handle, None))
        .await
        .expect("the carry read reaches the durable boundary");
    assert_eq!(snapshot.failure(), None, "{snapshot:?}");
    let counted = carried_from(&snapshot);
    assert!(
        counted
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count == wheat),
        "the carry snapshot shows the harvested crop: {counted:?}"
    );
}

/// (CP-007) A receipt-bearing resident break at a regional edge still uses the
/// journaled regional decision instead of falling through to a direct refusal.
#[tokio::test]
async fn harvest_at_regional_edge_commits_the_crop_and_cargo_together() {
    let fixture = Fixture::new(false);
    fixture.seed_edge_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-regional-edge",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([0, 64, 2], [0, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "air"))
    );
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count >= 1),
        "the edge decision records its crop in worker cargo"
    );
}

/// (CP-007) A storage projection failure after the world decision never loses
/// the previewed crop: reopening projects the exact journal receipt once.
#[tokio::test]
async fn harvest_recovers_cargo_and_progress_after_storage_projection_fails() {
    let fixture = Fixture::new(false);
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let request = work_request(
        "harvest-projection-recovery",
        &handle,
        ScriptResidentWorkOrder::Harvest {
            area: area([2, 64, 2], [2, 64, 2]),
            tool: "minecraft:iron_hoe".to_owned(),
        },
        1,
        revision,
    );

    storage.inject_fault_for_test(StorageFaultPoint::Write);
    assert!(matches!(
        fixture
            .runtime
            .execute_resident_order_operation(&mut storage, OWNER, &request)
            .await,
        Err(PluginStorageMutationError::DurabilityUnknown(_))
    ));
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air")),
        "the accepted world decision keeps the broken crop"
    );

    drop(storage);
    let mut reopened = fixture.storage();
    fixture
        .runtime
        .recover(&mut reopened)
        .expect("the accepted world receipt projects at recovery");
    let recovered = fixture.execute(&mut reopened, &request).await;
    let assignment = work_of(&recovered);
    assert_eq!(assignment.work_units_done, 1);
    let wheat = carry(&reopened, &handle)
        .iter()
        .filter(|(item, _)| item == "minecraft:wheat")
        .map(|(_, count)| *count)
        .sum::<u32>();
    assert_eq!(wheat, 1, "recovery must not duplicate the canonical crop");
}

/// (CP-007) A mining receipt recovers its exact ore cargo and work progress
/// after storage projection fails following the accepted world decision.
#[tokio::test]
async fn mine_recovers_cargo_and_progress_after_storage_projection_fails() {
    let fixture = Fixture::new(false);
    fixture.seed_ore();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_pickaxe", 1)],
    );
    let request = work_request(
        "mine-projection-recovery",
        &handle,
        ScriptResidentWorkOrder::Mine {
            area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
            tool: "minecraft:iron_pickaxe".to_owned(),
        },
        1,
        revision,
    );

    storage.inject_fault_for_test(StorageFaultPoint::Write);
    assert!(matches!(
        fixture
            .runtime
            .execute_resident_order_operation(&mut storage, OWNER, &request)
            .await,
        Err(PluginStorageMutationError::DurabilityUnknown(_))
    ));
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "air")),
        "the accepted world decision keeps the broken ore"
    );

    drop(storage);
    let mut reopened = fixture.storage();
    fixture
        .runtime
        .recover(&mut reopened)
        .expect("the accepted mine receipt projects at recovery");
    let recovered = fixture.execute(&mut reopened, &request).await;
    assert_eq!(work_of(&recovered).work_units_done, 1);
    let raw_iron = carry(&reopened, &handle)
        .iter()
        .filter(|(item, _)| item == "minecraft:raw_iron")
        .map(|(_, count)| *count)
        .sum::<u32>();
    assert_eq!(raw_iron, 1, "recovery must not duplicate canonical ore");
}
