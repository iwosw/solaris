use std::collections::BTreeSet;
use std::path::Path;

use super::*;

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).unwrap()
}

fn loader(root: &Path) -> VillageDataLoader {
    VillageDataLoader::new(root.join("data/minecraft/worldgen"))
}

fn write(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    write(
        dir.path(),
        "data/minecraft/worldgen/structure_set/villages.json",
        r#"{
            "placement": { "type": "minecraft:random_spread", "spacing": 34, "separation": 8, "salt": 10387312 },
            "structures": [{ "structure": "minecraft:village_plains", "weight": 1 }]
        }"#,
    );
    write(
        dir.path(),
        "data/minecraft/worldgen/structure/village_plains.json",
        r##"{
            "type": "minecraft:jigsaw",
            "biomes": "#minecraft:has_structure/village_plains",
            "max_distance_from_center": 80,
            "project_start_to_heightmap": "WORLD_SURFACE_WG",
            "size": 6,
            "spawn_overrides": {},
            "start_height": { "absolute": 0 },
            "start_pool": "minecraft:village/plains/town_centers",
            "step": "surface_structures",
            "terrain_adaptation": "beard_thin",
            "use_expansion_hack": true
        }"##,
    );
    write(
        dir.path(),
        "data/minecraft/worldgen/template_pool/village/plains/town_centers.json",
        r#"{
            "fallback": "minecraft:empty",
            "elements": [
                {
                    "element": {
                        "element_type": "minecraft:legacy_single_pool_element",
                        "location": "minecraft:village/plains/houses/plains_small_house_1",
                        "processors": "minecraft:zombie_plains",
                        "projection": "rigid"
                    },
                    "weight": 2
                }
            ]
        }"#,
    );
    write(
        dir.path(),
        "data/minecraft/worldgen/template_pool/empty.json",
        r#"{ "fallback": "minecraft:empty", "elements": [] }"#,
    );
    write(
        dir.path(),
        "data/minecraft/worldgen/processor_list/zombie_plains.json",
        r#"{
            "processors": [
                {
                    "processor_type": "minecraft:rule",
                    "rules": [
                        {
                            "input_predicate": { "predicate_type": "minecraft:always_true" },
                            "location_predicate": { "predicate_type": "minecraft:always_true" },
                            "output_state": { "Name": "minecraft:cobweb" }
                        }
                    ]
                }
            ]
        }"#,
    );
    dir
}

/// Every field of a structure set survives the load: placement, its
/// optional parameters, and each weighted structure.
#[test]
fn structure_set_round_trips() {
    let dir = tempfile::TempDir::new().unwrap();
    write(
        dir.path(),
        "data/minecraft/worldgen/structure_set/villages.json",
        r#"{
            "placement": {
                "type": "minecraft:random_spread",
                "spacing": 34,
                "separation": 8,
                "salt": 10387312,
                "spread_type": "triangular",
                "locate_offset": [4, 0, -4],
                "frequency_reduction_method": "legacy_type_1",
                "frequency": 0.5,
                "exclusion_zone": { "other_set": "minecraft:monuments", "chunk_count": 4 }
            },
            "structures": [
                { "structure": "minecraft:village_plains", "weight": 1 },
                { "structure": "minecraft:village_desert", "weight": 3 }
            ]
        }"#,
    );
    let spec = loader(dir.path())
        .load_structure_set(&identifier("minecraft:villages"))
        .unwrap();
    assert_eq!(
        spec,
        StructureSetSpec {
            id: identifier("minecraft:villages"),
            placement: PlacementSpec::RandomSpread(RandomSpreadPlacement {
                spacing: 34,
                separation: 8,
                salt: 10387312,
                spread_type: RandomSpreadType::Triangular,
                locate_offset: [4, 0, -4],
                frequency_reduction_method: FrequencyReductionMethod::LegacyType1,
                frequency: 0.5,
                exclusion_zone: Some(ExclusionZone {
                    other_set: identifier("minecraft:monuments"),
                    chunk_count: 4,
                }),
            }),
            structures: vec![
                StructureSetEntry {
                    structure: identifier("minecraft:village_plains"),
                    weight: 1,
                },
                StructureSetEntry {
                    structure: identifier("minecraft:village_desert"),
                    weight: 3,
                },
            ],
        }
    );
}

