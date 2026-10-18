use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

// Synthetic Solaris-authored fixtures only: they mirror the vanilla JSON
// shape without carrying any Mojang bytes.
fn worldgen_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    for sub in ["placed_feature", "configured_feature", "template_pool"] {
        fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    dir
}

fn write_json(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn id(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).unwrap()
}

fn flower_worldgen() -> TempDir {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/flower_fixture.json",
        r#"{
            "feature": "minecraft:flower_fixture",
            "placement": [
                { "type": "minecraft:count", "count": 64 },
                {
                    "type": "minecraft:random_offset",
                    "xz_spread": { "type": "minecraft:trapezoid", "min": -6, "max": 6, "plateau": 0 },
                    "y_spread": { "type": "minecraft:trapezoid", "min": -2, "max": 2, "plateau": 0 }
                },
                {
                    "type": "minecraft:block_predicate_filter",
                    "predicate": { "type": "minecraft:matching_block_tag", "tag": "minecraft:air" }
                }
            ]
        }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/flower_fixture.json",
        r#"{
            "type": "minecraft:simple_block",
            "config": {
                "to_place": {
                    "type": "minecraft:noise_threshold_provider",
                    "seed": 2345,
                    "noise": { "amplitudes": [1.0], "firstOctave": 0 },
                    "scale": 0.005,
                    "threshold": -0.8,
                    "high_chance": 0.33333334,
                    "default_state": { "Name": "minecraft:dandelion" },
                    "low_states": [
                        { "Name": "minecraft:orange_tulip" },
                        { "Name": "minecraft:red_tulip" }
                    ],
                    "high_states": [
                        { "Name": "minecraft:poppy" },
                        { "Name": "minecraft:azure_bluet" }
                    ]
                }
            }
        }"#,
    );
    dir
}

#[test]
fn simple_block_closure_resolves_provider_and_modifier_order() {
    let dir = flower_worldgen();
    let closure = FeatureClosure::new(dir.path());
    let spec = closure
        .load_placed_feature(&id("minecraft:flower_fixture"))
        .expect("fixture closure resolves");

    assert_eq!(spec.id, id("minecraft:flower_fixture"));
    assert_eq!(spec.feature.id, id("minecraft:flower_fixture"));
    assert_eq!(
        spec.placement,
        vec![
            PlacementModifierSpec::Count(IntProviderSpec::Constant(64)),
            PlacementModifierSpec::RandomOffset {
                xz_spread: IntProviderSpec::Trapezoid {
                    min_inclusive: -6,
                    max_inclusive: 6,
                    plateau: 0,
                },
                y_spread: IntProviderSpec::Trapezoid {
                    min_inclusive: -2,
                    max_inclusive: 2,
                    plateau: 0,
                },
            },
            PlacementModifierSpec::BlockPredicateFilter(BlockPredicateSpec::MatchingBlockTag {
                offset: (0, 0, 0),
                tag: id("minecraft:air"),
            }),
        ]
    );

    let ConfiguredFeatureKind::SimpleBlock {
        to_place,
        schedule_tick,
    } = &spec.feature.kind
    else {
        panic!("expected simple_block");
    };
    assert!(!schedule_tick);
    let StateProviderSpec::NoiseThreshold(noise) = to_place else {
        panic!("expected noise_threshold_provider");
    };
    assert_eq!(noise.seed, 2345);
    assert_eq!(
        noise.noise,
        NoiseParametersSpec {
            first_octave: 0,
            amplitudes: vec![1.0],
        }
    );
    assert_eq!(noise.scale, 0.005);
    assert_eq!(noise.threshold, -0.8);
    assert_eq!(noise.default_state.block, id("minecraft:dandelion"));
    assert_eq!(noise.low_states.len(), 2);
    assert_eq!(noise.high_states.len(), 2);
    assert_eq!(noise.high_states[1].block, id("minecraft:azure_bluet"));
}

