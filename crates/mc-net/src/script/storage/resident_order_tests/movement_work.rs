//! Resident work contracts: crop cycle, mining, cut-tree, fishing/livestock, crafting and world-event resume.
//!
//! Moved out of `resident_order_tests.rs` by CP-REFACTOR-040 sub-slice 5.
//! Child of `resident_order_tests`: explicit imports only, no glob.
//!
//! Shared builders (`crafting_station`, `state_with_props`, `area`, `work_of`, `work_request`, `carry`) stay in shared support.

use crate::script::storage::PluginStorageHandle;
use crate::script::storage::resident_orders::DurableResidentOrderChange;
use crate::server::ScriptEventSink;
use crate::server::ShutdownHandle;
use mc_data::Identifier;
use mc_entity::SpawnEntity;
use mc_entity::Vec3;
use mc_script::COMPONENT_PLUGIN_API_VERSION;
use mc_script::MAX_RESIDENT_CARRY_SLOTS;
use mc_script::MAX_RESIDENT_EQUIPMENT_SLOTS;
use mc_script::ScriptAxisAlignedZone;
use mc_script::ScriptEventKind;
use mc_script::ScriptOperation;
use mc_script::ScriptOperationFailure;
use mc_script::ScriptOperationRequest;
use mc_script::ScriptPluginManifest;
use mc_script::ScriptPosition;
use mc_script::ScriptResidentOrderOperation;
use mc_script::ScriptResidentWorkOrder;
use mc_script::ScriptWorkPauseReason;
use mc_script::ScriptWorkState;
use mc_script::script_boundary_pair;
use mc_world::BlockPos;
use mc_world::Chunk;
use mc_world::ChunkPos;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::{
    Fixture, OWNER, SURFACE_Y, area, carry, crafting_station, gear, seed_gear, seed_gear_in,
    state_of, state_with_props, work_of, work_request,
};
/// (CP-007) Replacing a crop after preview consumes neither the replacement
/// block nor the previewed loot; the worker record keeps its old progress.
#[tokio::test]
async fn replaced_crop_after_preview_does_not_create_cargo_or_work_progress() {
    let fixture = Fixture::with_replacement(false, Some("stone"));
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
                "harvest-replaced-after-preview",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "stone")),
        "the replacement survives the stale conditional decision"
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:wheat"),
        "a stale preview cannot mint crop cargo"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .expect("seeded resident record remains");
    assert_eq!(record.revision, revision);
    assert!(
        record.work.is_none(),
        "the stale call records no work progress"
    );
}

/// (CP-008) Harvesting and replanting use the real crop state and leave the
/// worker with the canonical produce and the exact remaining seed reserve.
#[tokio::test]
async fn harvest_then_replant_commits_crop_seed_and_work_together() {
    let fixture = Fixture::new(false);
    fixture.seed_edge_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_hoe", 1), ("minecraft:wheat_seeds", 2)],
    );
    let harvest = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-before-replant",
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
    assert_eq!(harvest.failure(), None, "{harvest:?}");
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "air"))
    );

    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("harvest record")
        .revision;
    let seeds_before_replant = carry(&storage, &handle)
        .iter()
        .find(|(item, _)| item == "minecraft:wheat_seeds")
        .map(|(_, count)| *count)
        .expect("harvest retains the canonical seed drop");
    let replant = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-after-harvest",
                &handle,
                ScriptResidentWorkOrder::Replant {
                    area: area([0, SURFACE_Y, 2], [0, SURFACE_Y, 2]),
                    seed: "minecraft:wheat_seeds".to_owned(),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&replant);
    assert_eq!(replant.failure(), None, "{replant:?}");
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "wheat")),
        "replant places the registered default crop state"
    );
    let carried = carry(&storage, &handle);
    assert_eq!(
        carried
            .iter()
            .find(|(item, _)| item == "minecraft:wheat_seeds")
            .map(|(_, count)| *count),
        Some(seeds_before_replant - 1),
        "exactly one seed left the worker's reserve"
    );
    assert!(
        carried
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count >= 1),
        "the first cycle's canonical harvest remains worker-owned"
    );
}

/// (CP-008) A replanted crop only starts its next cycle after the real world
/// reports maturity; its second harvest and replant retain worker-owned output.
#[tokio::test]
async fn a_matured_replanted_crop_completes_a_second_cycle() {
    let fixture = Fixture::new(false);
    fixture.seed_edge_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 5.5))
        .await;
    let harvest_work = ScriptResidentWorkOrder::Harvest {
        area: area([0, 64, 2], [0, 64, 2]),
        tool: "minecraft:iron_hoe".to_owned(),
    };
    let replant_work = ScriptResidentWorkOrder::Replant {
        area: area([0, SURFACE_Y, 2], [0, SURFACE_Y, 2]),
        seed: "minecraft:wheat_seeds".to_owned(),
        tool: "minecraft:iron_hoe".to_owned(),
    };
    let mut revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_hoe", 1), ("minecraft:wheat_seeds", 3)],
    );
    for (operation_id, work) in [
        ("first-harvest", harvest_work.clone()),
        ("first-replant", replant_work.clone()),
    ] {
        let outcome = fixture
            .execute(
                &mut storage,
                &work_request(operation_id, &handle, work, 1, revision),
            )
            .await;
        assert_eq!(outcome.failure(), None, "{outcome:?}");
        revision = storage
            .resident_orders()
            .record(&handle)
            .expect("completed first-cycle record")
            .revision;
    }
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "wheat")),
        "a newly replanted crop is not mature by receipt alone"
    );
    let premature = fixture
        .execute(
            &mut storage,
            &work_request(
                "premature-second-harvest",
                &handle,
                harvest_work.clone(),
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&premature).reason,
        Some(ScriptWorkPauseReason::MissingInput),
        "an immature crop cannot become loot before a world growth update"
    );
    assert_eq!(work_of(&premature).work_units_done, 0);
    revision = storage
        .resident_orders()
        .record(&handle)
        .expect("paused premature harvest record")
        .revision;
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 0, y: 64, z: 2 },
            state_with_props(&fixture.blocks, "wheat", &[("age", "7")]),
        )
        .expect("the loaded field accepts a real growth update");
    for (operation_id, work) in [
        ("second-harvest", harvest_work),
        ("second-replant", replant_work),
    ] {
        let outcome = fixture
            .execute(
                &mut storage,
                &work_request(operation_id, &handle, work, 1, revision),
            )
            .await;
        assert_eq!(outcome.failure(), None, "{outcome:?}");
        revision = storage
            .resident_orders()
            .record(&handle)
            .expect("completed second-cycle record")
            .revision;
    }
    assert_eq!(
        fixture.block(0, 64, 2).await,
        Some(state_of(&fixture.blocks, "wheat")),
        "the second cycle replants the world-grown crop"
    );
    assert!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:wheat")
            .map(|(_, count)| *count)
            .sum::<u32>()
            >= 2,
        "both actual harvests remain worker-owned"
    );
}

/// (CP-008) A field with no seed reserve advances neither the replant
/// assignment nor a speculative world decision.
#[tokio::test]
async fn replant_without_seed_leaves_the_field_unchanged() {
    let fixture = Fixture::new(false);
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let replant = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-without-seed",
                &handle,
                ScriptResidentWorkOrder::Replant {
                    area: area([2, SURFACE_Y, 2], [2, SURFACE_Y, 2]),
                    seed: "minecraft:wheat_seeds".to_owned(),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&replant);
    assert_eq!(replant.failure(), None, "{replant:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingInput));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_with_props(&fixture.blocks, "wheat", &[("age", "7")])),
        "a missing seed cannot change the field"
    );
}

