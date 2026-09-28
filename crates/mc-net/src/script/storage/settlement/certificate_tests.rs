//! Certificate/recognition behavioral suite moved out of `settlement_tests.rs`
//! by CP-REFACTOR-040 sub-slice 3.
//!
//! Declared as a child of `settlement_tests`, so `use super::*` reuses its
//! private `Fixture` and request helpers.

use super::*;

fn recognize_request(
    operation_id: &str,
    footprint: ScriptSurveyBounds,
    survey_token: &str,
    expected_world_revision: u64,
) -> ScriptOperationRequest {
    settlement(ScriptSettlementOperation::RecognizeBuilding {
        operation_id: operation_id.to_owned(),
        request: Box::new(ScriptBuildingCertificateRequest::new(
            footprint,
            "solaris:smithy".to_owned(),
            vec![
                ScriptBuildingInteractionPoint::Entrance { at: [1, 61, 1] },
                ScriptBuildingInteractionPoint::Workstation {
                    at: [3, 61, 3],
                    capability: "minecraft:crafting_table".to_owned(),
                    workplaces: 1,
                },
                ScriptBuildingInteractionPoint::Storage { at: [4, 61, 4] },
            ],
            survey_token.to_owned(),
            expected_world_revision,
        )),
    })
}

fn certificate_of(outcome: &ScriptOperationOutcome) -> mc_script::ScriptBuildingCertificate {
    match settlement_payload(outcome) {
        ScriptSettlementResult::Building { certificate } => certificate.as_ref().clone(),
        other => panic!("expected a building certificate, got {other:?}"),
    }
}

/// A valid player-built volume recognizes through the live world reads and
/// re-recognition of the same normalized footprint is idempotent.
#[tokio::test]
async fn recognize_building_validates_live_points_and_reuses_certificate() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let footprint = ScriptSurveyBounds::new([0, 60, 0], [7, 67, 7]).unwrap();
    let survey = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", footprint),
        )
        .await;
    assert_eq!(survey.failure(), None, "survey: {survey:?}");
    let snapshot = survey_of(&survey);
    fixture
        .world
        .set_container([4, 61, 4], vec![ItemStack::new(BIRCH_LOG, 4)]);
    fixture
        .world
        .set_resident_block([3, 61, 3], "crafting_table");

    let recognized = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-1",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(recognized.failure(), None, "recognize: {recognized:?}");
    let certificate = certificate_of(&recognized);
    assert_eq!(certificate.footprint, footprint);
    assert_eq!(certificate.world_revision, snapshot.world_revision);

    let repeat = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-2",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(repeat.failure(), None, "repeat: {repeat:?}");
    assert_eq!(
        certificate_of(&repeat),
        certificate,
        "repeat recognition is idempotent"
    );

    let foreign = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &recognize_request(
                "recognize-foreign",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(foreign.failure(), Some(ScriptOperationFailure::Forbidden));
}
/// A certificate footprint strictly inside a larger survey still recognizes:
/// the staleness fence compares the surveyed token bounds, not the smaller
/// footprint digest.
#[tokio::test]
async fn recognize_building_accepts_a_sub_footprint_of_a_larger_survey() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let surveyed = ScriptSurveyBounds::new([0, 60, 0], [15, 67, 15]).unwrap();
    let survey = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", surveyed),
        )
        .await;
    assert_eq!(survey.failure(), None, "survey: {survey:?}");
    let snapshot = survey_of(&survey);
    fixture
        .world
        .set_container([4, 61, 4], vec![ItemStack::new(BIRCH_LOG, 4)]);
    fixture
        .world
        .set_resident_block([3, 61, 3], "crafting_table");
    let footprint = ScriptSurveyBounds::new([0, 60, 0], [7, 67, 7]).unwrap();
    let recognized = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-sub-footprint",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(recognized.failure(), None, "recognize: {recognized:?}");
    assert_eq!(certificate_of(&recognized).footprint, footprint);
}
/// `Status` answers the durable ledger the id names: a structure snapshot for
/// built workplaces, the immutable certificate for recognized ones. Foreign
/// ids stay forbidden and unknown ids stay not-found, exactly as for
/// structures.
#[tokio::test]
async fn status_answers_structure_and_certificate_through_one_query() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let footprint = ScriptSurveyBounds::new([0, 60, 0], [7, 67, 7]).unwrap();
    let survey = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", footprint),
        )
        .await;
    assert_eq!(survey.failure(), None, "survey: {survey:?}");
    let snapshot = survey_of(&survey);
    fixture
        .world
        .set_container([4, 61, 4], vec![ItemStack::new(BIRCH_LOG, 4)]);
    fixture
        .world
        .set_resident_block([3, 61, 3], "crafting_table");
    let recognized = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-status",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(recognized.failure(), None, "recognize: {recognized:?}");
    let certificate = certificate_of(&recognized);

    let status = fixture
        .execute(
            &mut storage,
            OWNER,
            &status_request(&certificate.certificate_id),
        )
        .await;
    assert_eq!(status.failure(), None, "certificate status: {status:?}");
    assert_eq!(certificate_of(&status), certificate);

    let foreign = fixture
        .execute(
            &mut storage,
            FOREIGN,
            &status_request(&certificate.certificate_id),
        )
        .await;
    assert_eq!(foreign.failure(), Some(ScriptOperationFailure::Forbidden));

    let unknown = fixture
        .execute(&mut storage, OWNER, &status_request("forged-certificate"))
        .await;
    assert_eq!(unknown.failure(), Some(ScriptOperationFailure::NotFound));
}