/// Placement fields vanilla defaults stay defaulted, and fields vanilla
/// requires stay required.
#[test]
fn structure_set_optional_placement_fields_default() {
    let dir = tempfile::TempDir::new().unwrap();
    write(
        dir.path(),
        "data/minecraft/worldgen/structure_set/villages.json",
        r#"{
            "placement": { "type": "minecraft:random_spread", "spacing": 34, "separation": 8, "salt": 10387312 },
            "structures": [{ "structure": "minecraft:village_plains", "weight": 1 }]
        }"#,
    );
    let set = loader(dir.path())
        .load_structure_set(&identifier("minecraft:villages"))
        .unwrap();
    let PlacementSpec::RandomSpread(placement) = set.placement;
    assert_eq!(placement.spread_type, RandomSpreadType::Linear);
    assert_eq!(
        placement.frequency_reduction_method,
        FrequencyReductionMethod::Default
    );
    assert_eq!(placement.frequency, 1.0);
    assert_eq!(placement.locate_offset, [0, 0, 0]);
    assert_eq!(placement.exclusion_zone, None);

    // `placement` is required by `StructureSet.DIRECT_CODEC`.
    write(
        dir.path(),
        "data/minecraft/worldgen/structure_set/villages.json",
        r#"{ "structures": [] }"#,
    );
    let error = loader(dir.path())
        .load_structure_set(&identifier("minecraft:villages"))
        .expect_err("placement is required");
    assert!(
        error.to_string().contains("is missing field placement"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("structure set minecraft:villages"),
        "{error}"
    );
}

/// The jigsaw settings a village declares, including spawn overrides.
#[test]
fn structure_round_trips() {
    let dir = tempfile::TempDir::new().unwrap();
    write(
        dir.path(),
        "data/minecraft/worldgen/structure/village_plains.json",
        r##"{
            "type": "minecraft:jigsaw",
            "biomes": ["minecraft:plains", "minecraft:meadow"],
            "max_distance_from_center": { "horizontal": 80, "vertical": 40 },
            "project_start_to_heightmap": "WORLD_SURFACE_WG",
            "size": 6,
            "spawn_overrides": {
                "monster": {
                    "bounding_box": "piece",
                    "spawns": [
                        {
                            "data": { "type": "minecraft:zombie", "minCount": 1, "maxCount": 2 },
                            "weight": 1
                        }
                    ]
                }
            },
            "start_height": { "type": "minecraft:uniform", "min_inclusive": { "absolute": 0 }, "max_inclusive": { "below_top": 10 } },
            "start_pool": "minecraft:village/plains/town_centers",
            "start_jigsaw_name": "minecraft:bottom",
            "step": "surface_structures",
            "terrain_adaptation": "beard_thin",
            "use_expansion_hack": true,
            "dimension_padding": 2,
            "liquid_settings": "ignore_waterlogging"
        }"##,
    );
    let spec = loader(dir.path())
        .load_structure(&identifier("minecraft:village_plains"))
        .unwrap();
    assert_eq!(
        spec,
        StructureSpec {
            pool_aliases: Vec::new(),
            id: identifier("minecraft:village_plains"),
            structure_type: identifier("minecraft:jigsaw"),
            biomes: BiomeSet::Biomes(vec![
                identifier("minecraft:plains"),
                identifier("minecraft:meadow"),
            ]),
            spawn_overrides: vec![SpawnOverrideEntry {
                category: MobCategory::Monster,
                bounding_box: SpawnBoundingBox::Piece,
                spawns: vec![WeightedSpawn {
                    weight: 1,
                    entity_type: identifier("minecraft:zombie"),
                    min_count: 1,
                    max_count: 2,
                }],
            }],
            step: StructureStep::SurfaceStructures,
            terrain_adaptation: TerrainAdaptation::BeardThin,
            start_pool: identifier("minecraft:village/plains/town_centers"),
            start_jigsaw_name: Some(identifier("minecraft:bottom")),
            size: 6,
            start_height: HeightProviderSpec::Uniform {
                min_inclusive: VerticalAnchor::Absolute(0),
                max_inclusive: VerticalAnchor::BelowTop(10),
            },
            use_expansion_hack: true,
            project_start_to_heightmap: Some(HeightmapType::WorldSurfaceWg),
            max_distance_from_center: MaxDistance {
                horizontal: 80,
                vertical: 40,
            },
            dimension_padding: DimensionPadding { bottom: 2, top: 2 },
            liquid_settings: LiquidSettings::IgnoreWaterlogging,
        }
    );
}

/// The fields the codec defaults, defaulted on the structure too.
#[test]
fn structure_optional_fields_default() {
    let dir = fixture();
    let spec = loader(dir.path())
        .load_structure(&identifier("minecraft:village_plains"))
        .unwrap();
    assert_eq!(
        spec,
        StructureSpec {
            pool_aliases: Vec::new(),
            id: identifier("minecraft:village_plains"),
            structure_type: identifier("minecraft:jigsaw"),
            biomes: BiomeSet::Tag(identifier("minecraft:has_structure/village_plains")),
            spawn_overrides: Vec::new(),
            step: StructureStep::SurfaceStructures,
            terrain_adaptation: TerrainAdaptation::BeardThin,
            start_pool: identifier("minecraft:village/plains/town_centers"),
            start_jigsaw_name: None,
            size: 6,
            start_height: HeightProviderSpec::Constant(VerticalAnchor::Absolute(0)),
            use_expansion_hack: true,
            project_start_to_heightmap: Some(HeightmapType::WorldSurfaceWg),
            max_distance_from_center: MaxDistance {
                horizontal: 80,
                vertical: 80,
            },
            dimension_padding: DimensionPadding { bottom: 0, top: 0 },
            liquid_settings: LiquidSettings::ApplyWaterlogging,
        }
    );
}