/// (CP-008) A seed reserve alone cannot plant a crop: replant requires its
/// configured tool before either the field or inventory record can change.
#[tokio::test]
async fn replant_without_tool_leaves_the_field_and_seed_unchanged() {
    let fixture = Fixture::new(false);
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat_seeds", 1)]);
    let replant = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-without-tool",
                &handle,
                ScriptResidentWorkOrder::Replant {
                    area: area([2, SURFACE_Y, 2], [2, SURFACE_Y, 2]),
                    seed: "minecraft:wheat_seeds".to_owned(),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&replant);
    assert_eq!(replant.failure(), None, "{replant:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingTool));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_with_props(&fixture.blocks, "wheat", &[("age", "7")])),
        "a missing tool cannot change the field"
    );
    assert_eq!(
        carry(&storage, &handle),
        vec![("minecraft:wheat_seeds".to_owned(), 1)],
        "a missing tool cannot spend the seed reserve"
    );
}

/// (CP-008) A player edit after placement preview consumes neither seed nor
/// replant progress; the changed field wins the conditional decision.
#[tokio::test]
async fn changed_field_after_replant_preview_keeps_seed_and_work_unchanged() {
    let fixture = Fixture::with_replant_replacement(false, "stone");
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_hoe", 1), ("minecraft:wheat_seeds", 1)],
    );
    let harvest = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-before-changed-replant",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(harvest.failure(), None, "{harvest:?}");
    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("harvest record")
        .revision;
    let seeds_before_replant = carry(&storage, &handle)
        .iter()
        .find(|(item, _)| item == "minecraft:wheat_seeds")
        .map(|(_, count)| *count)
        .expect("harvest retains the canonical seed drop");
    let wheat_before_replant = carry(&storage, &handle)
        .iter()
        .filter(|(item, _)| item == "minecraft:wheat")
        .map(|(_, count)| *count)
        .sum::<u32>();
    let replant = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-after-player-field-change",
                &handle,
                ScriptResidentWorkOrder::Replant {
                    area: area([2, SURFACE_Y, 2], [2, SURFACE_Y, 2]),
                    seed: "minecraft:wheat_seeds".to_owned(),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        replant.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "stone")),
        "the player-changed field survives the rejected placement"
    );
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .find(|(item, _)| item == "minecraft:wheat_seeds")
            .map(|(_, count)| *count),
        Some(seeds_before_replant),
        "a rejected placement preserves the durable seed reserve"
    );
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:wheat")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        wheat_before_replant,
        "a rejected placement cannot duplicate harvested produce"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .expect("harvest record remains");
    assert_eq!(record.revision, revision);
    assert!(
        matches!(
            record.work.as_deref(),
            Some(work) if matches!(&work.work, ScriptResidentWorkOrder::Harvest { .. })
        ),
        "the rejected replant cannot replace prior committed work progress"
    );
}

/// (CP-008) A replant receipt never exceeds the simulation's conditional-edit
/// cap: the durable work watermark resumes the one remaining real field cell.
#[tokio::test]
async fn replant_splits_at_the_simulation_edit_limit() {
    let fixture = Fixture::new(false);
    fixture.prepare_dense_replant_field().await;
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 0.5))
        .await;
    let equipment = [
        ("minecraft:iron_hoe", 1),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
    ];
    let carry = [
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
        ("minecraft:wheat_seeds", 64),
    ];
    let revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true);
    let revision = seed_gear_in(&mut storage, &handle, uuid, &carry, false).max(revision);
    let work = ScriptResidentWorkOrder::Replant {
        area: area([0, 40, 0], [15, 44, 15]),
        seed: "minecraft:wheat_seeds".to_owned(),
        tool: "minecraft:iron_hoe".to_owned(),
    };
    let first = fixture
        .execute(
            &mut storage,
            &work_request(
                "replant-edit-limit-first",
                &handle,
                work.clone(),
                513,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&first);
    assert_eq!(first.failure(), None, "{first:?}");
    assert_eq!(assignment.state, ScriptWorkState::Running);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 512);

    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("first portion record")
        .revision;
    let resumed = fixture
        .execute(
            &mut storage,
            &work_request("replant-edit-limit-resume", &handle, work, 513, revision),
        )
        .await;
    let assignment = work_of(&resumed);
    assert_eq!(resumed.failure(), None, "{resumed:?}");
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 513);
}

/// (R1) A worker who cannot hold the loot leaves the world alone: no block is
/// broken to produce items nobody can own.
#[tokio::test]
async fn a_full_worker_reports_no_storage_and_leaves_the_crop_standing() {
    let fixture = Fixture::new(false);
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let mut revision = seed_gear_in(&mut storage, &handle, uuid, &fill, false);
    let equipment: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true).max(revision);
    // The hoe takes the only slot a full worker would have had to spare.
    revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]).max(revision);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "harvest-full",
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
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(assignment.work_units_done, 0);
    assert!(
        assignment.changes.iter().all(|change| change.delta <= 0),
        "nothing was produced: {:?}",
        assignment.changes
    );
    assert_ne!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air")),
        "the crop stays because the worker could not hold it"
    );
}

/// (R1) The same cargo path carries mined ore: one producer, one owner.
#[tokio::test]
async fn mined_ore_reaches_the_worker_cargo() {
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

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-cargo",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(assignment.work_units_done, 1);
    let carried = carry(&storage, &handle);
    assert!(
        carried
            .iter()
            .any(|(item, count)| item == "minecraft:raw_iron" && *count >= 1),
        "the mined ore is the worker's: {carried:?} changes {:?}",
        assignment.changes
    );
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "air")),
        "the ore really left the world"
    );
}

/// (CP-010) Mining must not destroy an ore whose canonical loot cannot enter
/// the worker's actual inventory.
#[tokio::test]
async fn mine_with_full_cargo_leaves_ore_untouched() {
    let fixture = Fixture::new(false);
    fixture.seed_ore();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
    let fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let mut revision = seed_gear_in(&mut storage, &handle, uuid, &fill, false);
    let equipment: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true).max(revision);
    revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_pickaxe", 1)],
    )
    .max(revision);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-full",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "iron_ore")),
        "a full worker cannot convert an ore to unowned loot"
    );
}

/// (CP-010) An unsuitable tool cannot consume an ore block without producing
/// its canonical loot for the worker.
#[tokio::test]
async fn mine_with_unsuitable_tool_preserves_ore() {
    let fixture = Fixture::new(false);
    fixture.seed_ore();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-with-hoe",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingTool));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "iron_ore")),
        "a non-dropping tool must leave the ore in the world"
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:raw_iron"),
        "a refused mine does not mint ore cargo"
    );
}

/// (CP-010) A tool-tier refusal after one accepted ore preserves the next ore
/// and exposes the reason without rolling back the accepted durable decision.
#[tokio::test]
async fn mine_pauses_for_a_later_ore_that_needs_a_better_tool() {
    let fixture = Fixture::new(false);
    fixture.seed_ore();
    {
        let mut world = fixture.world.lock().await;
        world
            .set_block_at(
                BlockPos {
                    x: 4,
                    y: SURFACE_Y - 2,
                    z: 4,
                },
                state_of(&fixture.blocks, "coal_ore"),
            )
            .expect("the first ore remains in the loaded fixture chunk");
        world
            .set_block_at(
                BlockPos {
                    x: 5,
                    y: SURFACE_Y - 2,
                    z: 4,
                },
                state_of(&fixture.blocks, "diamond_ore"),
            )
            .expect("the later ore remains in the loaded fixture chunk");
    }
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:stone_pickaxe", 1)],
    );
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-coal-before-diamond",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [5, SURFACE_Y - 2, 4]),
                    tool: "minecraft:stone_pickaxe".to_owned(),
                },
                2,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingTool));
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "air")),
        "the canonical coal decision remains committed"
    );
    assert_eq!(
        fixture.block(5, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "diamond_ore")),
        "the tier-gated ore remains available for a suitable tool"
    );
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:coal" && *count >= 1),
        "only the accepted coal reaches the worker"
    );
}