#[test]
fn rotated_and_weighted_pile_providers_resolve() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/pile_fixture.json",
        r#"{ "feature": "minecraft:pile_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/pile_fixture.json",
        r#"{
            "type": "minecraft:block_pile",
            "config": {
                "state_provider": {
                    "type": "minecraft:rotated_block_provider",
                    "state": {
                        "Name": "minecraft:hay_block",
                        "Properties": { "axis": "y" }
                    }
                }
            }
        }"#,
    );
    let closure = FeatureClosure::new(dir.path());
    let spec = closure
        .load_placed_feature(&id("minecraft:pile_fixture"))
        .unwrap();
    let ConfiguredFeatureKind::BlockPile { state_provider } = &spec.feature.kind else {
        panic!("expected block_pile");
    };
    assert_eq!(
        state_provider,
        &StateProviderSpec::Rotated {
            block: id("minecraft:hay_block")
        }
    );
    assert!(spec.placement.is_empty());

    let weighted = closure
        .parse_state_provider(
            &id("minecraft:pile_fixture"),
            &serde_json::json!({
                "type": "minecraft:weighted_state_provider",
                "entries": [
                    { "data": { "Name": "minecraft:blue_ice" }, "weight": 1 },
                    { "data": { "Name": "minecraft:packed_ice" }, "weight": 5 }
                ]
            }),
        )
        .unwrap();
    let StateProviderSpec::Weighted { entries } = weighted else {
        panic!("expected weighted_state_provider");
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].weight, 1);
    assert_eq!(entries[1].state.block, id("minecraft:packed_ice"));
}

#[test]
fn cactus_column_resolves_layers_direction_and_predicate() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/patch_cactus_fixture.json",
        r#"{
            "feature": "minecraft:cactus_fixture",
            "placement": [
                { "type": "minecraft:count", "count": 10 },
                {
                    "type": "minecraft:block_predicate_filter",
                    "predicate": {
                        "type": "minecraft:all_of",
                        "predicates": [
                            { "type": "minecraft:matching_block_tag", "tag": "minecraft:air" },
                            {
                                "type": "minecraft:would_survive",
                                "state": { "Name": "minecraft:cactus", "Properties": { "age": "0" } }
                            }
                        ]
                    }
                }
            ]
        }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/cactus_fixture.json",
        r#"{
            "type": "minecraft:block_column",
            "config": {
                "allowed_placement": { "type": "minecraft:matching_block_tag", "tag": "minecraft:air" },
                "direction": "up",
                "layers": [
                    {
                        "height": { "type": "minecraft:biased_to_bottom", "min_inclusive": 1, "max_inclusive": 3 },
                        "provider": {
                            "type": "minecraft:simple_state_provider",
                            "state": { "Name": "minecraft:cactus", "Properties": { "age": "0" } }
                        }
                    },
                    {
                        "height": {
                            "type": "minecraft:weighted_list",
                            "distribution": [
                                { "data": 0, "weight": 3 },
                                { "data": 1, "weight": 1 }
                            ]
                        },
                        "provider": {
                            "type": "minecraft:simple_state_provider",
                            "state": { "Name": "minecraft:cactus_flower" }
                        }
                    }
                ],
                "prioritize_tip": false
            }
        }"#,
    );
    let closure = FeatureClosure::new(dir.path());
    let spec = closure
        .load_placed_feature(&id("minecraft:patch_cactus_fixture"))
        .unwrap();
    let ConfiguredFeatureKind::BlockColumn {
        layers,
        direction,
        allowed_placement,
        prioritize_tip,
    } = &spec.feature.kind
    else {
        panic!("expected block_column");
    };
    assert_eq!(*direction, ColumnDirection::Up);
    assert_eq!(direction.offset(), (0, 1, 0));
    assert!(!prioritize_tip);
    assert_eq!(
        allowed_placement,
        &BlockPredicateSpec::MatchingBlockTag {
            offset: (0, 0, 0),
            tag: id("minecraft:air"),
        }
    );
    assert_eq!(
        layers[0].height,
        IntProviderSpec::BiasedToBottom {
            min_inclusive: 1,
            max_inclusive: 3,
        }
    );
    assert_eq!(
        layers[1].height,
        IntProviderSpec::WeightedList(vec![
            WeightedInt {
                provider: IntProviderSpec::Constant(0),
                weight: 3,
            },
            WeightedInt {
                provider: IntProviderSpec::Constant(1),
                weight: 1,
            },
        ])
    );
    assert_eq!(
        layers[1].provider,
        StateProviderSpec::Simple {
            state: BlockStateSpec {
                block: id("minecraft:cactus_flower"),
                properties: Vec::new(),
            }
        }
    );
}