/// Every element type the codec has, in one pool: the list form carrying a
/// feature element, an empty element, and both single element types with
/// the reference and inline processor forms.
#[test]
fn template_pool_round_trips_every_element_type() {
    let dir = tempfile::TempDir::new().unwrap();
    write(
        dir.path(),
        "data/minecraft/worldgen/template_pool/village/plains/town_centers.json",
        r##"{
            "fallback": "minecraft:empty",
            "elements": [
                {
                    "element": {
                        "element_type": "minecraft:legacy_single_pool_element",
                        "location": "minecraft:village/plains/houses/plains_small_house_1",
                        "processors": "minecraft:zombie_plains",
                        "projection": "rigid"
                    },
                    "weight": 1
                },
                {
                    "element": {
                        "element_type": "minecraft:single_pool_element",
                        "location": "minecraft:village/plains/houses/plains_small_house_2",
                        "processors": [
                            {
                                "processor_type": "minecraft:protected_blocks",
                                "value": "#minecraft:features_cannot_replace"
                            }
                        ],
                        "projection": "terrain_matching",
                        "override_liquid_settings": "ignore_waterlogging"
                    },
                    "weight": 2
                },
                {
                    "element": {
                        "element_type": "minecraft:list_pool_element",
                        "projection": "rigid",
                        "elements": [
                            {
                                "element_type": "minecraft:feature_pool_element",
                                "feature": "minecraft:pile_hay",
                                "projection": "rigid"
                            },
                            { "element_type": "minecraft:empty_pool_element" }
                        ]
                    },
                    "weight": 3
                }
            ]
        }"##,
    );
    let spec = loader(dir.path())
        .load_template_pool(&identifier("minecraft:village/plains/town_centers"))
        .unwrap();
    assert_eq!(
        spec,
        TemplatePoolSpec {
            id: identifier("minecraft:village/plains/town_centers"),
            fallback: identifier("minecraft:empty"),
            elements: vec![
                PoolElementEntry {
                    weight: 1,
                    element: PoolElementSpec::LegacySingle(SingleElementSpec {
                        location: identifier(
                            "minecraft:village/plains/houses/plains_small_house_1"
                        ),
                        projection: Projection::Rigid,
                        processors: ProcessorRef::List(identifier("minecraft:zombie_plains")),
                        override_liquid_settings: None,
                    }),
                },
                PoolElementEntry {
                    weight: 2,
                    element: PoolElementSpec::Single(SingleElementSpec {
                        location: identifier(
                            "minecraft:village/plains/houses/plains_small_house_2"
                        ),
                        projection: Projection::TerrainMatching,
                        processors: ProcessorRef::Inline(vec![
                            StructureProcessorSpec::ProtectedBlocks {
                                cannot_replace: identifier("minecraft:features_cannot_replace"),
                            }
                        ]),
                        override_liquid_settings: Some(LiquidSettings::IgnoreWaterlogging),
                    }),
                },
                PoolElementEntry {
                    weight: 3,
                    element: PoolElementSpec::List {
                        projection: Projection::Rigid,
                        elements: vec![
                            PoolElementSpec::Feature {
                                feature: identifier("minecraft:pile_hay"),
                                projection: Projection::Rigid,
                            },
                            PoolElementSpec::Empty,
                        ],
                    },
                },
            ],
        }
    );
}