/// (CP-010) An ore block inside solid terrain remains unavailable until a
/// resident can reach one of its bounded mining stances.
#[tokio::test]
async fn mine_does_not_extract_a_hidden_ore_without_a_route() {
    let fixture = Fixture::new(false);
    let hidden = [10, SURFACE_Y - 2, 10];
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: hidden[0],
                y: hidden[1],
                z: hidden[2],
            },
            state_of(&fixture.blocks, "iron_ore"),
        )
        .expect("hidden ore remains inside the loaded stone band");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_pickaxe", 1)],
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-hidden-ore",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area(hidden, hidden),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::BlockedRoute)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert_eq!(
        fixture.block(hidden[0], hidden[1], hidden[2]).await,
        Some(state_of(&fixture.blocks, "iron_ore"))
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:raw_iron"),
        "a sealed ore cannot become worker cargo"
    );
}

/// (CP-010) Protected mine cells pause before a physical preview can alter the
/// world or the worker's cargo.
#[tokio::test]
async fn mine_in_a_protected_zone_leaves_the_ore_untouched() {
    let protected = Arc::new(AtomicBool::new(true));
    let fixture = Fixture::with_protection_gate(protected);
    fixture.seed_ore();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_pickaxe", 1)],
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-protected-ore",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::Protected)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "iron_ore"))
    );
}

/// (CP-010) A source changed after an accessible mine preview commits neither
/// canonical ore loot nor progress.
#[tokio::test]
async fn replaced_ore_after_preview_does_not_create_cargo_or_work_progress() {
    let fixture = Fixture::with_replacement(false, Some("stone"));
    fixture.seed_ore();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_pickaxe", 1)],
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-replaced-after-preview",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([4, SURFACE_Y - 2, 4], [4, SURFACE_Y - 2, 4]),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(
        fixture.block(4, SURFACE_Y - 2, 4).await,
        Some(state_of(&fixture.blocks, "stone"))
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:raw_iron"),
        "a stale mine preview cannot mint ore cargo"
    );
    let record = storage
        .resident_orders()
        .record(&handle)
        .expect("seeded resident record remains");
    assert_eq!(record.revision, revision);
    assert!(
        record.work.is_none(),
        "the stale call records no work progress"
    );
}

/// (CP-010) An unloaded work cell has no route or loot fallback and leaves the
/// resident work record at zero progress.
#[tokio::test]
async fn mine_in_an_unloaded_area_pauses_without_cargo() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture.mining_resident(&mut storage, 3).await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[("minecraft:iron_pickaxe", 1)],
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "mine-unloaded-ore",
                &handle,
                ScriptResidentWorkOrder::Mine {
                    area: area([32, SURFACE_Y - 2, 32], [32, SURFACE_Y - 2, 32]),
                    tool: "minecraft:iron_pickaxe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::Unloaded)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:raw_iron")
    );
}

/// (CP-007) Tree cutting uses the same receipt-bearing break boundary as crop
/// harvest and mining.
#[tokio::test]
async fn cut_tree_commits_the_log_and_worker_cargo_together() {
    let fixture = Fixture::new(false);
    fixture.seed_tree();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-cargo",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [6, SURFACE_Y + 2, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert_eq!(assignment.work_units_done, 1);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:oak_log" && *count == 1),
        "the worker receives the one committed log"
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "air")),
        "the matching world log was committed in the receipt decision"
    );
}

/// (CP-009) Every trunk cell in one bounded taller tree shares the route to
/// its rooted standing position, so one receipt cannot strand the upper log.
#[tokio::test]
async fn cut_tree_reaches_the_full_tall_trunk_from_its_rooted_route() {
    let fixture = Fixture::new(false);
    fixture.seed_tree();
    let log = state_of(&fixture.blocks, "oak_log");
    let leaves = state_of(&fixture.blocks, "oak_leaves");
    {
        let mut world = fixture.world.lock().await;
        world
            .set_block_at(
                BlockPos {
                    x: 6,
                    y: SURFACE_Y + 3,
                    z: 6,
                },
                log,
            )
            .expect("the fixture canopy column accepts the taller trunk");
        for x in 5..=7 {
            for z in 5..=7 {
                world
                    .set_block_at(
                        BlockPos {
                            x,
                            y: SURFACE_Y + 4,
                            z,
                        },
                        leaves,
                    )
                    .expect("the fixture accepts the lifted canopy");
            }
        }
    }
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-tall-tree",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [6, SURFACE_Y + 3, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                3,
                revision,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).work_units_done, 3);
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:oak_log")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        3
    );
}
/// (CP-009) A grounded house column beside a real tree remains untouched.
#[tokio::test]
async fn cut_tree_keeps_adjacent_grounded_house_logs() {
    let fixture = Fixture::new(false);
    fixture.seed_tree();

    let house_log = state_of(&fixture.blocks, "oak_log");
    {
        let mut world = fixture.world.lock().await;
        for y in SURFACE_Y + 1..=SURFACE_Y + 2 {
            world
                .set_block_at(BlockPos { x: 9, y, z: 6 }, house_log)
                .expect("the house column stays in the loaded fixture chunk");
        }
    }
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-tree-not-house",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [9, SURFACE_Y + 2, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                3,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(
        fixture.block(9, SURFACE_Y + 1, 6).await,
        Some(house_log),
        "a grounded log column without a canopy is a house, not a tree"
    );
    assert_eq!(
        fixture.block(9, SURFACE_Y + 2, 6).await,
        Some(house_log),
        "the worker must not progress into the adjacent house column"
    );
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:oak_log")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        2,
        "only the two natural-tree trunk logs reach the worker"
    );
}

/// (CP-009) A real tree behind a blocked route remains in the world; a work
/// receipt does not teleport the logger into a reachable-looking area.
#[tokio::test]
async fn cut_tree_with_no_standable_route_leaves_the_trunk_untouched() {
    let fixture = Fixture::new(false);
    fixture.seed_tree();
    let stone = state_of(&fixture.blocks, "stone");
    {
        let mut world = fixture.world.lock().await;
        for (x, z) in [(5, 6), (7, 6), (6, 5), (6, 7)] {
            world
                .set_block_at(
                    BlockPos {
                        x,
                        y: SURFACE_Y + 1,
                        z,
                    },
                    stone,
                )
                .expect("the loaded fixture can close every adjacent route");
        }
    }
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-tree-blocked-route",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [6, SURFACE_Y + 2, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::BlockedRoute));
    assert_eq!(assignment.work_units_done, 0);
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log")),
        "the unreachable trunk remains intact"
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:oak_log"),
        "an unreachable tree cannot mint log cargo"
    );
}

/// (CP-009) A logger without its real axe cannot consume an otherwise valid
/// natural trunk.
#[tokio::test]
async fn cut_tree_without_an_axe_leaves_the_tree_untouched() {
    let fixture = Fixture::new(false);
    fixture.seed_tree();
    let mut storage = fixture.storage();
    let (handle, _uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-tree-without-axe",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [6, SURFACE_Y + 2, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                1,
                0,
            ),
        )
        .await;
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::MissingTool)
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log"))
    );
}

/// (CP-009) A full logger cannot turn a real tree into unowned drops.
#[tokio::test]
async fn cut_tree_with_full_cargo_leaves_the_tree_untouched() {
    let fixture = Fixture::new(false);
    fixture.seed_tree();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let mut revision = seed_gear_in(&mut storage, &handle, uuid, &fill, false);
    let equipment: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true).max(revision);
    revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]).max(revision);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "cut-tree-full",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [6, SURFACE_Y + 2, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::NoStorage)
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log"))
    );
}

