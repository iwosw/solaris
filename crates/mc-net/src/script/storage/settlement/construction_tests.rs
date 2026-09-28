//! Staged construction behavioral suite moved out of `settlement_tests.rs` by
//! CP-REFACTOR-040 sub-slice 4.
//!
//! Declared as a child of `settlement_tests`, so `use super::*` reuses its
//! private `Fixture` and request helpers.

use super::*;

#[tokio::test]
async fn prepare_builds_nothing_and_cancel_preserves_built_blocks() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-1")
        .await
        .0;

    // Preparing plans work and spends nothing.
    assert_eq!(fixture.world.built_total(), 0);
    assert!(structure.consumed.is_empty());
    assert_eq!(structure.watermark, 0);
    assert_eq!(structure.current_stage_index, 0);
    assert_eq!(structure.completed_work_units, 0);
    assert_eq!(
        structure.stages.len(),
        2,
        "one plan per stage: {structure:?}"
    );
    assert_eq!(structure.stages[0].stage, "foundation");
    assert_eq!(structure.stages[0].block_count, 3);
    assert_eq!(structure.stages[1].stage, "walls");
    assert_eq!(structure.stages[1].block_count, 2);
    assert_eq!(
        structure
            .stages
            .iter()
            .map(|stage| stage.work_units)
            .sum::<u64>(),
        5
    );

    // Cancelling a structure that never spent anything leaves the world empty.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request(
                "cancel-prepared",
                &structure.structure_id,
                structure.revision,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "cancel: {outcome:?}");
    let cancelled = structure_of(&outcome);
    assert_eq!(cancelled.state, ScriptStructureState::Cancelled);
    assert!(cancelled.consumed.is_empty());
    assert_eq!(fixture.world.built_total(), 0);

    // A second structure builds a partial portion and then cancels: the
    // committed blocks stay in the world and the unspent remainder returns.
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-2")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved = reserve_bound_warehouse_plan(&fixture, &mut storage, "res-cottage", &plan).await;
    assert_eq!(reserved.remaining_total(), 5);
    assert_invariant(&reserved.quantities);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-partial",
                &structure.structure_id,
                &structure.stages[0].stage,
                &reserved.reservation_ref,
                structure.revision,
                2,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "advance: {outcome:?}");
    let receipt = receipt_of(&outcome);
    assert_eq!(receipt.block_count, 2);
    assert_eq!(receipt.work_units, 2);
    assert_eq!(receipt.stage, "foundation");
    assert_eq!(receipt.sequence, 1);
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 2);

    let status = fixture
        .execute(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await;
    let running = structure_of(&status);
    assert_eq!(running.state, ScriptStructureState::Running);
    assert_eq!(running.consumed[0].quantity, 2, "two stone were spent");
    assert_eq!(running.current_stage_index, 0);
    assert_eq!(running.completed_work_units, 2);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-running", &structure.structure_id, running.revision),
        )
        .await;
    assert_eq!(outcome.failure(), None, "cancel: {outcome:?}");
    let cancelled = structure_of(&outcome);
    assert_eq!(cancelled.state, ScriptStructureState::Cancelled);
    assert!(
        cancelled.remaining.is_empty(),
        "a cancelled structure holds no remainder"
    );
    assert_eq!(
        fixture.world.built_blocks(&structure.structure_id),
        2,
        "cancel never removes built blocks"
    );

    let (_, projection) = storage
        .settlement_reservation(OWNER, &reserved.reservation_ref)
        .expect("the reservation stays projected");
    assert!(projection.released);
    assert_eq!(projection.remaining_total(), 0);
    assert_invariant(&projection.quantities);
    assert_eq!(projection.quantity("minecraft:stone").consumed, 2);
    assert_eq!(projection.quantity("minecraft:stone").returned, 1);
    assert_eq!(projection.quantity("minecraft:oak_planks").returned, 2);
}