/// Every processor type this layer implements, with their fields: a rule
/// with all three predicates, `block_age`, `gravity`,
/// `jigsaw_replacement` and `protected_blocks`.
#[test]
fn processor_list_round_trips_every_supported_processor() {
    let dir = tempfile::TempDir::new().unwrap();
    write(
        dir.path(),
        "data/minecraft/worldgen/processor_list/zombie_plains.json",
        r##"{
            "processors": [
                {
                    "processor_type": "minecraft:rule",
                    "rules": [
                        {
                            "input_predicate": {
                                "predicate_type": "minecraft:random_block_match",
                                "block": "minecraft:cobblestone",
                                "probability": 0.1
                            },
                            "location_predicate": {
                                "predicate_type": "minecraft:tag_match",
                                "tag": "minecraft:features_cannot_replace"
                            },
                            "position_predicate": {
                                "predicate_type": "minecraft:axis_aligned_linear_pos",
                                "min_chance": 0.0,
                                "max_chance": 0.5,
                                "min_dist": 1,
                                "max_dist": 4,
                                "axis": "y"
                            },
                            "output_state": {
                                "Name": "minecraft:mossy_cobblestone",
                                "Properties": { "snowy": "false" }
                            },
                            "block_entity_modifier": { "type": "minecraft:passthrough" }
                        },
                        {
                            "input_predicate": {
                                "predicate_type": "minecraft:blockstate_match",
                                "block_state": { "Name": "minecraft:dirt" }
                            },
                            "location_predicate": { "predicate_type": "minecraft:always_true" },
                            "output_state": { "Name": "minecraft:grass_block" }
                        }
                    ]
                },
                { "processor_type": "minecraft:block_age", "mossiness": 0.5 },
                { "processor_type": "minecraft:gravity", "heightmap": "WORLD_SURFACE_WG", "offset": -1 },
                { "processor_type": "minecraft:jigsaw_replacement" },
                { "processor_type": "minecraft:protected_blocks", "value": "#minecraft:features_cannot_replace" }
            ]
        }"##,
    );
    let spec = loader(dir.path())
        .load_processor_list(&identifier("minecraft:zombie_plains"))
        .unwrap();
    assert_eq!(
        spec,
        ProcessorListSpec {
            id: identifier("minecraft:zombie_plains"),
            processors: vec![
                StructureProcessorSpec::Rule {
                    rules: vec![
                        ProcessorRuleSpec {
                            append_loot: None,
                            input_predicate: RuleTestSpec::RandomBlockMatch {
                                block: identifier("minecraft:cobblestone"),
                                probability: 0.1,
                            },
                            location_predicate: RuleTestSpec::TagMatch {
                                tag: identifier("minecraft:features_cannot_replace"),
                            },
                            position_predicate: PosRuleTestSpec::AxisAlignedLinearPos {
                                min_chance: 0.0,
                                max_chance: 0.5,
                                min_dist: 1,
                                max_dist: 4,
                                axis: Axis::Y,
                            },
                            output_state: BlockStateSpec {
                                block: identifier("minecraft:mossy_cobblestone"),
                                properties: vec![("snowy".to_owned(), "false".to_owned())],
                            },
                        },
                        ProcessorRuleSpec {
                            append_loot: None,
                            input_predicate: RuleTestSpec::BlockStateMatch {
                                state: BlockStateSpec {
                                    block: identifier("minecraft:dirt"),
                                    properties: Vec::new(),
                                },
                            },
                            location_predicate: RuleTestSpec::AlwaysTrue,
                            position_predicate: PosRuleTestSpec::AlwaysTrue,
                            output_state: BlockStateSpec {
                                block: identifier("minecraft:grass_block"),
                                properties: Vec::new(),
                            },
                        },
                    ],
                },
                StructureProcessorSpec::BlockAge { mossiness: 0.5 },
                StructureProcessorSpec::Gravity {
                    heightmap: HeightmapType::WorldSurfaceWg,
                    offset: -1,
                },
                StructureProcessorSpec::JigsawReplacement,
                StructureProcessorSpec::ProtectedBlocks {
                    cannot_replace: identifier("minecraft:features_cannot_replace"),
                },
            ],
        }
    );
}

/// An empty processor list is legal, and so is the bare-array file form.
#[test]
fn processor_list_accepts_the_empty_and_bare_array_forms() {
    let dir = tempfile::TempDir::new().unwrap();
    write(
        dir.path(),
        "data/minecraft/worldgen/processor_list/empty.json",
        r#"{ "processors": [] }"#,
    );
    write(
        dir.path(),
        "data/minecraft/worldgen/processor_list/bare.json",
        r#"[{ "processor_type": "minecraft:jigsaw_replacement" }]"#,
    );
    let empty = loader(dir.path())
        .load_processor_list(&identifier("minecraft:empty"))
        .unwrap();
    assert_eq!(empty.processors, Vec::new());
    let bare = loader(dir.path())
        .load_processor_list(&identifier("minecraft:bare"))
        .unwrap();
    assert_eq!(
        bare.processors,
        vec![StructureProcessorSpec::JigsawReplacement]
    );
}