/// (CP-009) Removing the matching permission fence resumes the same logger
/// once; an unchanged protected tree remains unmodified.
#[tokio::test]
async fn released_tree_permission_resumes_the_paused_logger_once() {
    let protected = Arc::new(AtomicBool::new(true));
    let fixture = Fixture::with_protection_gate(Arc::clone(&protected));
    fixture.seed_tree();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_axe", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-protected-tree",
                &handle,
                ScriptResidentWorkOrder::CutTree {
                    area: area([6, SURFACE_Y + 1, 6], [6, SURFACE_Y + 2, 6]),
                    tool: "minecraft:iron_axe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::Protected)
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "oak_log"))
    );
    let released_zone = ScriptAxisAlignedZone::try_new(
        "released-tree",
        "minecraft:overworld",
        ScriptPosition::try_new(5.5, 63.0, 5.5).expect("valid zone minimum"),
        ScriptPosition::try_new(7.5, 66.0, 7.5).expect("valid zone maximum"),
    )
    .expect("valid released zone");
    protected.store(false, Ordering::Release);
    let resumed = fixture
        .runtime
        .resume_paused_work_for_zone(&mut storage, &released_zone, |_| true)
        .await
        .expect("released tree permission resumes work");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert_eq!(
        fixture.block(6, SURFACE_Y + 1, 6).await,
        Some(state_of(&fixture.blocks, "air"))
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_zone(&mut storage, &released_zone, |_| true)
            .await
            .expect("the completed logger cannot resume twice")
            .is_empty()
    );
}

/// (CP-011) Fishing pays a durable owner only from water currently present in
/// the named work area; its deterministic cod result is not a vanilla-loot claim.
#[tokio::test]
async fn fish_uses_real_water_and_credits_resident_cargo() {
    let fixture = Fixture::new(false);
    fixture.seed_water();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(7.5, 64.0, 8.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]);
    let request = work_request(
        "fish-water-column",
        &handle,
        ScriptResidentWorkOrder::Fish {
            area: area([8, SURFACE_Y, 8], [8, SURFACE_Y, 8]),
            tool: "minecraft:fishing_rod".to_owned(),
        },
        1,
        revision,
    );
    let outcome = fixture.execute(&mut storage, &request).await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 1);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:cod" && *count == 1),
        "the catch belongs to the worker"
    );
    drop(storage);
    let mut reopened = fixture.storage();
    let replay = fixture.execute(&mut reopened, &request).await;
    assert_eq!(work_of(&replay).work_units_done, 1);
    assert_eq!(
        carry(&reopened, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:cod")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        1,
        "restart and replay preserve the one committed catch"
    );
}

/// (CP-011) A changed water source cannot produce another catch after the
/// previous durable result.
#[tokio::test]
async fn fish_stops_when_its_water_source_is_removed() {
    let fixture = Fixture::new(false);
    fixture.seed_water();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(7.5, 64.0, 8.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]);
    let work = ScriptResidentWorkOrder::Fish {
        area: area([8, SURFACE_Y, 8], [8, SURFACE_Y, 8]),
        tool: "minecraft:fishing_rod".to_owned(),
    };
    let first = fixture
        .execute(
            &mut storage,
            &work_request(
                "fish-before-water-removal",
                &handle,
                work.clone(),
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(work_of(&first).work_units_done, 1);
    let revision = storage
        .resident_orders()
        .record(&handle)
        .expect("first fish work record")
        .revision;
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 8,
                y: SURFACE_Y,
                z: 8,
            },
            state_of(&fixture.blocks, "air"),
        )
        .expect("fixture water cell remains loaded");

    let removed = fixture
        .execute(
            &mut storage,
            &work_request("fish-after-water-removal", &handle, work, 2, revision),
        )
        .await;
    assert_eq!(removed.failure(), None, "{removed:?}");
    assert_eq!(work_of(&removed).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&removed).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert_eq!(work_of(&removed).work_units_done, 0);
    assert_eq!(
        carry(&storage, &handle)
            .iter()
            .filter(|(item, _)| item == "minecraft:cod")
            .map(|(_, count)| *count)
            .sum::<u32>(),
        1
    );
}

/// (CP-011) A fisher with no cargo space retains its real water source and
/// receives neither a virtual catch nor progress.
#[tokio::test]
async fn fish_with_full_cargo_pauses_without_a_catch() {
    let fixture = Fixture::new(false);
    fixture.seed_water();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(7.5, 64.0, 8.5))
        .await;
    let fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let mut revision = seed_gear_in(&mut storage, &handle, uuid, &fill, false);
    let equipment: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    revision = seed_gear_in(&mut storage, &handle, uuid, &equipment, true).max(revision);
    revision =
        seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]).max(revision);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "fish-full-cargo",
                &handle,
                ScriptResidentWorkOrder::Fish {
                    area: area([8, SURFACE_Y, 8], [8, SURFACE_Y, 8]),
                    tool: "minecraft:fishing_rod".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::NoStorage)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:cod")
    );
}

/// (CP-011) Visible water behind a closed route is not a remote fishing source.
#[tokio::test]
async fn fish_with_no_route_leaves_the_water_and_cargo_untouched() {
    let fixture = Fixture::new(true);
    fixture.seed_water();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(12.5, 64.0, 8.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "fish-blocked-water",
                &handle,
                ScriptResidentWorkOrder::Fish {
                    area: area([8, SURFACE_Y, 8], [8, SURFACE_Y, 8]),
                    tool: "minecraft:fishing_rod".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::BlockedRoute)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert_eq!(
        fixture.block(8, SURFACE_Y, 8).await,
        Some(state_of(&fixture.blocks, "water"))
    );
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:cod")
    );
}

/// (CP-011) An unloaded first shore candidate does not hide a later loaded,
/// reachable shore at a chunk boundary.
#[tokio::test]
async fn fish_uses_a_loaded_boundary_shore_after_an_unloaded_stance() {
    let fixture = Fixture::new(false);
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 0,
                y: SURFACE_Y,
                z: 8,
            },
            state_of(&fixture.blocks, "water"),
        )
        .expect("boundary water remains in the loaded fixture chunk");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 7.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:fishing_rod", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "fish-boundary-shore",
                &handle,
                ScriptResidentWorkOrder::Fish {
                    area: area([0, SURFACE_Y, 8], [0, SURFACE_Y, 8]),
                    tool: "minecraft:fishing_rod".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Committed);
    assert_eq!(work_of(&outcome).work_units_done, 1);
}