/// Recognition refuses a missing station, a missing container, a stale fence,
/// an unloaded footprint, and a namespaced capability alias.
#[tokio::test]
async fn recognize_building_refuses_invalid_live_points() {
    let fixture = Fixture::new();
    let mut storage = fixture.storage();
    let footprint = ScriptSurveyBounds::new([0, 60, 0], [7, 67, 7]).unwrap();
    let survey = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", footprint),
        )
        .await;
    assert_eq!(survey.failure(), None, "survey: {survey:?}");
    let snapshot = survey_of(&survey);

    let missing_station = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-missing-station",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(
        missing_station.failure(),
        Some(ScriptOperationFailure::Blocked)
    );
    fixture
        .world
        .set_resident_block([3, 61, 3], "crafting_table");

    let missing_container = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-missing-container",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(
        missing_container.failure(),
        Some(ScriptOperationFailure::NotFound)
    );
    fixture
        .world
        .set_container([4, 61, 4], vec![ItemStack::new(BIRCH_LOG, 4)]);

    fixture.world.set_unstandable([1, 61, 1]);
    let blocked_entrance = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-blocked-entrance",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(
        blocked_entrance.failure(),
        Some(ScriptOperationFailure::Blocked)
    );

    fixture.world.mark_change();
    let stale = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-stale",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(stale.failure(), Some(ScriptOperationFailure::StaleRevision));

    fixture
        .world
        .set_availability(ScriptChunkAvailability::Unloaded);
    // Unloaded footprint wins over the stale fence: the token is stale, but
    // the POI read fails first and the caller retries on load.
    let stale_unloaded = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-stale-unloaded",
                footprint,
                &snapshot.survey_token,
                snapshot.world_revision,
            ),
        )
        .await;
    assert_eq!(
        stale_unloaded.failure(),
        Some(ScriptOperationFailure::Unloaded)
    );
    fixture
        .world
        .set_availability(ScriptChunkAvailability::Loaded);
    // The earlier mark_change staled the original token; re-survey so the
    // remaining pins test their own check, not the fence.
    let resurvey = fixture
        .execute(
            &mut storage,
            OWNER,
            &survey_request("minecraft:overworld", footprint),
        )
        .await;
    assert_eq!(resurvey.failure(), None, "resurvey: {resurvey:?}");
    let fresh = survey_of(&resurvey);
    // A closed walk blocks recognition even though every cell is loaded and
    // standable: close all four neighbour walks into the station. The earlier
    // entrance refusal is cleared so this pin reaches the route fence.
    fixture.world.clear_unstandable();
    for neighbour in [[4, 61, 3], [2, 61, 3], [3, 61, 4], [3, 61, 2]] {
        fixture.world.set_blocked_route([1, 61, 1], neighbour);
    }
    let blocked_route = fixture
        .execute(
            &mut storage,
            OWNER,
            &recognize_request(
                "recognize-blocked-route",
                footprint,
                &fresh.survey_token,
                fresh.world_revision,
            ),
        )
        .await;
    assert_eq!(
        blocked_route.failure(),
        Some(ScriptOperationFailure::Blocked)
    );
    // The route pin must not leak into the alias check.
    fixture.world.clear_blocked_routes();
    let aliased = settlement(ScriptSettlementOperation::RecognizeBuilding {
        operation_id: "recognize-alias".to_owned(),
        request: Box::new(ScriptBuildingCertificateRequest::new(
            footprint,
            "solaris:smithy".to_owned(),
            vec![
                ScriptBuildingInteractionPoint::Entrance { at: [1, 61, 1] },
                ScriptBuildingInteractionPoint::Workstation {
                    at: [3, 61, 3],
                    capability: "solaris:crafting_table".to_owned(),
                    workplaces: 1,
                },
                ScriptBuildingInteractionPoint::Storage { at: [4, 61, 4] },
            ],
            fresh.survey_token.clone(),
            fresh.world_revision,
        )),
    });
    let aliased = fixture.execute(&mut storage, OWNER, &aliased).await;
    assert_eq!(aliased.failure(), Some(ScriptOperationFailure::Blocked));
}