#[tokio::test]
async fn pause_resume_requires_the_accepted_footprint_and_releases_only_unspent_materials() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-resume")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved = reserve_bound_warehouse_plan(&fixture, &mut storage, "res-resume", &plan).await;

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &pause_request(
                "pause-prepared",
                &structure.structure_id,
                structure.revision,
            ),
        )
        .await;
    let paused = structure_of(&outcome);
    assert_eq!(paused.state, ScriptStructureState::Paused);
    assert_eq!(paused.pause_reason, None);

    let resume = resume_request("resume-prepared", &structure.structure_id, paused.revision);
    let outcome = fixture.execute(&mut storage, OWNER, &resume).await;
    let prepared = structure_of(&outcome);
    assert_eq!(prepared.state, ScriptStructureState::Prepared);
    assert!(prepared.consumed.is_empty());
    assert_eq!(
        fixture.execute(&mut storage, OWNER, &resume).await,
        outcome,
        "a repeated resume returns the accepted status rather than a new intent"
    );

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-resume",
                &structure.structure_id,
                &structure.stages[0].stage,
                &reserved.reservation_ref,
                prepared.revision,
                2,
            ),
        )
        .await;
    let receipt = receipt_of(&outcome);
    assert_eq!(receipt.block_count, 2);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &pause_request("pause-running", &structure.structure_id, receipt.revision),
        )
        .await;
    let paused = structure_of(&outcome);
    assert_eq!(paused.state, ScriptStructureState::Paused);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &resume_request("resume-running", &structure.structure_id, paused.revision),
        )
        .await;
    let running = structure_of(&outcome);
    assert_eq!(running.state, ScriptStructureState::Running);
    assert_eq!(running.consumed[0].quantity, 2);

    fixture.world.mark_change();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &pause_request("pause-changed", &structure.structure_id, running.revision),
        )
        .await;
    let paused = structure_of(&outcome);
    assert_eq!(paused.pause_reason.as_deref(), Some("site_changed"));
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &resume_request("resume-changed", &structure.structure_id, paused.revision),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-resume", &structure.structure_id, paused.revision),
        )
        .await;
    let cancelled = structure_of(&outcome);
    assert_eq!(cancelled.state, ScriptStructureState::Cancelled);
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 2);
    let (_, reservation) = storage
        .settlement_reservation(OWNER, &reserved.reservation_ref)
        .expect("cancelled structure leaves a durable reservation projection");
    assert_eq!(reservation.quantity("minecraft:stone").consumed, 2);
    assert_eq!(reservation.quantity("minecraft:stone").returned, 1);
    assert_eq!(reservation.quantity("minecraft:oak_planks").returned, 2);
    assert_invariant(&reservation.quantities);

    let repeated = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request(
                "cancel-resume-again",
                &structure.structure_id,
                cancelled.revision,
            ),
        )
        .await;
    assert_eq!(repeated.failure(), Some(ScriptOperationFailure::Blocked));
}