/// (CP-011) An unloaded first animal stance does not mask a later loaded,
/// reachable stance at the same chunk boundary.
#[tokio::test]
async fn tend_livestock_uses_a_loaded_boundary_stance_after_an_unloaded_one() {
    let fixture = Fixture::new(false);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(0.5, 64.0, 8.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("cow joins the local session index");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(0.5, 64.0, 7.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-boundary-cow",
                &handle,
                ScriptResidentWorkOrder::TendLivestock {
                    area: area([0, 64, 8], [0, 64, 8]),
                    feed: "minecraft:wheat".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Committed);
    assert_eq!(work_of(&outcome).work_units_done, 1);
}

/// (CP-011) Tending spends feed only while a live, locally tracked animal is
/// available; it produces no virtual livestock goods.
#[tokio::test]
async fn tend_livestock_consumes_feed_for_a_visible_animal() {
    let fixture = Fixture::new(false);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(6.5, 64.0, 6.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("the cow joins the local session index");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-visible-cow",
                &handle,
                ScriptResidentWorkOrder::TendLivestock {
                    area: area([6, 64, 6], [6, 64, 6]),
                    feed: "minecraft:wheat".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 1);
    assert!(
        !carry(&storage, &handle)
            .iter()
            .any(|(item, _)| item == "minecraft:wheat"),
        "the real tending action consumes resident-owned feed"
    );
    assert!(
        assignment.changes.iter().all(|change| change.delta <= 0),
        "tending does not mint livestock output: {:?}",
        assignment.changes
    );
}

/// (CP-011) A live compatible animal does not turn an absent feed reserve into
/// a completed tending unit.
#[tokio::test]
async fn tend_livestock_without_feed_pauses_before_work() {
    let fixture = Fixture::new(false);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(6.5, 64.0, 6.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("cow joins the local session index");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-without-feed",
                &handle,
                ScriptResidentWorkOrder::TendLivestock {
                    area: area([6, 64, 6], [6, 64, 6]),
                    feed: "minecraft:wheat".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
}

/// (CP-011) A resident cannot spend wheat on a chicken or a baby cow: the
/// shared vanilla food tags and breeding state gate the durable cost.
#[tokio::test]
async fn tend_livestock_refuses_incompatible_or_immature_animals() {
    for (case, type_name, animal) in [
        (
            "incompatible-chicken",
            "minecraft:chicken",
            mc_entity::AnimalBreedingState::adult(),
        ),
        (
            "immature-cow",
            "minecraft:cow",
            mc_entity::AnimalBreedingState::baby(),
        ),
    ] {
        let fixture = Fixture::new(false);
        let mut entity = SpawnEntity::new(0, type_name, Vec3::new(6.5, 64.0, 6.5));
        entity.animal = Some(animal);
        fixture
            .sessions
            .spawn_tracked_entity_for_test(entity, false)
            .expect("the animal joins the local session index");
        let mut storage = fixture.storage();
        let (handle, uuid) = fixture
            .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
            .await;
        let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);
        let operation_id = format!("tend-{case}");
        let outcome = fixture
            .execute(
                &mut storage,
                &work_request(
                    &operation_id,
                    &handle,
                    ScriptResidentWorkOrder::TendLivestock {
                        area: area([6, 64, 6], [6, 64, 6]),
                        feed: "minecraft:wheat".to_owned(),
                    },
                    1,
                    revision,
                ),
            )
            .await;
        let assignment = work_of(&outcome);

        assert_eq!(outcome.failure(), None, "{case}: {outcome:?}");
        assert_eq!(assignment.state, ScriptWorkState::Paused, "{case}");
        assert_eq!(
            assignment.reason,
            Some(ScriptWorkPauseReason::MissingInput),
            "{case}"
        );
        assert_eq!(assignment.work_units_done, 0, "{case}");
        assert!(
            carry(&storage, &handle)
                .iter()
                .any(|(item, count)| item == "minecraft:wheat" && *count == 1),
            "{case}: an ineligible animal cannot consume feed"
        );
    }
}

/// (CP-011) A mature, compatible animal behind a closed route cannot consume
/// the resident's feed.
#[tokio::test]
async fn tend_livestock_with_no_route_preserves_feed() {
    let fixture = Fixture::new(true);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(6.5, 64.0, 6.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("cow joins the local session index");
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(12.5, 64.0, 6.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-blocked-cow",
                &handle,
                ScriptResidentWorkOrder::TendLivestock {
                    area: area([6, 64, 6], [6, 64, 6]),
                    feed: "minecraft:wheat".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::BlockedRoute)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count == 1)
    );
}

/// (CP-011) A removed animal is not retained as an invisible livestock source.
#[tokio::test]
async fn dead_livestock_preserves_feed_and_work_progress() {
    let fixture = Fixture::new(false);
    let mut cow = SpawnEntity::new(0, "minecraft:cow", Vec3::new(6.5, 64.0, 6.5));
    cow.animal = Some(mc_entity::AnimalBreedingState::adult());
    let cow_id = fixture
        .sessions
        .spawn_tracked_entity_for_test(cow, false)
        .expect("cow joins the local session index");
    assert!(
        fixture.sessions.remove_tracked_entity_for_test(cow_id),
        "the test removes the live source before work begins"
    );
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:wheat", 1)]);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "tend-dead-cow",
                &handle,
                ScriptResidentWorkOrder::TendLivestock {
                    area: area([6, 64, 6], [6, 64, 6]),
                    feed: "minecraft:wheat".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(work_of(&outcome).state, ScriptWorkState::Paused);
    assert_eq!(
        work_of(&outcome).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert_eq!(work_of(&outcome).work_units_done, 0);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count == 1)
    );
}

/// (CP-011) Water and livestock work both preserve their input when their
/// corresponding live source is absent.
#[tokio::test]
async fn fish_and_livestock_without_sources_preserve_inputs() {
    let fishing = Fixture::new(false);
    let mut fishing_storage = fishing.storage();
    let (fisher, fisher_uuid) = fishing
        .resident(&mut fishing_storage, 3, Vec3::new(7.5, 64.0, 7.5))
        .await;
    let fishing_revision = seed_gear(
        &mut fishing_storage,
        &fisher,
        fisher_uuid,
        &[("minecraft:fishing_rod", 1)],
    );
    let fishing_outcome = fishing
        .execute(
            &mut fishing_storage,
            &work_request(
                "fish-without-water",
                &fisher,
                ScriptResidentWorkOrder::Fish {
                    area: area([7, SURFACE_Y, 7], [7, SURFACE_Y, 7]),
                    tool: "minecraft:fishing_rod".to_owned(),
                },
                1,
                fishing_revision,
            ),
        )
        .await;
    let fishing_work = work_of(&fishing_outcome);
    assert_eq!(fishing_work.state, ScriptWorkState::Paused);
    assert_eq!(
        fishing_work.reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert!(
        carry(&fishing_storage, &fisher)
            .iter()
            .any(|(item, count)| item == "minecraft:fishing_rod" && *count == 1),
        "no water leaves the rod untouched"
    );

    let livestock = Fixture::new(false);
    let mut livestock_storage = livestock.storage();
    let (tender, tender_uuid) = livestock
        .resident(&mut livestock_storage, 3, Vec3::new(6.5, 64.0, 5.5))
        .await;
    let livestock_revision = seed_gear(
        &mut livestock_storage,
        &tender,
        tender_uuid,
        &[("minecraft:wheat", 1)],
    );
    let livestock_outcome = livestock
        .execute(
            &mut livestock_storage,
            &work_request(
                "tend-without-livestock",
                &tender,
                ScriptResidentWorkOrder::TendLivestock {
                    area: area([6, 64, 6], [6, 64, 6]),
                    feed: "minecraft:wheat".to_owned(),
                },
                1,
                livestock_revision,
            ),
        )
        .await;
    let livestock_work = work_of(&livestock_outcome);
    assert_eq!(livestock_work.state, ScriptWorkState::Paused);
    assert_eq!(
        livestock_work.reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    assert!(
        carry(&livestock_storage, &tender)
            .iter()
            .any(|(item, count)| item == "minecraft:wheat" && *count == 1),
        "no livestock leaves the feed with its owner"
    );
}

/// (A08) A craft without inputs commits nothing; with inputs it consumes real
/// items and produces the recipe result.
#[tokio::test]
async fn craft_requires_inputs_and_consumes_them_exactly_once() {
    let fixture = Fixture::new(false);
    fixture.seed_crafting_table();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let craft = ScriptResidentWorkOrder::Craft {
        recipe: "minecraft:oak_planks".to_owned(),
        count: 2,
        station: crafting_station(),
    };

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("craft-1", &handle, craft.clone(), 2, 0),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingInput));
    assert!(assignment.changes.is_empty());

    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 2)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("craft-2", &handle, craft.clone(), 2, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(
        assignment.state,
        ScriptWorkState::Committed,
        "{assignment:?}"
    );
    assert_eq!(assignment.work_units_done, 2);
    let changes = assignment
        .changes
        .iter()
        .map(|change| (change.item_id.as_str(), change.delta))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(changes.get("minecraft:oak_log"), Some(&-2));
    assert_eq!(changes.get("minecraft:oak_planks"), Some(&8));

    // The same operation id with a different payload conflicts instead of
    // crafting again.
    let conflicting = work_request(
        "craft-2",
        &handle,
        ScriptResidentWorkOrder::Craft {
            recipe: "minecraft:oak_planks".to_owned(),
            count: 1,
            station: crafting_station(),
        },
        1,
        assignment.revision,
    );
    let outcome = fixture.execute(&mut storage, &conflicting).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
    let planks = gear(&storage, &handle)
        .into_iter()
        .filter(|(item, _)| item == "minecraft:oak_planks")
        .map(|(_, count)| count)
        .sum::<u32>();
    assert_eq!(planks, 8, "no second craft ran");
}

/// (CP-012) A recipe uses its own stateless station capability: campfire food
/// does not run at a crafting table, and the mismatch preserves the real input.
#[tokio::test]
async fn craft_requires_the_recipe_matching_station_for_campfire_food() {
    let fixture = Fixture::new(false);
    fixture.seed_crafting_table();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:cod", 1)]);
    let work = ScriptResidentWorkOrder::Craft {
        recipe: "minecraft:cooked_cod_from_campfire_cooking".to_owned(),
        count: 1,
        station: crafting_station(),
    };

    let missing_station = fixture
        .execute(
            &mut storage,
            &work_request("cook-cod-wrong-station", &handle, work.clone(), 1, revision),
        )
        .await;
    let assignment = work_of(&missing_station);
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(
        assignment.reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    assert_eq!(assignment.work_units_done, 0);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:cod" && *count == 1),
        "a table cannot consume campfire food"
    );

    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            state_with_props(
                &fixture.blocks,
                "campfire",
                &[
                    ("facing", "north"),
                    ("lit", "false"),
                    ("signal_fire", "false"),
                    ("waterlogged", "false"),
                ],
            ),
        )
        .expect("fixture station remains loaded");
    let unlit = fixture
        .execute(
            &mut storage,
            &work_request(
                "cook-cod-unlit-campfire",
                &handle,
                work.clone(),
                1,
                assignment.revision,
            ),
        )
        .await;
    let assignment = work_of(&unlit);
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(
        assignment.reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    assert_eq!(assignment.work_units_done, 0);

    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            state_with_props(
                &fixture.blocks,
                "campfire",
                &[
                    ("facing", "north"),
                    ("lit", "true"),
                    ("signal_fire", "false"),
                    ("waterlogged", "false"),
                ],
            ),
        )
        .expect("fixture station remains loaded");
    let cooked = fixture
        .execute(
            &mut storage,
            &work_request("cook-cod-campfire", &handle, work, 1, assignment.revision),
        )
        .await;
    let assignment = work_of(&cooked);
    assert_eq!(
        assignment.state,
        ScriptWorkState::Committed,
        "{assignment:?}"
    );
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([("minecraft:cod", -1), ("minecraft:cooked_cod", 1)])
    );
}

/// (CP-012) A real container input returns its recipe remainder beside the
/// result; all output entries belong to the same durable craft receipt.
#[tokio::test]
async fn craft_returns_recipe_container_remainders_with_its_output() {
    let fixture = Fixture::new(false);
    fixture.seed_crafting_table();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(
        &mut storage,
        &handle,
        uuid,
        &[
            ("minecraft:milk_bucket", 1),
            ("minecraft:milk_bucket", 1),
            ("minecraft:milk_bucket", 1),
            ("minecraft:sugar", 2),
            ("minecraft:egg", 1),
            ("minecraft:wheat", 3),
        ],
    );

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "craft-cake-remainders",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:cake".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(
        assignment.state,
        ScriptWorkState::Committed,
        "{assignment:?}"
    );
    assert_eq!(assignment.work_units_done, 1);
    assert_eq!(
        assignment
            .changes
            .iter()
            .map(|change| (change.item_id.as_str(), change.delta))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            ("minecraft:bucket", 3),
            ("minecraft:cake", 1),
            ("minecraft:egg", -1),
            ("minecraft:milk_bucket", -3),
            ("minecraft:sugar", -2),
            ("minecraft:wheat", -3),
        ])
    );
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:bucket" && *count == 3),
        "the three milk buckets become three owned empty buckets"
    );
}

