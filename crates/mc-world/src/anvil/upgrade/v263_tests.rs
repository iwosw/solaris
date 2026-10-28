use super::*;
use mc_nbt::{ListTag, Tag, tag_type};

fn compound(fields: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}
#[test]
fn native_26_3_chunk_normalizes_only_typed_block_states() {
    let state = compound(vec![
        ("id", Tag::String("minecraft:stone".into())),
        ("properties", compound(vec![])),
    ]);
    let opaque = compound(vec![("id", Tag::String("plugin:state".into()))]);
    let mut chunk = compound(vec![
        ("DataVersion", Tag::Int(5023)),
        (
            "sections",
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![compound(vec![(
                    "block_states",
                    compound(vec![(
                        "palette",
                        Tag::List(ListTag {
                            element_type: tag_type::COMPOUND,
                            elements: vec![state],
                        }),
                    )]),
                )])],
            }),
        ),
        ("opaque", opaque.clone()),
    ]);
    upgrade_chunk(&mut chunk, 5023).unwrap();
    let sections = nbt::get(&chunk, "sections").and_then(nbt::list).unwrap();
    let states = nbt::get(&sections[0], "block_states").unwrap();
    let palette = nbt::get(states, "palette").and_then(nbt::list).unwrap();
    assert_eq!(
        nbt::get(&palette[0], "Name"),
        Some(&Tag::String("minecraft:stone".into()))
    );
    assert!(nbt::get(&palette[0], "Properties").is_some());
    assert!(nbt::get(&palette[0], "id").is_none());
    assert_eq!(nbt::get(&chunk, "opaque"), Some(&opaque));
    assert_eq!(
        nbt::get(&chunk, "DataVersion"),
        Some(&Tag::Int(TARGET_DATA_VERSION))
    );
}
#[test]
fn native_26_3_entity_and_player_keep_shapes_and_nested_item_patches() {
    for kind in [RecordKind::Entity, RecordKind::Player] {
        let mut value = compound(vec![
            ("DataVersion", Tag::Int(5023)),
            ("id", Tag::String("minecraft:falling_block".into())),
            (
                "BlockState",
                compound(vec![("id", Tag::String("minecraft:sand".into()))]),
            ),
            ("opaque", Tag::Int(27)),
        ]);
        upgrade_record(&mut value, kind, 5023).unwrap();
        assert_eq!(
            nbt::get(nbt::get(&value, "BlockState").unwrap(), "Name"),
            Some(&Tag::String("minecraft:sand".into()))
        );
        assert_eq!(nbt::get(&value, "opaque"), Some(&Tag::Int(27)));
    }
}

#[test]
fn export_26_3_uses_real_block_state_shape_and_matching_level_version() {
    let mut entity = compound(vec![
        ("id", Tag::String("minecraft:falling_block".into())),
        (
            "BlockState",
            compound(vec![
                ("Name", Tag::String("minecraft:sand".into())),
                ("Properties", compound(vec![])),
            ]),
        ),
        (
            "opaque",
            compound(vec![("Name", Tag::String("plugin:opaque".into()))]),
        ),
    ]);
    prepare_export(&mut entity, RecordKind::Entity).unwrap();
    let state = nbt::get(&entity, "BlockState").unwrap();
    assert_eq!(
        nbt::get(state, "id"),
        Some(&Tag::String("minecraft:sand".into()))
    );
    assert!(nbt::get(state, "properties").is_some());
    assert!(nbt::get(state, "Name").is_none());
    assert!(nbt::get(nbt::get(&entity, "opaque").unwrap(), "Name").is_some());
    let mut level = compound(vec![
        ("DataVersion", Tag::Int(4790)),
        (
            "Data",
            compound(vec![(
                "Version",
                compound(vec![
                    ("Name", Tag::String("26.1.2".into())),
                    ("Id", Tag::Int(4790)),
                ]),
            )]),
        ),
    ]);
    prepare_export(&mut level, RecordKind::Level).unwrap();
    assert_eq!(nbt::get(&level, "DataVersion"), Some(&Tag::Int(5023)));
    let data = nbt::get(&level, "Data").unwrap();
    assert_eq!(nbt::get(data, "DataVersion"), Some(&Tag::Int(5023)));
    let version = nbt::get(data, "Version").unwrap();
    assert_eq!(nbt::get(version, "Name"), Some(&Tag::String("26.3".into())));
    assert_eq!(nbt::get(version, "Id"), Some(&Tag::Int(5023)));
}

