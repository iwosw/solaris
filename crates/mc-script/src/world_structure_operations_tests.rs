use super::*;

fn point(at: [i32; 3]) -> ScriptWorksiteInteractionPoint {
    ScriptWorksiteInteractionPoint::Container { at }
}
fn request(points: Vec<ScriptWorksiteInteractionPoint>) -> ScriptWorksiteRequest {
    ScriptWorksiteRequest::new(
        ScriptWorldBounds::new([0, 0, 0], [4, 4, 4]).unwrap(),
        points,
        "survey-token".to_owned(),
        1,
    )
}

#[test]
fn worksite_rejects_duplicate_and_outside_physical_points() {
    assert!(
        request(vec![point([1, 1, 1]), point([1, 1, 1])])
            .validate()
            .is_err()
    );
    assert!(request(vec![point([5, 1, 1])]).validate().is_err());
    assert!(
        request(vec![point([1, 1, 1]), point([2, 1, 1])])
            .validate()
            .is_ok()
    );
}

#[test]
fn worksite_check_cannot_claim_validity_with_missing_live_points() {
    let mut check = ScriptWorksiteCheck::new(
        "worksite".to_owned(),
        1,
        true,
        vec![ScriptWorksitePointFailure::new(
            [1, 1, 1],
            ScriptWorksiteFailure::MissingContainer,
        )],
    );
    assert!(check.validate().is_err());
    check.valid = false;
    assert!(check.validate().is_ok());
    check.blocked_points.clear();
    assert!(check.validate().is_err());
}

#[test]
fn unavailable_feature_contents_never_look_like_observed_facts() {
    let mut feature = ScriptWorldFeature::new(
        "feature".to_owned(),
        "minecraft:village".to_owned(),
        "minecraft:overworld".to_owned(),
        ScriptWorldBounds::new([0, 0, 0], [4, 4, 4]).unwrap(),
        false,
        vec![],
        vec![point([1, 1, 1])],
        vec![],
    );
    assert!(feature.validate().is_err());
    feature.contents_known = true;
    assert!(feature.validate().is_ok());
}

#[test]
fn broad_world_bounds_do_not_weaken_survey_admission() {
    let bounds = ScriptWorldBounds::new([0, 0, 0], [255, 20, 255]).unwrap();
    assert!(
        ScriptWorldStructureOperation::Survey {
            dimension: "minecraft:overworld".to_owned(),
            bounds,
        }
        .validate()
        .is_err()
    );
    let feature = ScriptWorldFeature::new(
        "feature".to_owned(),
        "minecraft:village".to_owned(),
        "minecraft:overworld".to_owned(),
        bounds,
        false,
        vec![],
        vec![],
        vec![],
    );
    assert!(feature.validate().is_ok());
}

#[test]
fn unloaded_block_queries_cannot_invent_physical_facts() {
    let mut query = ScriptWorldBlockQuery::new(
        1,
        vec![ScriptWorldBlockFacts::new(
            [0, 0, 0],
            None,
            vec![],
            false,
            false,
            false,
            false,
        )],
    );
    assert!(query.validate().is_ok());
    query.blocks[0].container = true;
    assert!(query.validate().is_err());
}