/// Every unsupported type names both the type id and the entry that
/// referenced it.
#[test]
fn unsupported_types_fail_closed() {
    let cases: &[(&str, &str, &str)] = &[
        // (fixture body, the file it replaces, the referring entry)
        (
            r#"{
                "placement": { "type": "minecraft:concentric_rings", "spacing": 34, "separation": 8, "salt": 1 },
                "structures": []
            }"#,
            "structure_set/villages.json",
            "minecraft:villages",
        ),
        (
            r##"{
                "type": "minecraft:nether_fossil",
                "biomes": "#minecraft:is_nether",
                "spawn_overrides": {},
                "step": "surface_structures"
            }"##,
            "structure/village_plains.json",
            "minecraft:village_plains",
        ),
        (
            r##"{
                "type": "minecraft:jigsaw",
                "biomes": "#minecraft:has_structure/village_plains",
                "max_distance_from_center": 80,
                "size": 6,
                "spawn_overrides": {},
                "start_height": { "type": "minecraft:weighted_list", "distribution": [] },
                "start_pool": "minecraft:village/plains/town_centers",
                "step": "surface_structures",
                "use_expansion_hack": true
            }"##,
            "structure/village_plains.json",
            "minecraft:village_plains",
        ),
        (
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": { "element_type": "minecraft:absent_pool_element" },
                        "weight": 1
                    }
                ]
            }"#,
            "template_pool/village/plains/town_centers.json",
            "minecraft:village/plains/town_centers",
        ),
        (
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1",
                            "processors": [
                                { "processor_type": "minecraft:absent_processor", "delegate": {} }
                            ],
                            "projection": "rigid"
                        },
                        "weight": 1
                    }
                ]
            }"#,
            "template_pool/village/plains/town_centers.json",
            "minecraft:village/plains/town_centers",
        ),
        (
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1",
                            "processors": [
                                {
                                    "processor_type": "minecraft:rule",
                                    "rules": [
                                        {
                                            "input_predicate": { "predicate_type": "minecraft:absent_test" },
                                            "location_predicate": { "predicate_type": "minecraft:always_true" },
                                            "output_state": { "Name": "minecraft:air" }
                                        }
                                    ]
                                }
                            ],
                            "projection": "rigid"
                        },
                        "weight": 1
                    }
                ]
            }"#,
            "template_pool/village/plains/town_centers.json",
            "minecraft:village/plains/town_centers",
        ),
        (
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1",
                            "processors": [
                                {
                                    "processor_type": "minecraft:rule",
                                    "rules": [
                                        {
                                            "input_predicate": { "predicate_type": "minecraft:always_true" },
                                            "location_predicate": { "predicate_type": "minecraft:always_true" },
                                            "position_predicate": { "predicate_type": "minecraft:absent_pos_test" },
                                            "output_state": { "Name": "minecraft:air" }
                                        }
                                    ]
                                }
                            ],
                            "projection": "rigid"
                        },
                        "weight": 1
                    }
                ]
            }"#,
            "template_pool/village/plains/town_centers.json",
            "minecraft:village/plains/town_centers",
        ),
        (
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1",
                            "processors": [
                                {
                                    "processor_type": "minecraft:rule",
                                    "rules": [
                                        {
                                            "input_predicate": { "predicate_type": "minecraft:always_true" },
                                            "location_predicate": { "predicate_type": "minecraft:always_true" },
                                            "block_entity_modifier": { "type": "minecraft:absent_modifier", "loot_table": "minecraft:empty" },
                                            "output_state": { "Name": "minecraft:air" }
                                        }
                                    ]
                                }
                            ],
                            "projection": "rigid"
                        },
                        "weight": 1
                    }
                ]
            }"#,
            "template_pool/village/plains/town_centers.json",
            "minecraft:village/plains/town_centers",
        ),
        (
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1",
                            "processors": [
                                { "processor_type": "minecraft:protected_blocks", "value": "minecraft:features_cannot_replace" }
                            ],
                            "projection": "rigid"
                        },
                        "weight": 1
                    }
                ]
            }"#,
            "template_pool/village/plains/town_centers.json",
            "minecraft:village/plains/town_centers",
        ),
    ];
    let expected_type = [
        "minecraft:concentric_rings",
        "minecraft:nether_fossil",
        "minecraft:weighted_list",
        "minecraft:absent_pool_element",
        "minecraft:absent_processor",
        "minecraft:absent_test",
        "minecraft:absent_pos_test",
        "minecraft:absent_modifier",
    ];
    for ((body, relative, referrer), type_id) in cases.iter().zip(expected_type) {
        let dir = fixture();
        write(
            dir.path(),
            &format!("data/minecraft/worldgen/{relative}"),
            body,
        );
        let error = load_all(dir.path()).expect_err("unsupported data fails closed");
        let message = error.to_string();
        assert!(
            message.contains(type_id),
            "{type_id} missing from {message}"
        );
        assert!(
            message.contains(referrer),
            "{referrer} missing from {message}"
        );
    }
    // The `protected_blocks` case rejects the unhashed tag name: the
    // `Type` outcome rather than `Value`.
    let dir = fixture();
    write(
        dir.path(),
        "data/minecraft/worldgen/template_pool/village/plains/town_centers.json",
        cases[8].0,
    );
    let error = load_all(dir.path()).expect_err("an unhashed protected block tag fails");
    assert!(
        error
            .to_string()
            .contains("has invalid value for value: minecraft:features_cannot_replace"),
        "{error}"
    );
}