#[tokio::test]
async fn paused_construction_reopens_then_resumes_or_cancels_once() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-pause-reopen")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved =
        reserve_bound_warehouse_plan(&fixture, &mut storage, "res-pause-reopen", &plan).await;
    let advanced = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-pause-reopen",
                &structure.structure_id,
                &structure.stages[0].stage,
                &reserved.reservation_ref,
                structure.revision,
                2,
            ),
        )
        .await;
    let receipt = receipt_of(&advanced);
    let paused = fixture
        .execute(
            &mut storage,
            OWNER,
            &pause_request("pause-reopen", &structure.structure_id, receipt.revision),
        )
        .await;
    let paused = structure_of(&paused);
    drop(storage);

    let (runtime, _) = fixture.reopened();
    let mut storage = fixture.storage();
    let status = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await
        .unwrap();
    assert_eq!(
        structure_of(&status),
        paused,
        "pause survives storage reopen"
    );
    let resumed = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &resume_request("resume-reopen", &structure.structure_id, paused.revision),
        )
        .await
        .unwrap();
    let running = structure_of(&resumed);
    assert_eq!(running.state, ScriptStructureState::Running);
    assert_eq!(
        running.completed_work_units, 2,
        "resume creates no new portion"
    );
    let cancel = cancel_request("cancel-reopen", &structure.structure_id, running.revision);
    let cancelled = runtime
        .execute_settlement_operation(&mut storage, OWNER, &cancel)
        .await
        .unwrap();
    let cancelled = structure_of(&cancelled);
    assert_eq!(cancelled.state, ScriptStructureState::Cancelled);
    let (_, reservation) = storage
        .settlement_reservation(OWNER, &reserved.reservation_ref)
        .expect("cancelled reservation survives reopen");
    assert_eq!(reservation.quantity("minecraft:stone").consumed, 2);
    assert_eq!(reservation.quantity("minecraft:stone").returned, 1);
    assert_eq!(reservation.quantity("minecraft:oak_planks").returned, 2);
    assert_invariant(&reservation.quantities);
    let replay = runtime
        .execute_settlement_operation(&mut storage, OWNER, &cancel)
        .await
        .unwrap();
    assert_eq!(structure_of(&replay), cancelled);
}
#[tokio::test]
async fn prepare_plans_a_body_only_blueprint_as_one_stage() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    let token = survey_of(&outcome).survey_token;
    let mut anchor = fixture.anchor();
    anchor[1] += 8;

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("prepare-body", "solaris:tower", anchor, &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), None, "prepare: {outcome:?}");
    let structure = structure_of(&outcome);
    assert_eq!(structure.stages.len(), 1);
    assert_eq!(structure.stages[0].stage, "body");
    assert_eq!(structure.stages[0].block_count, 1);
    assert_eq!(structure.stages[0].work_units, 1);
    assert_eq!(structure.stages[0].materials.len(), 1);
    assert_eq!(structure.stages[0].materials[0].resource, "minecraft:stone");
    assert_eq!(structure.stages[0].materials[0].quantity, 1);
}

#[tokio::test]
async fn advance_replays_after_reopen_without_a_second_portion() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-3")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved = reserve_bound_warehouse_plan(&fixture, &mut storage, "res-replay", &plan).await;
    let advance = advance_request(
        "advance-foundation",
        &structure.structure_id,
        &structure.stages[0].stage,
        &reserved.reservation_ref,
        structure.revision,
        512,
    );
    let outcome = fixture.execute(&mut storage, OWNER, &advance).await;
    assert_eq!(outcome.failure(), None, "advance: {outcome:?}");
    let receipt = receipt_of(&outcome);
    assert_eq!(receipt.block_count, 3, "the whole first stage commits");
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 3);

    let (_, projected) = storage
        .settlement_reservation(OWNER, &reserved.reservation_ref)
        .expect("the reservation stays projected");
    assert_eq!(projected.receipt_watermark, 1);
    assert_eq!(
        projected.bound_to.as_deref(),
        Some(structure.structure_id.as_str())
    );
    assert_eq!(projected.quantity("minecraft:stone").consumed, 3);
    assert_invariant(&projected.quantities);

    // Reopen the journal from the same directory: the settlement ledger, the
    // structure, and the reservation projection all rebuild.
    storage.force_compact_for_test().unwrap();
    drop(storage);
    let (runtime, world) = fixture.reopened();
    let mut storage = fixture.storage();
    let (_, rebuilt) = storage
        .settlement_reservation(OWNER, &reserved.reservation_ref)
        .expect("the projection survives a reopen");
    assert_eq!(rebuilt, projected, "the reopened projection is identical");

    let replay = runtime
        .execute_settlement_operation(&mut storage, OWNER, &advance)
        .await
        .unwrap();
    assert_eq!(replay, outcome, "the same operation id returns its receipt");
    assert_eq!(receipt_of(&replay), receipt);
    assert_eq!(
        world.built_total(),
        0,
        "a replayed advance applies no second portion"
    );

    // The reopened ledger still serves the structure's status.
    let status = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await
        .unwrap();
    let running = structure_of(&status);
    assert_eq!(running.state, ScriptStructureState::Running);
    assert_eq!(running.watermark, 1);
    assert_eq!(running.consumed[0].quantity, 3);

    // A changed fingerprint under the same operation id conflicts.
    let conflicting = advance_request(
        "advance-foundation",
        &structure.structure_id,
        &structure.stages[1].stage,
        &reserved.reservation_ref,
        structure.revision,
        512,
    );
    let outcome = runtime
        .execute_settlement_operation(&mut storage, OWNER, &conflicting)
        .await
        .unwrap();
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
}