#[test]
fn native_26_3_pot_templates_keep_count_nested_components_and_absent_faces() {
    let custom = compound(vec![
        ("id", Tag::String("opaque:not_a_state".into())),
        ("Name", Tag::String("opaque:also_not_a_state".into())),
    ]);
    let template = compound(vec![
        ("id", Tag::String("minecraft:brick".into())),
        ("count", Tag::Int(2)),
        (
            "components",
            compound(vec![
                ("minecraft:waxed", compound(vec![])),
                ("minecraft:custom_data", custom.clone()),
            ]),
        ),
    ]);
    let components = compound(vec![
        (
            "minecraft:pot_decorations",
            compound(vec![("back", template)]),
        ),
        ("minecraft:custom_data", custom),
    ]);
    let mut item = compound(vec![
        ("id", Tag::String("minecraft:decorated_pot".into())),
        ("components", components.clone()),
    ]);
    upgrade_item(&mut item, 5023).unwrap();
    assert_eq!(nbt::get(&item, "components"), Some(&components));
    prepare_export(&mut item, RecordKind::Item).unwrap();
    assert_eq!(nbt::get(&item, "components"), Some(&components));
}

#[test]
fn export_version_selection_is_schema_scoped_and_does_not_mutate() {
    let mut item = compound(vec![
        ("id", Tag::String("minecraft:stone".into())),
        (
            "components",
            compound(vec![(
                "minecraft:custom_data",
                compound(vec![("minecraft:waxed", compound(vec![]))]),
            )]),
        ),
    ]);
    let original = item.clone();
    assert!(!requires_26_3(&mut item, RecordKind::Item).unwrap());
    assert_eq!(item, original);
    nbt::set(
        nbt::get_mut(&mut item, "components").unwrap(),
        "minecraft:waxed",
        compound(vec![]),
    );
    let original = item.clone();
    assert!(requires_26_3(&mut item, RecordKind::Item).unwrap());
    assert_eq!(item, original);
}

#[test]
fn bundle_template_shorthand_and_mixed_wrappers_import_and_export() {
    for (contents, promoted) in [
        (
            Tag::List(ListTag {
                element_type: tag_type::STRING,
                elements: vec![Tag::String("minecraft:stone".into())],
            }),
            false,
        ),
        (
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![
                    compound(vec![("", Tag::String("minecraft:stone".into()))]),
                    compound(vec![
                        ("id", Tag::String("minecraft:brick".into())),
                        ("count", Tag::Int(2)),
                        (
                            "components",
                            compound(vec![("minecraft:waxed", compound(vec![]))]),
                        ),
                    ]),
                ],
            }),
            true,
        ),
    ] {
        let bundle = compound(vec![
            ("id", Tag::String("minecraft:bundle".into())),
            ("count", Tag::Int(1)),
            (
                "components",
                compound(vec![("minecraft:bundle_contents", contents)]),
            ),
        ]);
        for from in [4790, 5023] {
            for kind in [RecordKind::Player, RecordKind::Chunk] {
                let inventory = Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![bundle.clone()],
                });
                let mut record = if kind == RecordKind::Player {
                    compound(vec![
                        ("DataVersion", Tag::Int(from)),
                        ("Inventory", inventory),
                    ])
                } else {
                    compound(vec![
                        ("DataVersion", Tag::Int(from)),
                        (
                            "block_entities",
                            Tag::List(ListTag {
                                element_type: tag_type::COMPOUND,
                                elements: vec![compound(vec![
                                    ("id", Tag::String("minecraft:chest".into())),
                                    ("Items", inventory),
                                ])],
                            }),
                        ),
                    ])
                };
                let mut expected = record.clone();
                nbt::set(&mut expected, "DataVersion", Tag::Int(TARGET_DATA_VERSION));
                upgrade_record(&mut record, kind, from).unwrap();
                assert_eq!(record, expected);
                assert_eq!(requires_26_3(&mut record, kind).unwrap(), promoted);
                assert_eq!(record, expected, "inspection is non-mutating");
                prepare_export(&mut record, kind).unwrap();
                nbt::set(&mut expected, "DataVersion", Tag::Int(5023));
                assert_eq!(record, expected);
            }
        }
    }
}