/// Required fields are required, and the error names the entry and field.
#[test]
fn missing_required_fields_fail_closed() {
    let cases: &[(&str, &str, &str)] = &[
        (
            r##"{
                "type": "minecraft:jigsaw",
                "biomes": "#minecraft:has_structure/village_plains",
                "max_distance_from_center": 80,
                "size": 6,
                "start_height": { "absolute": 0 },
                "start_pool": "minecraft:village/plains/town_centers",
                "step": "surface_structures",
                "use_expansion_hack": true
            }"##,
            "structure/village_plains.json",
            "is missing field spawn_overrides",
        ),
        (
            r##"{
                "type": "minecraft:jigsaw",
                "biomes": "#minecraft:has_structure/village_plains",
                "max_distance_from_center": 80,
                "spawn_overrides": {},
                "size": 6,
                "start_height": { "absolute": 0 },
                "start_pool": "minecraft:village/plains/town_centers",
                "step": "surface_structures"
            }"##,
            "structure/village_plains.json",
            "is missing field use_expansion_hack",
        ),
        (
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1"
                        },
                        "weight": 1
                    }
                ]
            }"#,
            "template_pool/village/plains/town_centers.json",
            "is missing field projection",
        ),
        (
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1",
                            "projection": "rigid"
                        },
                        "weight": 1
                    }
                ]
            }"#,
            "template_pool/village/plains/town_centers.json",
            "is missing field processors",
        ),
        (
            r#"{
                "processors": [
                    {
                        "processor_type": "minecraft:rule",
                        "rules": [
                            {
                                "location_predicate": { "predicate_type": "minecraft:always_true" },
                                "output_state": { "Name": "minecraft:air" }
                            }
                        ]
                    }
                ]
            }"#,
            "processor_list/zombie_plains.json",
            "is missing field input_predicate",
        ),
        (
            r#"{
                "processors": [{ "processor_type": "minecraft:block_age" }]
            }"#,
            "processor_list/zombie_plains.json",
            "is missing field mossiness",
        ),
    ];
    let entries = [
        "minecraft:village_plains",
        "minecraft:village_plains",
        "minecraft:village/plains/town_centers",
        "minecraft:village/plains/town_centers",
        "minecraft:zombie_plains",
        "minecraft:zombie_plains",
    ];
    for ((body, relative, expected), entry) in cases.iter().zip(entries) {
        let dir = fixture();
        write(
            dir.path(),
            &format!("data/minecraft/worldgen/{relative}"),
            body,
        );
        let error = load_all(dir.path()).expect_err("missing field fails closed");
        let message = error.to_string();
        assert!(
            message.contains(expected),
            "{expected} missing from {message}"
        );
        assert!(message.contains(entry), "{entry} missing from {message}");
    }
}

/// A structure outside the vanilla namespace never resolves: the cache
/// only holds `data/minecraft`.
#[test]
fn non_vanilla_namespace_fails_closed() {
    let dir = fixture();
    let error = loader(dir.path())
        .load_structure_set(&identifier("solaris:villages"))
        .expect_err("a non-vanilla structure set fails closed");
    assert!(
        error
            .to_string()
            .contains("non-minecraft structure set solaris:villages"),
        "{error}"
    );
}

/// A missing entry names the id, the entry that referenced it and the path
/// that was searched. Inside one load the referrer is the entry that
/// carries the reference; a top-level load is its own referrer.
#[test]
fn missing_entry_names_referrer() {
    let dir = fixture();
    let error = loader(dir.path())
        .load_template_pool(&identifier("minecraft:village/plains/absent"))
        .expect_err("the absent pool fails");
    let message = error.to_string();
    assert!(
        message.contains("missing template pool minecraft:village/plains/absent"),
        "{message}"
    );
    assert!(message.contains("village/plains/absent.json"), "{message}");

    // A pool's element carries an inline processor list, so an unsupported
    // rule test there is reported against the pool that carries it.
    write(
        dir.path(),
        "data/minecraft/worldgen/template_pool/village/plains/town_centers.json",
        r#"{
            "fallback": "minecraft:empty",
            "elements": [
                {
                    "element": {
                        "element_type": "minecraft:legacy_single_pool_element",
                        "location": "minecraft:village/plains/houses/plains_small_house_1",
                        "processors": [
                            {
                                "processor_type": "minecraft:rule",
                                "rules": [
                                    {
                                        "input_predicate": { "predicate_type": "minecraft:absent_test" },
                                        "location_predicate": { "predicate_type": "minecraft:always_true" },
                                        "output_state": { "Name": "minecraft:air" }
                                    }
                                ]
                            }
                        ],
                        "projection": "rigid"
                    },
                    "weight": 1
                }
            ]
        }"#,
    );
    let error = load_all(dir.path()).expect_err("the absent rule test fails");
    let message = error.to_string();
    assert!(message.contains("minecraft:absent_test"), "{message}");
    assert!(
        message.contains("referenced by minecraft:village/plains/town_centers"),
        "{message}"
    );
}