#[tokio::test]
async fn structure_portion_recovers_its_receipt_after_storage_append_fails() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-recover")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved = reserve_bound_warehouse_plan(&fixture, &mut storage, "res-recover", &plan).await;
    let advance = advance_request(
        "advance-recover",
        &structure.structure_id,
        &structure.stages[0].stage,
        &reserved.reservation_ref,
        structure.revision,
        512,
    );

    // The world half has accepted and journaled its after-image, but the
    // storage projection loses its first append attempt.
    storage.inject_fault_for_test(StorageFaultPoint::Write);
    assert!(
        fixture
            .runtime
            .execute_settlement_operation(&mut storage, OWNER, &advance)
            .await
            .is_err()
    );
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 3);
    let Fixture {
        root,
        runtime: original_runtime,
        world: original_world,
        catalog,
        sessions,
    } = fixture;
    drop(storage);
    drop(original_runtime);
    drop(original_world);
    drop(sessions);

    let sessions = Arc::new(SessionRegistry::new());
    let world = Arc::new(FakeWorld::new().journal_warehouse_transfers(Arc::clone(&sessions)));
    let runtime = runtime_at_with(
        Arc::clone(&catalog),
        Arc::clone(&world),
        sessions,
        Some(root.path()),
        true,
        true,
    );
    let mut storage = PluginStorage::open(root.path()).unwrap();
    runtime
        .recover(&mut storage)
        .expect("the durable world receipt restores its storage projection");

    let replay = runtime
        .execute_settlement_operation(&mut storage, OWNER, &advance)
        .await
        .expect("the recovered operation remains replayable");
    assert_eq!(replay.failure(), None, "recovered receipt: {replay:?}");
    assert_eq!(receipt_of(&replay).block_count, 3);
    assert_eq!(
        world.built_total(),
        0,
        "replaying a recovered receipt submits no second world portion"
    );
    let (_, reservation) = storage
        .settlement_reservation(OWNER, &reserved.reservation_ref)
        .expect("the reservation projection recovered with the receipt");
    assert_eq!(reservation.quantity("minecraft:stone").consumed, 3);
    assert_invariant(&reservation.quantities);
}

#[tokio::test]
async fn structure_portion_refuses_missing_reserved_material_without_building() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-underfunded")
        .await
        .0;
    let plan = plan_of(&structure);
    let mut quantities = resource_plan_totals(&plan)
        .unwrap()
        .into_iter()
        .map(|(resource, quantity)| {
            ScriptInventoryReservationQuantity::new(resource, quantity, 0, 0, quantity)
        })
        .collect::<Vec<_>>();
    let stone = quantities
        .iter_mut()
        .find(|quantity| quantity.resource_id == "minecraft:stone")
        .expect("cottage foundation needs stone");
    stone.returned = 1;
    stone.remaining -= 1;
    let reserved = reserve_plan_with_quantities(&mut storage, "res-underfunded", &plan, quantities);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-underfunded",
                &structure.structure_id,
                &structure.stages[0].stage,
                "res-underfunded",
                structure.revision,
                structure.stages[0].work_units,
            ),
        )
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::InsufficientItems),
        "a portion cannot turn a missing reserved material into blocks"
    );
    assert_eq!(fixture.world.built_total(), 0);
    let status = fixture
        .execute(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await;
    assert_eq!(structure_of(&status), structure);
    let (_, projection) = storage
        .settlement_reservation(OWNER, "res-underfunded")
        .expect("the refused portion leaves its reservation untouched");
    assert_eq!(projection, reserved);
    assert_invariant(&projection.quantities);
}