#[test]
fn rule_based_provider_resolves_rules_and_fallback() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/tree_like_fixture.json",
        r#"{ "feature": "minecraft:tree_like_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/tree_like_fixture.json",
        r#"{
            "type": "minecraft:block_pile",
            "config": {
                "state_provider": {
                    "type": "minecraft:rule_based_state_provider",
                    "fallback": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:dirt" }
                    },
                    "rules": [
                        {
                            "if_true": {
                                "type": "minecraft:not",
                                "predicate": {
                                    "type": "minecraft:matching_block_tag",
                                    "tag": "minecraft:cannot_replace_below_tree_trunk"
                                }
                            },
                            "then": {
                                "type": "minecraft:simple_state_provider",
                                "state": { "Name": "minecraft:coarse_dirt" }
                            }
                        }
                    ]
                }
            }
        }"#,
    );
    let closure = FeatureClosure::new(dir.path());
    let spec = closure
        .load_placed_feature(&id("minecraft:tree_like_fixture"))
        .unwrap();
    let ConfiguredFeatureKind::BlockPile { state_provider } = &spec.feature.kind else {
        panic!("expected block_pile");
    };
    let StateProviderSpec::RuleBased { fallback, rules } = state_provider else {
        panic!("expected rule_based_state_provider");
    };
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0].predicate,
        BlockPredicateSpec::Not {
            predicate: Box::new(BlockPredicateSpec::MatchingBlockTag {
                offset: (0, 0, 0),
                tag: id("minecraft:cannot_replace_below_tree_trunk"),
            }),
        }
    );
    assert_eq!(
        fallback.as_deref(),
        Some(&StateProviderSpec::Simple {
            state: BlockStateSpec {
                block: id("minecraft:dirt"),
                properties: Vec::new(),
            },
        })
    );
}

#[test]
fn matching_blocks_predicate_accepts_scalar_and_list_forms() {
    let dir = worldgen_dir();
    let closure = FeatureClosure::new(dir.path());
    let referrer = id("minecraft:fixture");
    for (body, expected) in [
        (
            serde_json::json!({ "type": "minecraft:matching_blocks", "blocks": "minecraft:grass_block" }),
            vec![id("minecraft:grass_block")],
        ),
        (
            serde_json::json!({
                "type": "minecraft:matching_blocks",
                "blocks": ["minecraft:sand", "minecraft:red_sand"],
                "offset": [0, -1, 0]
            }),
            vec![id("minecraft:sand"), id("minecraft:red_sand")],
        ),
    ] {
        let predicate = closure.parse_block_predicate(&referrer, &body).unwrap();
        let BlockPredicateSpec::MatchingBlocks { blocks, .. } = predicate else {
            panic!("expected matching_blocks");
        };
        assert_eq!(blocks, expected);
    }
}

#[test]
fn unsupported_configured_feature_type_fails_closed_naming_type_and_referrer() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/tree_fixture.json",
        r#"{ "feature": "minecraft:tree_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/tree_fixture.json",
        r#"{ "type": "minecraft:random_patch", "config": {} }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:tree_fixture"))
        .expect_err("random patches are outside the closure");
    assert!(matches!(
        error,
        ClosureError::UnsupportedType {
            kind: CONFIGURED_FEATURE,
            ..
        }
    ));
    let message = error.to_string();
    assert!(message.contains("minecraft:random_patch"), "{message}");
    assert!(message.contains("minecraft:tree_fixture"), "{message}");
}

#[test]
fn unsupported_placement_modifier_fails_closed_naming_type_and_referrer() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/in_square_fixture.json",
        r#"{
            "feature": "minecraft:plain_fixture",
            "placement": [ { "type": "minecraft:in_square" } ]
        }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/plain_fixture.json",
        r#"{
            "type": "minecraft:block_pile",
            "config": {
                "state_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:melon" }
                }
            }
        }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:in_square_fixture"))
        .expect_err("unreachable modifiers must fail closed");
    assert!(matches!(
        error,
        ClosureError::UnsupportedType {
            kind: PLACEMENT_MODIFIER,
            ..
        }
    ));
    let message = error.to_string();
    assert!(message.contains("minecraft:in_square"), "{message}");
    assert!(message.contains("minecraft:in_square_fixture"), "{message}");
}