#[test]
fn container_template_identifier_is_preserved_at_anvil_boundaries() {
    for from in [4790, 5023] {
        let mut item = compound(vec![
            ("DataVersion", Tag::Int(from)),
            ("id", Tag::String("minecraft:shulker_box".into())),
            (
                "components",
                compound(vec![(
                    "minecraft:container",
                    Tag::List(ListTag {
                        element_type: tag_type::COMPOUND,
                        elements: vec![compound(vec![
                            ("slot", Tag::Int(0)),
                            ("item", Tag::String("minecraft:stone".into())),
                        ])],
                    }),
                )]),
            ),
        ]);
        let mut expected = item.clone();
        nbt::set(&mut expected, "DataVersion", Tag::Int(TARGET_DATA_VERSION));
        upgrade_item(&mut item, from).unwrap();
        assert_eq!(item, expected);
        assert!(!requires_26_3(&mut item, RecordKind::Item).unwrap());
        assert_eq!(item, expected);
        prepare_export(&mut item, RecordKind::Item).unwrap();
        nbt::set(&mut expected, "DataVersion", Tag::Int(5023));
        assert_eq!(item, expected);
    }
}

#[test]
fn native_26_3_default_state_strings_and_mixed_palette_wrappers_normalize() {
    for (element_type, elements, expected) in [
        (
            tag_type::STRING,
            vec![Tag::String("minecraft:air".into())],
            vec![compound(vec![(
                "Name",
                Tag::String("minecraft:air".into()),
            )])],
        ),
        (
            tag_type::COMPOUND,
            vec![
                compound(vec![("", Tag::String("minecraft:air".into()))]),
                compound(vec![
                    ("id", Tag::String("minecraft:oak_log".into())),
                    (
                        "properties",
                        compound(vec![("axis", Tag::String("x".into()))]),
                    ),
                ]),
            ],
            vec![
                compound(vec![("Name", Tag::String("minecraft:air".into()))]),
                compound(vec![
                    ("Name", Tag::String("minecraft:oak_log".into())),
                    (
                        "Properties",
                        compound(vec![("axis", Tag::String("x".into()))]),
                    ),
                ]),
            ],
        ),
    ] {
        let mut chunk = compound(vec![
            ("DataVersion", Tag::Int(5023)),
            (
                "sections",
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![compound(vec![
                        ("Y", Tag::Byte(0)),
                        (
                            "block_states",
                            compound(vec![
                                (
                                    "palette",
                                    Tag::List(ListTag {
                                        element_type,
                                        elements,
                                    }),
                                ),
                                ("data", Tag::LongArray(vec![0; 256])),
                            ]),
                        ),
                    ])],
                }),
            ),
        ]);
        upgrade_chunk(&mut chunk, 5023).unwrap();
        let sections = nbt::get(&chunk, "sections").and_then(nbt::list).unwrap();
        let states = nbt::get(&sections[0], "block_states").unwrap();
        let Tag::List(palette) = nbt::get(states, "palette").unwrap() else {
            panic!("palette")
        };
        assert_eq!(palette.element_type, tag_type::COMPOUND);
        assert_eq!(palette.elements, expected);
    }
    let mut entity = compound(vec![
        ("DataVersion", Tag::Int(4790)),
        ("id", Tag::String("minecraft:falling_block".into())),
        ("BlockState", Tag::String("minecraft:sand".into())),
    ]);
    let original = entity.clone();
    assert!(requires_26_3(&mut entity, RecordKind::Entity).unwrap());
    assert_eq!(
        entity, original,
        "shorthand inspection must not rewrite the value"
    );
    nbt::set(&mut entity, "DataVersion", Tag::Int(5023));
    upgrade_record(&mut entity, RecordKind::Entity, 5023).unwrap();
    assert_eq!(
        nbt::get(nbt::get(&entity, "BlockState").unwrap(), "Name"),
        Some(&Tag::String("minecraft:sand".into()))
    );
}

#[test]
fn promoted_chunk_uses_26_3_blending_height_float_sentinel() {
    let mut chunk = compound(vec![
        ("DataVersion", Tag::Int(4790)),
        ("Status", Tag::String("minecraft:full".into())),
        (
            "blending_data",
            compound(vec![(
                "heights",
                Tag::List(ListTag {
                    element_type: tag_type::DOUBLE,
                    elements: vec![Tag::Double(2.25), Tag::Double(f64::MAX)],
                }),
            )]),
        ),
    ]);
    prepare_export(&mut chunk, RecordKind::Chunk).unwrap();
    let heights = nbt::get(nbt::get(&chunk, "blending_data").unwrap(), "heights").unwrap();
    assert_eq!(
        heights,
        &Tag::List(ListTag {
            element_type: tag_type::FLOAT,
            elements: vec![Tag::Float(2.25), Tag::Float(f32::MAX)],
        })
    );
}