#[tokio::test]
async fn competing_structure_portions_admit_only_one_revision() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-competing")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved =
        reserve_bound_warehouse_plan(&fixture, &mut storage, "res-competing", &plan).await;
    let first = advance_request(
        "advance-competing-a",
        &structure.structure_id,
        &structure.stages[0].stage,
        &reserved.reservation_ref,
        structure.revision,
        1,
    );
    let competing = advance_request(
        "advance-competing-b",
        &structure.structure_id,
        &structure.stages[0].stage,
        &reserved.reservation_ref,
        structure.revision,
        1,
    );

    let accepted = fixture.execute(&mut storage, OWNER, &first).await;
    assert_eq!(accepted.failure(), None, "first portion: {accepted:?}");
    let refused = fixture.execute(&mut storage, OWNER, &competing).await;
    assert_eq!(
        refused.failure(),
        Some(ScriptOperationFailure::StaleRevision),
        "the second contender cannot reuse the consumed structure revision"
    );
    let current = structure_of(&refused);
    assert_eq!(current.revision, receipt_of(&accepted).revision);
    assert_eq!(current.current_stage_index, 0);
    assert_eq!(current.completed_work_units, 1);
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 1);
    let (_, reservation) = storage
        .settlement_reservation(OWNER, &reserved.reservation_ref)
        .expect("the accepted portion stays projected");
    assert_eq!(reservation.quantity("minecraft:stone").consumed, 1);
    assert_invariant(&reservation.quantities);
}

#[tokio::test]
async fn advance_pauses_when_the_reserved_site_changed() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-4")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved = reserve_bound_warehouse_plan(&fixture, &mut storage, "res-paused", &plan).await;

    fixture.world.mark_change();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-changed",
                &structure.structure_id,
                &structure.stages[0].stage,
                &reserved.reservation_ref,
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "advance: {outcome:?}");
    let paused = structure_of(&outcome);
    assert_eq!(paused.state, ScriptStructureState::Paused);
    assert_eq!(paused.pause_reason.as_deref(), Some("site_changed"));
    assert!(paused.consumed.is_empty());
    assert_eq!(fixture.world.built_total(), 0, "a pause builds nothing");

    let (_, projection) = storage
        .settlement_reservation(OWNER, &reserved.reservation_ref)
        .expect("the reservation stays projected");
    assert_eq!(
        projection.quantities, reserved.quantities,
        "nothing was consumed"
    );
    for quantity in &projection.quantities {
        assert_eq!(quantity.remaining, quantity.reserved);
    }

    // A paused structure can still be cancelled, and the cancel is terminal.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-paused", &structure.structure_id, paused.revision),
        )
        .await;
    assert_eq!(
        structure_of(&outcome).state,
        ScriptStructureState::Cancelled
    );
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &cancel_request("cancel-again", &structure.structure_id, paused.revision),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
}

#[tokio::test]
async fn advance_absorbs_its_own_durable_commits_but_a_foreign_edit_still_pauses() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-own")
        .await
        .0;
    let plan = plan_of(&structure);
    let reserved = reserve_bound_warehouse_plan(&fixture, &mut storage, "res-own", &plan).await;

    // First portion: two of the foundation's three cells. Applying it is a
    // durable world commit of this structure's own work, which advances the
    // world revision exactly like the live adapter's journal decision does.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-own-1",
                &structure.structure_id,
                &structure.stages[0].stage,
                &reserved.reservation_ref,
                structure.revision,
                2,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "first advance: {outcome:?}");
    assert_eq!(receipt_of(&outcome).block_count, 2);
    let revision = receipt_of(&outcome).revision;

    // Second portion completes the same stage. The previous portion's own
    // durable commit must not be mistaken for a site change: no foreign edit
    // happened, so this advance must build, not pause.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-own-2",
                &structure.structure_id,
                &structure.stages[0].stage,
                &reserved.reservation_ref,
                revision,
                2,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "second advance: {outcome:?}");
    assert_eq!(receipt_of(&outcome).block_count, 1);
    let revision = receipt_of(&outcome).revision;
    let status = fixture
        .execute(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await;
    let running = structure_of(&status);
    assert_eq!(running.state, ScriptStructureState::Running);
    assert_eq!(running.pause_reason, None);
    assert_eq!(fixture.world.built_blocks(&structure.structure_id), 3);

    // A durable change this structure did not make must still pause the build.
    fixture.world.mark_change();
    let next_stage = structure
        .stages
        .get(1)
        .expect("the cottage has a second stage to advance");
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-own-3",
                &structure.structure_id,
                &next_stage.stage,
                &reserved.reservation_ref,
                revision,
                2,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), None, "third advance: {outcome:?}");
    let paused = structure_of(&outcome);
    assert_eq!(paused.state, ScriptStructureState::Paused);
    assert_eq!(paused.pause_reason.as_deref(), Some("site_changed"));
    assert_eq!(
        fixture.world.built_blocks(&structure.structure_id),
        3,
        "the paused advance built nothing"
    );
}