#[test]
fn unsupported_state_provider_and_predicate_fail_closed() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/dual_noise_fixture.json",
        r#"{ "feature": "minecraft:dual_noise_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/dual_noise_fixture.json",
        r#"{
            "type": "minecraft:block_pile",
            "config": { "state_provider": { "type": "minecraft:dual_noise_provider" } }
        }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:dual_noise_fixture"))
        .expect_err("unsupported providers must fail closed");
    let message = error.to_string();
    assert!(
        message.contains("minecraft:dual_noise_provider"),
        "{message}"
    );
    assert!(
        message.contains("minecraft:dual_noise_fixture"),
        "{message}"
    );

    write_json(
        dir.path(),
        "placed_feature/solid_filter_fixture.json",
        r#"{
            "feature": "minecraft:plain_fixture",
            "placement": [
                {
                    "type": "minecraft:block_predicate_filter",
                    "predicate": { "type": "minecraft:solid" }
                }
            ]
        }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/plain_fixture.json",
        r#"{
            "type": "minecraft:block_pile",
            "config": {
                "state_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:melon" }
                }
            }
        }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:solid_filter_fixture"))
        .expect_err("unsupported predicates must fail closed");
    let message = error.to_string();
    assert!(message.contains("minecraft:solid"), "{message}");
    assert!(
        message.contains("minecraft:solid_filter_fixture"),
        "{message}"
    );
}

#[test]
fn entries_outside_the_closure_are_never_loaded() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/plain_fixture.json",
        r#"{ "feature": "minecraft:plain_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/plain_fixture.json",
        r#"{
            "type": "minecraft:block_pile",
            "config": {
                "state_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:melon" }
                }
            }
        }"#,
    );
    // Unreachable and unsupported: a load that enumerated the registry
    // would fail here, resolve-by-reference must not read it at all.
    write_json(
        dir.path(),
        "placed_feature/unreachable_tree.json",
        r#"{ "feature": "minecraft:unreachable_tree", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/unreachable_tree.json",
        r#"{ "type": "minecraft:tree", "config": {} }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/unreferenced.json",
        r#"{ "type": "minecraft:not_a_thing", "config": {} }"#,
    );

    let spec = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:plain_fixture"))
        .expect("unreachable entries must not be loaded");
    assert_eq!(spec.id, id("minecraft:plain_fixture"));
}

#[test]
fn pool_walk_reads_only_feature_pool_elements() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/pile_fixture.json",
        r#"{ "feature": "minecraft:pile_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/pile_fixture.json",
        r#"{
            "type": "minecraft:block_pile",
            "config": {
                "state_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:snow", "Properties": { "layers": "1" } }
                }
            }
        }"#,
    );
    write_json(
        dir.path(),
        "placed_feature/flower_fixture.json",
        r#"{ "feature": "minecraft:flower_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/flower_fixture.json",
        r#"{
            "type": "minecraft:simple_block",
            "config": {
                "to_place": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:dandelion" }
                }
            }
        }"#,
    );
    write_json(
        dir.path(),
        "template_pool/village_fixture/decor.json",
        r#"{
            "elements": [
                {
                    "element": {
                        "element_type": "minecraft:legacy_single_pool_element",
                        "location": "minecraft:village/does_not_exist",
                        "processors": { "processors": [] },
                        "projection": "rigid"
                    },
                    "weight": 2
                },
                {
                    "element": {
                        "element_type": "minecraft:feature_pool_element",
                        "feature": "minecraft:pile_fixture",
                        "projection": "rigid"
                    },
                    "weight": 4
                },
                {
                    "element": {
                        "element_type": "minecraft:feature_pool_element",
                        "feature": "minecraft:flower_fixture",
                        "projection": "rigid"
                    },
                    "weight": 1
                },
                { "element": { "element_type": "minecraft:empty_pool_element" }, "weight": 2 }
            ],
            "fallback": "minecraft:empty"
        }"#,
    );

    let entries = FeatureClosure::new(dir.path())
        .load_pool_features(&id("minecraft:village_fixture/decor"))
        .expect("feature elements resolve");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].placed_feature, id("minecraft:pile_fixture"));
    assert_eq!(entries[0].weight, 4);
    assert_eq!(entries[0].pool, id("minecraft:village_fixture/decor"));
    assert_eq!(entries[1].placed_feature, id("minecraft:flower_fixture"));
    assert_eq!(entries[1].weight, 1);
}