/// (CP-012) A full inventory keeps every ingredient and leaves no partial output
/// when a recipe can fill only part of an existing output stack.
#[tokio::test]
async fn craft_without_output_capacity_preserves_ingredients() {
    let fixture = Fixture::new(false);
    fixture.seed_crafting_table();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let mut carry_fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    carry_fill[0] = ("minecraft:oak_planks", 62);
    carry_fill[1] = ("minecraft:oak_log", 64);
    let revision = seed_gear_in(&mut storage, &handle, uuid, &carry_fill, false);
    let equipment_fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let revision = seed_gear_in(&mut storage, &handle, uuid, &equipment_fill, true).max(revision);

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "craft-full-output",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(assignment.work_units_done, 0);
    let contents = gear(&storage, &handle);
    assert_eq!(
        contents
            .iter()
            .filter(|(item, _)| item == "minecraft:oak_log")
            .map(|(_, count)| count)
            .sum::<u32>(),
        64,
        "the failed output placement restores the exact input stack"
    );
    assert_eq!(
        contents
            .iter()
            .filter(|(item, _)| item == "minecraft:oak_planks")
            .map(|(_, count)| count)
            .sum::<u32>(),
        62,
        "a failed partial merge leaves no phantom output"
    );
}

/// (CP-012) A failed output cannot simplify a component-bearing input during
/// rollback: the resident keeps its exact named stack and reports no delta.
#[tokio::test]
async fn craft_with_full_output_keeps_component_bearing_input_unchanged() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let carry_fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_CARRY_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let revision = seed_gear_in(&mut storage, &handle, uuid, &carry_fill, false);
    let equipment_fill: Vec<(&str, u32)> = (0..MAX_RESIDENT_EQUIPMENT_SLOTS)
        .map(|_| ("minecraft:stone", 64))
        .collect();
    let revision = seed_gear_in(&mut storage, &handle, uuid, &equipment_fill, true).max(revision);
    let mut record = storage
        .resident_orders()
        .record(&handle)
        .cloned()
        .expect("seeded resident record");
    record.carry[0]
        .as_mut()
        .expect("first carry stack")
        .custom_name = Some("owned stone".to_owned());
    let revision = storage
        .append_resident_order_change(DurableResidentOrderChange::Record {
            record: Box::new(record),
        })
        .expect("named input is durable")
        .max(revision);
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            state_of(&fixture.blocks, "stonecutter"),
        )
        .expect("fixture station remains loaded");

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "craft-named-stone-without-output-capacity",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:stone_slab_from_stone_stonecutting".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::NoStorage));
    assert_eq!(assignment.work_units_done, 0);
    assert!(assignment.changes.is_empty());
    let input = storage
        .resident_orders()
        .record(&handle)
        .and_then(|record| record.carry[0].as_ref())
        .expect("named source stays in its original slot");
    assert_eq!(input.item_id, "minecraft:stone");
    assert_eq!(input.count, 64);
    assert_eq!(input.custom_name.as_deref(), Some("owned stone"));
}