#[tokio::test]
async fn prepare_refuses_a_changed_preview_blueprint_without_reserving_the_site() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let survey = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    let token = survey_of(&survey).survey_token;
    let prepare = |operation_id: &str, hash: String| {
        settlement(ScriptSettlementOperation::PrepareStructure {
            operation_id: operation_id.to_owned(),
            site_id: site_id_for(fixture.anchor()),
            blueprint_id: COTTAGE.to_owned(),
            anchor: fixture.anchor(),
            rotation: 0,
            survey_token: token.clone(),
            expected_site_revision: 0,
            expected_blueprint_hash: Some(hash),
        })
    };
    let changed = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare("preview-stale", "0".repeat(64)),
        )
        .await;
    assert_eq!(
        changed.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert!(storage.settlements().active_structures(OWNER).is_empty());

    let hash = fixture
        .catalog
        .get(COTTAGE)
        .unwrap()
        .content_hash()
        .to_owned();
    let matching = fixture
        .execute(&mut storage, OWNER, &prepare("preview-matching", hash))
        .await;
    assert_eq!(matching.failure(), None, "matching preview: {matching:?}");
    match settlement_payload(&matching) {
        ScriptSettlementResult::Structure { structure } => {
            assert_eq!(structure.blueprint_id, COTTAGE);
        }
        other => panic!("expected planned structure, got {other:?}"),
    }
}

#[tokio::test]
async fn prepare_rejects_stale_and_foreign_survey_tokens() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    assert_eq!(outcome.failure(), None, "survey: {outcome:?}");
    let survey = survey_of(&outcome);
    assert_eq!(survey.usable_plots, 64);
    assert_eq!(survey.chunk_availability, ScriptChunkAvailability::Loaded);
    assert_eq!(survey.revision, 1);
    let token = survey.survey_token;

    // The surveyed footprint changed under the token.
    fixture.world.mark_change();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("stale", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );

    // A token is owner scoped.
    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &prepare_request("foreign", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    // An unknown token is not found rather than assumed valid.
    let unknown = "0".repeat(64);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("unknown", COTTAGE, fixture.anchor(), &unknown, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));

    // An unknown blueprint is not found.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request(
                "unknown-blueprint",
                "solaris:keep",
                fixture.anchor(),
                &token,
                0,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));
}

#[tokio::test]
async fn prepare_rejects_claimed_and_overlapping_footprints() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    let token = survey_of(&outcome).survey_token;

    fixture.world.set_claimed(true);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("claimed", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
    fixture.world.set_claimed(false);

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("first", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), None, "prepare: {outcome:?}");

    // The same footprint is already reserved by this plugin.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("overlap", COTTAGE, fixture.anchor(), &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));

    // A stale site revision is refused.
    let mut anchor = fixture.anchor();
    anchor[1] += 8;
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("stale-site", COTTAGE, anchor, &token, 7),
        )
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
}