#[test]
fn pool_referencing_a_missing_feature_names_the_pool() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "template_pool/village_fixture/decor.json",
        r#"{
            "elements": [
                {
                    "element": {
                        "element_type": "minecraft:feature_pool_element",
                        "feature": "minecraft:absent_feature",
                        "projection": "rigid"
                    },
                    "weight": 1
                }
            ]
        }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_pool_features(&id("minecraft:village_fixture/decor"))
        .expect_err("missing placed features fail closed");
    assert!(matches!(
        error,
        ClosureError::MissingEntry {
            kind: PLACED_FEATURE,
            ..
        }
    ));
    let message = error.to_string();
    assert!(message.contains("minecraft:absent_feature"), "{message}");
    assert!(
        message.contains("minecraft:village_fixture/decor"),
        "{message}"
    );
}

#[test]
fn non_minecraft_references_fail_closed() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/plugin_fixture.json",
        r#"{ "feature": "example:plugin_feature", "placement": [] }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:plugin_fixture"))
        .expect_err("non-vanilla configured features fail closed");
    assert!(matches!(
        error,
        ClosureError::UnsupportedNamespace {
            kind: CONFIGURED_FEATURE,
            ..
        }
    ));
    let message = error.to_string();
    assert!(message.contains("example:plugin_feature"), "{message}");
    assert!(message.contains("minecraft:plugin_fixture"), "{message}");
}

#[test]
fn block_state_property_values_must_be_strings() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/bad_props_fixture.json",
        r#"{ "feature": "minecraft:plain_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/plain_fixture.json",
        r#"{
            "type": "minecraft:block_pile",
            "config": {
                "state_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:snow", "Properties": { "layers": 1 } }
                }
            }
        }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:bad_props_fixture"))
        .expect_err("a non-string property value is a data error");
    let message = error.to_string();
    assert!(message.contains("layers=1"), "{message}");
}

#[test]
fn pool_element_weight_is_required_and_bounded() {
    for (weight, expect_error) in [("\"weight\": 0", true), ("\"weight\": 151", true)] {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "template_pool/village_fixture/decor.json",
            &format!(
                r#"{{
                    "elements": [
                        {{
                            "element": {{
                                "element_type": "minecraft:feature_pool_element",
                                "feature": "minecraft:absent_feature",
                                "projection": "rigid"
                            }},
                            {weight}
                        }}
                    ]
                }}"#
            ),
        );
        let result = FeatureClosure::new(dir.path())
            .load_pool_features(&id("minecraft:village_fixture/decor"));
        assert_eq!(result.is_err(), expect_error, "weight {weight}");
    }

    // A missing weight is an error too, unlike a silent default of 1.
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "template_pool/village_fixture/decor.json",
        r#"{
            "elements": [
                {
                    "element": {
                        "element_type": "minecraft:empty_pool_element"
                    }
                }
            ]
        }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_pool_features(&id("minecraft:village_fixture/decor"))
        .expect_err("weight is required");
    assert!(error.to_string().contains("weight"), "{error}");
}

#[test]
fn list_pool_elements_fail_closed() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "template_pool/village_fixture/decor.json",
        r#"{
            "elements": [
                {
                    "element": {
                        "element_type": "minecraft:list_pool_element",
                        "elements": [
                            {
                                "element_type": "minecraft:feature_pool_element",
                                "feature": "minecraft:absent_feature",
                                "projection": "rigid"
                            }
                        ],
                        "projection": "rigid"
                    },
                    "weight": 1
                }
            ]
        }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_pool_features(&id("minecraft:village_fixture/decor"))
        .expect_err("a list element can wrap feature elements, so it may not be skipped");
    let message = error.to_string();
    assert!(message.contains("minecraft:list_pool_element"), "{message}");
    assert!(
        message.contains("minecraft:village_fixture/decor"),
        "{message}"
    );
}