/// (CP-012) A craft pauses before consuming material when its named station
/// is absent, rather than treating an arbitrary loaded cell as a workshop.
#[tokio::test]
async fn craft_requires_the_named_crafting_table() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request(
                "craft-without-table",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: area([7, SURFACE_Y, 7], [7, SURFACE_Y, 7]),
                },
                1,
                revision,
            ),
        )
        .await;
    let assignment = work_of(&outcome);

    assert_eq!(outcome.failure(), None, "{outcome:?}");
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(
        assignment.reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    assert_eq!(assignment.work_units_done, 0);
    assert!(
        gear(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:oak_log" && *count == 1),
        "a missing table leaves the craft input with its resident owner"
    );
}
/// (A08) A job without its tool commits nothing and reports the typed reason;
/// with the tool it breaks the real block and reports only committed drops.
#[tokio::test]
async fn harvest_requires_its_tool_and_commits_real_drops() {
    let fixture = Fixture::new(false);
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 3, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let harvest = ScriptResidentWorkOrder::Harvest {
        area: area([2, 64, 2], [2, 64, 2]),
        tool: "minecraft:iron_hoe".to_owned(),
    };

    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("harvest-1", &handle, harvest.clone(), 4, 0),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.state, ScriptWorkState::Paused);
    assert_eq!(assignment.reason, Some(ScriptWorkPauseReason::MissingTool));
    assert_eq!(assignment.work_units_done, 0);
    assert!(assignment.changes.is_empty());
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_with_props(&fixture.blocks, "wheat", &[("age", "7")])),
        "no tool means the crop is untouched"
    );

    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let outcome = fixture
        .execute(
            &mut storage,
            &work_request("harvest-2", &handle, harvest, 4, revision),
        )
        .await;
    let assignment = work_of(&outcome);
    assert_eq!(assignment.reason, None, "{assignment:?}");
    assert!(assignment.work_units_done >= 1);
    assert!(
        assignment
            .changes
            .iter()
            .any(|change| change.item_id == "minecraft:wheat" && change.delta > 0),
        "the harvested crop is reported: {:?}",
        assignment.changes
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air")),
        "the crop really left the world"
    );
    assert!(
        gear(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:iron_hoe" && *count == 1),
        "the tool stays in the canonical slot"
    );
}

/// (CP-013) A post-commit station event resumes the same paused assignment
/// exactly once; another region cannot wake it, and completion cannot loop.
#[tokio::test]
async fn world_event_resumes_the_same_paused_craft_once() {
    let fixture = Fixture::new(false);
    fixture.seed_crafting_table();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 17, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let air = state_of(&fixture.blocks, "air");
    let crafting_table = state_of(&fixture.blocks, "crafting_table");
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            air,
        )
        .expect("fixture station stays loaded");
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "paused-craft",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    drop(storage);
    let mut storage = fixture.storage();
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .and_then(|record| record.work.as_ref())
            .map(|work| work.state),
        Some(ScriptWorkState::Paused),
        "the reopened Store retains the exact paused job"
    );

    let unrelated = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[8, 8]], |_| true)
        .await
        .expect("unrelated world event is bounded");
    assert!(unrelated.is_empty());
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,

                y: SURFACE_Y + 1,
                z: 9,
            },
            crafting_table,
        )
        .expect("fixture station restoration commits");

    let inactive_owner = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[0, 0]], |_| false)
        .await
        .expect("inactive owner leaves durable work paused");
    assert!(inactive_owner.is_empty());
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .and_then(|record| record.work.as_ref())
            .map(|work| work.state),
        Some(ScriptWorkState::Paused)
    );

    let resumed = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[0, 0]], |_| true)
        .await
        .expect("matching world event resumes work");
    assert_eq!(resumed.len(), 1);
    let assignment = work_of(&resumed[0].outcome);
    assert_eq!(assignment.state, ScriptWorkState::Committed);
    assert_eq!(assignment.reason, None);
    assert_eq!(assignment.work_units_done, 1);
    assert!(
        carry(&storage, &handle)
            .iter()
            .any(|(item, count)| item == "minecraft:oak_planks" && *count == 4)
    );

    let repeated = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[0, 0]], |_| true)
        .await
        .expect("repeated event does not reschedule committed work");
    assert!(repeated.is_empty());
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .and_then(|record| record.work.as_ref())
            .map(|work| work.done),
        Some(1)
    );
}

/// (CP-013) A growth update retries the same harvest that paused only because
/// its crop was immature; a later unchanged world event cannot harvest twice.
#[tokio::test]
async fn world_event_resumes_the_same_matured_harvest_once() {
    let fixture = Fixture::new(false);
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 23, Vec3::new(2.5, 64.0, 5.5))
        .await;
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 2, y: 64, z: 2 },
            state_with_props(&fixture.blocks, "wheat", &[("age", "6")]),
        )
        .expect("fixture field accepts an immature crop");
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-immature-harvest",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    drop(storage);
    let mut storage = fixture.storage();

    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 2, y: 64, z: 2 },
            state_with_props(&fixture.blocks, "wheat", &[("age", "7")]),
        )
        .expect("fixture field accepts the growth update");
    let resumed = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[0, 0]], |_| true)
        .await
        .expect("mature crop resumes the paused harvest");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air"))
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_chunks(
                &mut storage,
                "minecraft:overworld",
                &[[0, 0]],
                |_| true,
            )
            .await
            .expect("completed harvest cannot loop")
            .is_empty()
    );
}

/// (CP-013) A missing craft ingredient waits for the addressed inventory
/// transfer, not every changed block in the same active work region.
#[tokio::test]
async fn world_event_does_not_retry_inventory_missing_craft() {
    let fixture = Fixture::new(false);
    fixture.seed_crafting_table();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 24, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-missing-craft-input",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                0,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    let paused_revision = storage
        .resident_orders()
        .record(&handle)
        .expect("paused craft is durable")
        .revision;
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_chunks(
                &mut storage,
                "minecraft:overworld",
                &[[0, 0]],
                |_| true,
            )
            .await
            .expect("world wake stays bounded to source waits")
            .is_empty()
    );
    assert_eq!(
        storage
            .resident_orders()
            .record(&handle)
            .expect("world wake preserves the paused craft")
            .revision,
        paused_revision,
        "an unrelated world wake must not create a replacement receipt"
    );

    seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let resumed = fixture
        .runtime
        .resume_paused_work_for_inventory_change(
            &mut storage,
            std::slice::from_ref(&handle),
            &[],
            |_| true,
        )
        .await
        .expect("addressed inventory event resumes the craft");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
}

/// (CP-013) Cancelling a source wait removes its world wake classification
/// before the record reaches durable replay.
#[tokio::test]
async fn cancelling_a_world_input_wait_reopens_cleanly() {
    let fixture = Fixture::new(false);
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 25, Vec3::new(2.5, 64.0, 5.5))
        .await;
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 2, y: 64, z: 2 },
            state_with_props(&fixture.blocks, "wheat", &[("age", "6")]),
        )
        .expect("fixture field accepts an immature crop");
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-before-cancel",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingInput)
    );
    let paused_revision = storage
        .resident_orders()
        .record(&handle)
        .expect("paused work is durable")
        .revision;
    let cancel = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::CancelWork {
                operation_id: "cancel-world-wait".to_owned(),
                handle: handle.clone(),
                expected_revision: paused_revision,
            },
        },
    )
    .expect("valid cancel request");
    fixture.execute(&mut storage, &cancel).await;

    drop(storage);
    let reopened = fixture.storage();
    let record = reopened
        .resident_orders()
        .record(&handle)
        .expect("cancelled work survives storage replay");
    let work = record.work.as_ref().expect("cancelled work record");
    assert_eq!(work.state, ScriptWorkState::Cancelled);
    assert_eq!(work.reason, None);
    assert!(!work.world_input_wait);
    assert_eq!(work.revision, record.revision);
    assert_ne!(work.revision, 0);
}