#[tokio::test]
async fn prepare_refuses_a_footprint_the_terrain_rises_into() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    let token = survey_of(&outcome).survey_token;
    let anchor = fixture.anchor();

    // Terrain above the base row would be overwritten by the structure body, so
    // the placement is refused and reserves nothing.
    fixture.world.set_opaque_y(Some(anchor[1] + 1));
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("rises-into", COTTAGE, anchor, &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
    assert!(
        storage.settlements().active_structures(OWNER).is_empty(),
        "a refused placement reserves nothing"
    );

    // Terrain level with the base row is the ground the structure stands on.
    fixture.world.set_opaque_y(Some(anchor[1]));
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request("flush", COTTAGE, anchor, &token, 0),
        )
        .await;
    assert_eq!(outcome.failure(), None, "flush prepare: {outcome:?}");
    assert_eq!(structure_of(&outcome).state, ScriptStructureState::Prepared);

    // An unloaded footprint can never prove the volume is free.
    fixture.world.set_opaque_y(None);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request(
                "unloaded",
                COTTAGE,
                [anchor[0] + 4, anchor[1], anchor[2]],
                &token,
                0,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Unloaded));
}

#[tokio::test]
async fn advance_rejects_a_reservation_with_a_mismatched_plan() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-5")
        .await
        .0;
    let other = ScriptInventoryResourcePlan::new(vec![ScriptInventoryWorkPortion::new(
        1,
        vec![ScriptInventoryMaterial::new(
            "minecraft:oak_planks".to_owned(),
            1,
        )],
    )]);
    let reserved = reserve_bound_warehouse_plan(&fixture, &mut storage, "res-other", &other).await;

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-mismatched",
                &structure.structure_id,
                &structure.stages[0].stage,
                &reserved.reservation_ref,
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
    assert_eq!(fixture.world.built_total(), 0);

    // A reservation that does not exist is not found.
    let unknown = "0".repeat(64);
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-unknown",
                &structure.structure_id,
                &structure.stages[0].stage,
                &unknown,
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));

    // A stage that is not the next one is an invalid request.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &advance_request(
                "advance-unknown-stage",
                &structure.structure_id,
                "cellar",
                &reserved.reservation_ref,
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::InvalidRequest)
    );
}

#[tokio::test]
async fn structures_are_owner_scoped_and_unknown_ids_are_not_found() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_cottage(&fixture, &mut storage, "prepare-6")
        .await
        .0;

    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &status_request(&structure.structure_id),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &cancel_request(
                "foreign-cancel",
                &structure.structure_id,
                structure.revision,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    let unknown = "0".repeat(64);
    let outcome = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &advance_request(
                "foreign-advance",
                &structure.structure_id,
                &structure.stages[0].stage,
                &unknown,
                structure.revision,
                512,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    let outcome = fixture
        .execute(&mut storage, OWNER, &status_request("forged-structure"))
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));

    // The owner still sees its structure unchanged.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &status_request(&structure.structure_id),
        )
        .await;
    let status = structure_of(&outcome);
    assert_eq!(status.state, ScriptStructureState::Prepared);
    assert_eq!(status.revision, structure.revision);
    assert_eq!(status.reserved_footprint, structure.reserved_footprint);
}

#[tokio::test]
async fn active_structure_limit_is_enforced() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 0, 0], [7, 3, 7]).unwrap();
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    let token = survey_of(&outcome).survey_token;
    let anchor = fixture.anchor();

    for index in 0..64 {
        let at = [anchor[0], mc_world::MIN_Y + 5 * index, anchor[2]];
        let outcome = fixture
            .execute(
                &mut storage,
                OWNER,
                &prepare_request(&format!("prepare-{index}"), COTTAGE, at, &token, 0),
            )
            .await;
        assert_eq!(outcome.failure(), None, "prepare {index}: {outcome:?}");
    }

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &prepare_request(
                "prepare-overflow",
                COTTAGE,
                [anchor[0], mc_world::MIN_Y + 330, anchor[2]],
                &token,
                0,
            ),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Capacity));
}