#[test]
fn predicate_offsets_must_stay_within_vanillas_bound() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/offset_fixture.json",
        r#"{
            "feature": "minecraft:plain_fixture",
            "placement": [
                {
                    "type": "minecraft:block_predicate_filter",
                    "predicate": {
                        "type": "minecraft:matching_block_tag",
                        "tag": "minecraft:air",
                        "offset": [0, 17, 0]
                    }
                }
            ]
        }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/plain_fixture.json",
        r#"{
            "type": "minecraft:block_pile",
            "config": {
                "state_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:melon" }
                }
            }
        }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:offset_fixture"))
        .expect_err("Vec3i.offsetCodec(16) bounds predicate offsets");
    assert!(error.to_string().contains("offset"), "{error}");
}

#[test]
fn block_column_requires_prioritize_tip() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/column_fixture.json",
        r#"{ "feature": "minecraft:column_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/column_fixture.json",
        r#"{
            "type": "minecraft:block_column",
            "config": {
                "allowed_placement": { "type": "minecraft:matching_block_tag", "tag": "minecraft:air" },
                "direction": "up",
                "layers": [
                    {
                        "height": 1,
                        "provider": {
                            "type": "minecraft:simple_state_provider",
                            "state": { "Name": "minecraft:cactus", "Properties": { "age": "0" } }
                        }
                    }
                ]
            }
        }"#,
    );
    let error = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:column_fixture"))
        .expect_err("prioritize_tip has no default in vanilla");
    assert!(error.to_string().contains("prioritize_tip"), "{error}");
}

#[test]
fn tree_configuration_resolves_placers_and_defaults() {
    let dir = worldgen_dir();
    write_json(
        dir.path(),
        "placed_feature/tree_fixture.json",
        r#"{ "feature": "minecraft:tree_fixture", "placement": [] }"#,
    );
    write_json(
        dir.path(),
        "configured_feature/tree_fixture.json",
        r#"{
            "type": "minecraft:tree",
            "config": {
                "decorators": [],
                "foliage_placer": { "type": "minecraft:acacia_foliage_placer", "offset": 0, "radius": 2 },
                "foliage_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:acacia_leaves", "Properties": { "distance": "7", "persistent": "false", "waterlogged": "false" } }
                },
                "minimum_size": { "type": "minecraft:two_layers_feature_size" },
                "trunk_placer": { "type": "minecraft:forking_trunk_placer", "base_height": 5, "height_rand_a": 2, "height_rand_b": 2 },
                "trunk_provider": {
                    "type": "minecraft:simple_state_provider",
                    "state": { "Name": "minecraft:acacia_log", "Properties": { "axis": "y" } }
                }
            }
        }"#,
    );
    let spec = FeatureClosure::new(dir.path())
        .load_placed_feature(&id("minecraft:tree_fixture"))
        .expect("tree closure resolves");
    let ConfiguredFeatureKind::Tree(tree) = &spec.feature.kind else {
        panic!("expected a tree");
    };
    assert_eq!(
        tree.trunk_placer,
        TrunkPlacerSpec::Forking(TrunkPlacerHeights {
            base_height: 5,
            height_rand_a: 2,
            height_rand_b: 2,
        })
    );
    // `ignore_vines` defaults to false and the below-trunk provider to
    // vanilla's `PLACE_BELOW_OVERWORLD_TRUNKS`.
    assert!(!tree.ignore_vines);
    let StateProviderSpec::RuleBased { fallback, rules } = &tree.below_trunk_provider else {
        panic!("expected the default below-trunk rule");
    };
    assert!(fallback.is_none());
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules[0].predicate,
        BlockPredicateSpec::Not {
            predicate: Box::new(BlockPredicateSpec::MatchingBlockTag {
                offset: (0, 0, 0),
                tag: id("minecraft:cannot_replace_below_tree_trunk"),
            }),
        }
    );
    // The `two_layers_feature_size` codec's `.orElse` defaults.
    assert_eq!(tree.minimum_size.limit, 1);
    assert_eq!(tree.minimum_size.lower_size, 0);
    assert_eq!(tree.minimum_size.upper_size, 1);
    assert_eq!(tree.minimum_size.min_clipped_height, None);
}