/// (CP-013) A work target absent from the resident world stays unloaded until
/// the matching chunk is published again; the reload resumes that same record.
#[tokio::test]
async fn chunk_load_event_resumes_the_same_unloaded_craft_once() {
    let fixture = Fixture::new(false);
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 21, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let station = area([24, SURFACE_Y + 1, 8], [24, SURFACE_Y + 1, 8]);
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-unloaded-craft",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station,
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::Unloaded)
    );

    let air = state_of(&fixture.blocks, "air");
    let crafting_table = state_of(&fixture.blocks, "crafting_table");
    let mut loaded = Chunk::empty(
        ChunkPos { x: 1, z: 0 },
        air,
        Identifier::parse("minecraft:plains").expect("valid fixture biome"),
    );
    loaded.set_block(8, SURFACE_Y + 1, 8, crafting_table);
    fixture
        .world
        .lock()
        .await
        .commit_chunk_snapshot(ChunkPos { x: 1, z: 0 }, loaded)
        .expect("fixture publishes the reloaded chunk");

    let resumed = fixture
        .runtime
        .resume_paused_work_for_chunks(&mut storage, "minecraft:overworld", &[[1, 0]], |_| true)
        .await
        .expect("matching chunk load resumes craft");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_chunks(
                &mut storage,
                "minecraft:overworld",
                &[[1, 0]],
                |_| true,
            )
            .await
            .expect("completed work cannot resume twice")
            .is_empty()
    );
}

/// (CP-013) A resident inventory event wakes only that resident's blocked
/// prerequisite and preserves the original assignment watermark.
#[tokio::test]
async fn inventory_event_resumes_the_same_paused_harvest_once() {
    let fixture = Fixture::new(false);
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 18, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let harvest = ScriptResidentWorkOrder::Harvest {
        area: area([2, 64, 2], [2, 64, 2]),
        tool: "minecraft:iron_hoe".to_owned(),
    };
    let paused = fixture
        .execute(
            &mut storage,
            &work_request("paused-harvest", &handle, harvest, 1, 0),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingTool)
    );

    seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let resumed = fixture
        .runtime
        .resume_paused_work_for_inventory_change(
            &mut storage,
            std::slice::from_ref(&handle),
            &[],
            |_| true,
        )
        .await
        .expect("resident inventory event resumes work");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air"))
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_inventory_change(
                &mut storage,
                std::slice::from_ref(&handle),
                &[],
                |_| true,
            )
            .await
            .expect("completed work cannot loop")
            .is_empty()
    );
}
/// (CP-013) A changed protection zone selects only the paused work region it
/// overlaps. Removing that protection resumes the original job once.
#[tokio::test]
async fn zone_change_resumes_the_same_protected_harvest_once() {
    let protected = Arc::new(AtomicBool::new(true));
    let fixture = Fixture::with_protection_gate(Arc::clone(&protected));
    fixture.seed_crop();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 22, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:iron_hoe", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "pause-protected-harvest",
                &handle,
                ScriptResidentWorkOrder::Harvest {
                    area: area([2, 64, 2], [2, 64, 2]),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::Protected)
    );
    let released_zone = ScriptAxisAlignedZone::try_new(
        "released-field",
        "minecraft:overworld",
        ScriptPosition::try_new(1.5, 63.0, 1.5).expect("valid zone minimum"),
        ScriptPosition::try_new(2.5, 65.0, 2.5).expect("valid zone maximum"),
    )
    .expect("valid released zone");
    let other_zone = ScriptAxisAlignedZone::try_new(
        "other-field",
        "minecraft:overworld",
        ScriptPosition::try_new(128.0, 63.0, 128.0).expect("valid zone minimum"),
        ScriptPosition::try_new(129.0, 65.0, 129.0).expect("valid zone maximum"),
    )
    .expect("valid unrelated zone");
    assert!(
        storage
            .resident_orders()
            .paused_records_for_zone(&other_zone)
            .is_empty(),
        "a non-overlapping zone cannot wake protected work"
    );

    protected.store(false, Ordering::Release);
    let resumed = fixture
        .runtime
        .resume_paused_work_for_zone(&mut storage, &released_zone, |_| true)
        .await
        .expect("removed protection resumes work");
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        work_of(&resumed[0].outcome).state,
        ScriptWorkState::Committed
    );
    assert_eq!(
        fixture.block(2, 64, 2).await,
        Some(state_of(&fixture.blocks, "air"))
    );
    assert!(
        fixture
            .runtime
            .resume_paused_work_for_zone(&mut storage, &released_zone, |_| true)
            .await
            .expect("completed protected work cannot loop")
            .is_empty()
    );
}

/// (CP-013) The storage actor sends the real native resumption receipt only
/// after its queued world event observes the restored station.
#[tokio::test]
async fn storage_actor_delivers_one_native_resume_receipt() {
    let fixture = Fixture::new(false);
    fixture.seed_crafting_table();
    let mut storage = fixture.storage();
    let (handle, uuid) = fixture
        .resident(&mut storage, 19, Vec3::new(2.5, 64.0, 5.5))
        .await;
    let air = state_of(&fixture.blocks, "air");
    let crafting_table = state_of(&fixture.blocks, "crafting_table");
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            air,
        )
        .expect("fixture station stays loaded");
    let revision = seed_gear(&mut storage, &handle, uuid, &[("minecraft:oak_log", 1)]);
    let paused = fixture
        .execute(
            &mut storage,
            &work_request(
                "actor-paused-craft",
                &handle,
                ScriptResidentWorkOrder::Craft {
                    recipe: "minecraft:oak_planks".to_owned(),
                    count: 1,
                    station: crafting_station(),
                },
                1,
                revision,
            ),
        )
        .await;
    assert_eq!(
        work_of(&paused).reason,
        Some(ScriptWorkPauseReason::MissingStation)
    );
    let receipt = storage
        .operation_receipt(OWNER, "actor-paused-craft")
        .cloned()
        .expect("paused assignment receipt is durable");
    storage
        .acknowledge_operation(&receipt)
        .expect("fixture consumes the original receipt before actor startup");
    fixture
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos {
                x: 9,
                y: SURFACE_Y + 1,
                z: 9,
            },
            crafting_table,
        )
        .expect("fixture station restoration commits");

    let (boundary, mut endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("event capacity"),
        NonZeroUsize::new(8).expect("command capacity"),
    );
    let manifest = ScriptPluginManifest::new(OWNER, OWNER, "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .validate()
        .expect("test owner manifest is valid");
    endpoint
        .register_plugin_routes(&manifest)
        .expect("test owner registration is live");
    let actor = PluginStorageHandle::start(
        storage,
        fixture.runtime.clone(),
        ScriptEventSink::new(boundary),
        ShutdownHandle::default(),
    );
    actor
        .wake_resident_work("minecraft:overworld", vec![[0, 0]])
        .await;
    let event = match tokio::time::timeout(Duration::from_secs(1), endpoint.recv_event()).await {
        Ok(Some(event)) => event,
        Ok(None) => panic!("storage actor closed before native resume delivery"),
        Err(_) => panic!(
            "native resume event is not delivered; storage actor failed: {}",
            actor.failed()
        ),
    };
    let ScriptEventKind::OperationResult {
        operation_id,
        outcome,
        ..
    } = event.kind()
    else {
        panic!(
            "expected native operation result, got {}",
            event.event_name()
        );
    };
    assert!(
        operation_id
            .as_deref()
            .is_some_and(|id| id.starts_with("native-resume-"))
    );
    assert_eq!(work_of(outcome).state, ScriptWorkState::Committed);
    assert_eq!(
        fixture.block(9, SURFACE_Y + 1, 9).await,
        Some(crafting_table)
    );
    drop(actor);
}