/// Loads the fixture closure the way an engine would: set, then each
/// structure, its start pool and fallback chain, and every processor list
/// those elements name.
fn load_all(root: &Path) -> Result<(), VillageDataError> {
    let loader = loader(root);
    let set = loader.load_structure_set(&identifier("minecraft:villages"))?;
    let mut pools: BTreeSet<Identifier> = BTreeSet::new();
    for entry in &set.structures {
        let structure = loader.load_structure(&entry.structure)?;
        let mut next = Some(structure.start_pool.clone());
        while let Some(pool_id) = next.take() {
            let pool = loader.load_template_pool(&pool_id)?;
            for element in &pool.elements {
                let single = match &element.element {
                    PoolElementSpec::Single(single) | PoolElementSpec::LegacySingle(single) => {
                        single
                    }
                    PoolElementSpec::List { .. }
                    | PoolElementSpec::Feature { .. }
                    | PoolElementSpec::Empty => continue,
                };
                if let ProcessorRef::List(list) = &single.processors {
                    loader.load_processor_list(list)?;
                }
            }
            if pool.fallback != pool_id {
                next = Some(pool.fallback.clone());
            }
            pools.insert(pool_id);
        }
    }
    assert!(!pools.is_empty());
    Ok(())
}

/// The local vanilla content cache, when it is present. An explicit
/// `SOLARIS_CONTENT_CACHE` pins the search to that root — an operator who
/// names a cache does not want another one silently standing in for it —
/// otherwise the workspace cache and the documented scratch cache are tried.
fn real_cache_worldgen() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("SOLARIS_CONTENT_CACHE") {
        return find_village_cache([PathBuf::from(dir)]);
    }
    let mut candidates = Vec::new();
    if let Some(workspace) = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
    {
        candidates.push(workspace.join("data/vanilla"));
    }
    candidates.push(PathBuf::from("/tmp/jdk-cold2"));
    find_village_cache(candidates)
}

/// The first candidate that actually holds the villages structure set.
fn find_village_cache(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .map(|root| root.join("data/minecraft/worldgen"))
        .find(|worldgen| worldgen.join("structure_set/villages.json").is_file())
}

/// What drives the loud skip: a candidate only counts when it holds the
/// villages set, and an absent cache is reported as absent.
#[test]
fn cache_detection_distinguishes_present_from_absent() {
    let dir = tempfile::TempDir::new().unwrap();
    assert_eq!(find_village_cache([dir.path().to_path_buf()]), None);
    write(
        dir.path(),
        "data/minecraft/worldgen/structure_set/villages.json",
        r#"{ "placement": { "type": "minecraft:random_spread", "spacing": 34, "separation": 8, "salt": 1 }, "structures": [] }"#,
    );
    assert_eq!(
        find_village_cache([dir.path().to_path_buf()]),
        Some(dir.path().join("data/minecraft/worldgen"))
    );
}

