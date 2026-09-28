//! Behavioral suite for the warehouse binding moved out of `settlement_tests.rs`
//! by CP-REFACTOR-030.
//!
//! Declared as a child of `settlement_tests`, so `use super::*` reuses its
//! private `Fixture` and request helpers without a second fixture or a widened
//! visibility surface.

use super::*;

#[tokio::test]
async fn manual_warehouse_binds_surveyed_container_and_reuses_handle() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 60, 0], [7, 67, 7]).unwrap();
    let survey = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    assert_eq!(survey.failure(), None, "survey: {survey:?}");
    let token = survey_of(&survey).survey_token;
    let position = [3, 64, 3];
    fixture
        .world
        .set_container(position, vec![ItemStack::new(BIRCH_LOG, 4)]);

    let bind = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual", &token, position),
        )
        .await;
    assert_eq!(bind.failure(), None, "bind: {bind:?}");
    let binding = warehouse_of(&bind);
    assert!(matches!(
        &binding.source,
        ScriptWarehouseSource::Manual { position: bound } if *bound == position
    ));
    assert_eq!(
        binding.handle,
        format!(
            "warehouse:{OWNER}:manual:{}:{}:{}",
            position[0], position[1], position[2]
        )
    );

    let read = fixture
        .runtime
        .execute_owned_inventory(
            &mut storage,
            OWNER,
            &warehouse_query_request(&binding.handle, Some(binding.revision)),
        )
        .await
        .expect("manual warehouse query reaches the durable boundary");
    assert_eq!(read.failure(), None, "read: {read:?}");

    let repeat = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual-again", &token, position),
        )
        .await;
    assert_eq!(repeat.failure(), None, "repeat: {repeat:?}");
    assert_eq!(warehouse_of(&repeat), binding, "repeat bind is idempotent");

    let foreign = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &bind_manual_warehouse_request("bind-manual-foreign", &token, position),
        )
        .await;
    assert_eq!(foreign.failure(), Some(ScriptOperationFailure::Forbidden));
}
#[tokio::test]
async fn manual_warehouse_refuses_stale_token_outside_claimed_missing_and_cross_kind() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let bounds = ScriptSurveyBounds::new([0, 60, 0], [7, 67, 7]).unwrap();
    let survey = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    assert_eq!(survey.failure(), None, "survey: {survey:?}");
    let token = survey_of(&survey).survey_token;
    let position = [3, 64, 3];
    fixture
        .world
        .set_container(position, vec![ItemStack::new(BIRCH_LOG, 4)]);

    let foreign_token = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &bind_manual_warehouse_request("bind-manual-foreign-token", &token, position),
        )
        .await;
    assert_eq!(
        foreign_token.failure(),
        Some(ScriptOperationFailure::Forbidden)
    );

    let outside = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual-outside", &token, [100, 64, 100]),
        )
        .await;
    assert_eq!(
        outside.failure(),
        Some(ScriptOperationFailure::InvalidRequest)
    );

    fixture.world.set_claimed(true);
    let claimed = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual-claimed", &token, position),
        )
        .await;
    assert_eq!(claimed.failure(), Some(ScriptOperationFailure::Forbidden));
    fixture.world.set_claimed(false);

    fixture
        .world
        .set_availability(ScriptChunkAvailability::Unloaded);
    let unloaded = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual-unloaded", &token, position),
        )
        .await;
    assert_eq!(unloaded.failure(), Some(ScriptOperationFailure::Unloaded));
    fixture
        .world
        .set_availability(ScriptChunkAvailability::Loaded);

    fixture.world.clear_container(position);
    let missing = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual-missing", &token, position),
        )
        .await;
    assert_eq!(missing.failure(), Some(ScriptOperationFailure::NotFound));
    fixture
        .world
        .set_container(position, vec![ItemStack::new(BIRCH_LOG, 4)]);

    fixture.world.mark_change();
    let stale = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual-stale", &token, position),
        )
        .await;
    assert_eq!(stale.failure(), Some(ScriptOperationFailure::StaleRevision));
    // Stale token plus unloaded target must still answer `Unloaded`: the
    // availability fence runs before the freshness fence.
    fixture
        .world
        .set_availability(ScriptChunkAvailability::Unloaded);
    let stale_unloaded = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual-stale-unloaded", &token, position),
        )
        .await;
    assert_eq!(
        stale_unloaded.failure(),
        Some(ScriptOperationFailure::Unloaded)
    );
    fixture
        .world
        .set_availability(ScriptChunkAvailability::Loaded);

    let nether = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:the_nether", bounds),
        )
        .await;
    assert_eq!(nether.failure(), None, "nether survey: {nether:?}");
    let nether_token = survey_of(&nether).survey_token;
    fixture
        .world
        .set_container(position, vec![ItemStack::new(BIRCH_LOG, 4)]);
    let dimension = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual-nether", &nether_token, position),
        )
        .await;
    assert_eq!(
        dimension.failure(),
        Some(ScriptOperationFailure::RuntimeUnavailable)
    );
}
#[tokio::test]
async fn manual_bind_refuses_position_held_by_authored_binding() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let structure = prepared_warehouse(&fixture, &mut storage, "prepare-wh-manual").await;
    let position = container_position(&fixture, 1);
    fixture
        .world
        .set_container(position, vec![ItemStack::new(BIRCH_LOG, 1)]);
    let authored = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_warehouse_request("bind-authored-first", &structure.structure_id, 0),
        )
        .await;
    assert_eq!(authored.failure(), None, "authored bind: {authored:?}");

    let bounds = ScriptSurveyBounds::new(
        [position[0] - 3, position[1] - 3, position[2] - 3],
        [position[0] + 3, position[1] + 3, position[2] + 3],
    )
    .unwrap();
    let survey = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", bounds),
        )
        .await;
    assert_eq!(survey.failure(), None, "survey: {survey:?}");
    let token = survey_of(&survey).survey_token;
    let manual = fixture
        .execute(
            &mut storage,
            OWNER,
            &bind_manual_warehouse_request("bind-manual-cross", &token, position),
        )
        .await;
    assert_eq!(manual.failure(), None, "cross-kind bind: {manual:?}");
    assert_eq!(
        warehouse_of(&manual),
        warehouse_of(&authored),
        "one physical container keeps one handle across kinds"
    );
}