/// The real-data proof: the villages set, all five village structures with
/// their biome tags and start pools, and the processor lists those pools
/// name. Skips loudly when no cache is present.
#[test]
fn real_cache_villages_closure_loads() {
    let Some(worldgen) = real_cache_worldgen() else {
        eprintln!(
            "SKIP real_cache_villages_closure_loads: no vanilla content cache with \
             data/minecraft/worldgen/structure_set/villages.json (set SOLARIS_CONTENT_CACHE, \
             or place one at /tmp/jdk-cold2)"
        );
        return;
    };
    println!("real cache: {}", worldgen.display());
    let loader = VillageDataLoader::new(&worldgen);

    let set = loader
        .load_structure_set(&identifier("minecraft:villages"))
        .expect("the villages structure set resolves");
    let PlacementSpec::RandomSpread(placement) = set.placement;
    assert_eq!((placement.spacing, placement.separation), (34, 8));
    assert_eq!(placement.salt, 10387312);
    assert_eq!(placement.spread_type, RandomSpreadType::Linear);
    assert_eq!(placement.frequency, 1.0);
    assert_eq!(
        placement.frequency_reduction_method,
        FrequencyReductionMethod::Default
    );
    assert_eq!(placement.locate_offset, [0, 0, 0]);
    assert_eq!(placement.exclusion_zone, None);
    println!(
        "structure set {} placement=random_spread spacing={} separation={} salt={} spread={:?}",
        set.id, placement.spacing, placement.separation, placement.salt, placement.spread_type
    );

    let ids: Vec<Identifier> = set
        .structures
        .iter()
        .map(|entry| entry.structure.clone())
        .collect();
    assert_eq!(ids.len(), 5);
    println!("structure set {} names {} structures", set.id, ids.len());

    let mut pools = BTreeSet::new();
    let mut processor_lists = BTreeSet::new();
    let mut elements = 0;
    for entry in &set.structures {
        let structure = loader
            .load_structure(&entry.structure)
            .expect("every village structure resolves");
        assert_eq!(entry.weight, 1);
        assert_eq!(structure.structure_type, identifier("minecraft:jigsaw"));
        assert_eq!(structure.size, 6);
        assert_eq!(
            structure.start_height,
            HeightProviderSpec::Constant(VerticalAnchor::Absolute(0))
        );
        assert_eq!(
            structure.project_start_to_heightmap,
            Some(HeightmapType::WorldSurfaceWg)
        );
        assert_eq!(structure.terrain_adaptation, TerrainAdaptation::BeardThin);
        assert_eq!(
            structure.max_distance_from_center,
            MaxDistance {
                horizontal: 80,
                vertical: 80
            }
        );
        assert!(structure.use_expansion_hack);
        assert_eq!(structure.step, StructureStep::SurfaceStructures);
        assert!(structure.spawn_overrides.is_empty());
        assert_eq!(structure.start_jigsaw_name, None);
        let BiomeSet::Tag(tag) = &structure.biomes else {
            panic!("village {} uses a biome tag", structure.id);
        };
        assert_eq!(
            tag.to_string(),
            format!("minecraft:has_structure/{}", structure.id.path())
        );
        assert!(
            structure.start_pool.path().ends_with("/town_centers"),
            "{}",
            structure.start_pool
        );
        println!(
            "structure {} biome_tag={} start_pool={} size={} start_height={:?} terrain={:?} max_distance={:?}",
            structure.id,
            tag,
            structure.start_pool,
            structure.size,
            structure.start_height,
            structure.terrain_adaptation,
            structure.max_distance_from_center
        );

        let mut next = Some(structure.start_pool.clone());
        while let Some(pool_id) = next.take() {
            if !pools.insert(pool_id.clone()) {
                continue;
            }
            let pool = loader
                .load_template_pool(&pool_id)
                .expect("every referenced pool resolves");
            println!(
                "pool {} elements={} fallback={}",
                pool.id,
                pool.elements.len(),
                pool.fallback
            );
            elements += pool.elements.len();
            for element in &pool.elements {
                let single = match &element.element {
                    PoolElementSpec::Single(single) | PoolElementSpec::LegacySingle(single) => {
                        single
                    }
                    PoolElementSpec::Feature { .. } | PoolElementSpec::Empty => continue,
                    PoolElementSpec::List { .. } => {
                        panic!("a village start pool holds no list element")
                    }
                };
                match &single.processors {
                    ProcessorRef::List(list) => {
                        processor_lists.insert(list.clone());
                    }
                    ProcessorRef::Inline(processors) => assert!(
                        processors.is_empty(),
                        "village start pool elements carry empty inline lists"
                    ),
                }
            }
            if pool.fallback != pool_id {
                next = Some(pool.fallback.clone());
            }
        }
    }
    println!(
        "village start-pool closure: pools={} elements={} processor_lists={}",
        pools.len(),
        elements,
        processor_lists.len()
    );
    assert_eq!(pools.len(), 6, "five start pools plus minecraft:empty");
    assert_eq!(elements, 32);
    let expected_lists: Vec<String> = processor_lists.iter().map(ToString::to_string).collect();
    assert_eq!(
        expected_lists,
        vec![
            "minecraft:mossify_10_percent",
            "minecraft:mossify_20_percent",
            "minecraft:mossify_70_percent",
            "minecraft:zombie_desert",
            "minecraft:zombie_plains",
            "minecraft:zombie_savanna",
            "minecraft:zombie_taiga",
        ]
    );
    let mut rules = 0;
    for list_id in &processor_lists {
        let list = loader
            .load_processor_list(list_id)
            .expect("every referenced processor list resolves");
        let list_rules: usize = list
            .processors
            .iter()
            .map(|processor| match processor {
                StructureProcessorSpec::Rule { rules } => rules.len(),
                other => panic!("{list_id} holds a non-rule processor {other:?}"),
            })
            .sum();
        rules += list_rules;
        println!(
            "processor_list {} processors={} rules={}",
            list.id,
            list.processors.len(),
            list_rules
        );
    }
    assert_eq!(rules, 56);
    println!("village start-pool closure: rule processors={rules}");
}
