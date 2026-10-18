use super::super::*;
use mc_nbt::{ListTag, Tag, tag_type};

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}

fn tables(name: &str) -> LootTables {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/data/minecraft/loot_table/blocks");
    let path = root.join(format!("{name}.json"));
    let table = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    LootTables {
        block_drops: BTreeMap::new(),
        block_loot: BTreeMap::from([(
            id(&format!("minecraft:{name}")),
            block_loot_from_table(&path, &table).unwrap().unwrap(),
        )]),
    }
}

fn roll(
    tables: &LootTables,
    block: &str,
    entity: Option<&Tag>,
) -> Result<Vec<LootDrop>, BlockLootEvaluationError> {
    let block = id(block);
    let tool = LootContextItem::empty();
    let mut context = BlockLootContext::try_new(
        &block,
        &[],
        &tool,
        LootRandomBinding::new(tables.block_random_sequence(&block).cloned(), 1),
    )
    .unwrap();
    if let Some(entity) = entity {
        context = context.with_block_entity(entity);
    }
    tables.evaluate_block(&context).map(Option::unwrap)
}

#[test]
fn banners_drop_without_a_block_entity_context() {
    let tables = tables("white_banner");
    let drops = roll(&tables, "minecraft:white_banner", None).unwrap();
    assert_eq!(drops, vec![LootDrop::single(id("minecraft:white_banner"))]);
}

#[test]
fn banner_implicit_and_saved_components_copy_losslessly_and_filter_unselected_data() {
    let patterns = Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements: vec![Tag::Compound(vec![
            (
                "pattern".into(),
                Tag::String("minecraft:stripe_bottom".into()),
            ),
            ("color".into(), Tag::String("red".into())),
        ])],
    });
    let name = Tag::Compound(vec![
        ("text".into(), Tag::String("Flag".into())),
        ("italic".into(), Tag::Byte(0)),
    ]);
    let item_name = Tag::String("Named banner".into());
    let tooltip = Tag::Compound(vec![("hide_tooltip".into(), Tag::Byte(1))]);
    let entity = Tag::Compound(vec![
        ("patterns".into(), patterns.clone()),
        ("CustomName".into(), name.clone()),
        (
            "components".into(),
            Tag::Compound(vec![
                ("minecraft:item_name".into(), item_name.clone()),
                ("minecraft:tooltip_display".into(), tooltip.clone()),
                ("minecraft:rarity".into(), Tag::String("rare".into())),
                ("minecraft:damage".into(), Tag::Int(7)),
            ]),
        ),
    ]);
    let drops = roll(
        &tables("white_banner"),
        "minecraft:white_banner",
        Some(&entity),
    )
    .unwrap();
    let components = drops[0].components.as_deref().unwrap();
    assert_eq!(
        components,
        &Tag::Compound(vec![
            ("minecraft:custom_name".into(), name),
            ("minecraft:item_name".into(), item_name),
            ("minecraft:tooltip_display".into(), tooltip),
            ("minecraft:banner_patterns".into(), patterns),
            ("minecraft:rarity".into(), Tag::String("rare".into())),
        ])
    );
}

#[test]
fn copied_container_components_reframe_slots_and_retain_lock_and_unopened_loot() {
    let entity = Tag::Compound(vec![
        (
            "Items".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![
                    ("Slot".into(), Tag::Byte(3)),
                    ("id".into(), Tag::String("minecraft:diamond".into())),
                    ("count".into(), Tag::Int(2)),
                ])],
            }),
        ),
        (
            "lock".into(),
            Tag::Compound(vec![(
                "items".into(),
                Tag::String("minecraft:tripwire_hook".into()),
            )]),
        ),
        (
            "LootTable".into(),
            Tag::String("minecraft:chests/end_city_treasure".into()),
        ),
        ("LootTableSeed".into(), Tag::Long(19)),
    ]);
    let drops = roll(
        &tables("blue_shulker_box"),
        "minecraft:blue_shulker_box",
        Some(&entity),
    )
    .unwrap();
    let Tag::Compound(components) = drops[0].components.as_deref().unwrap() else {
        panic!();
    };
    let Tag::List(container) = crate::item_nbt::field(components, "minecraft:container").unwrap()
    else {
        panic!();
    };
    let Tag::Compound(slot) = &container.elements[0] else {
        panic!();
    };
    assert_eq!(crate::item_nbt::field(slot, "slot"), Some(&Tag::Int(3)));
    assert_eq!(
        crate::item_nbt::field(slot, "item"),
        Some(&Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:diamond".into())),
            ("count".into(), Tag::Int(2))
        ]))
    );
    assert!(crate::item_nbt::field(components, "minecraft:lock").is_some());
    assert_eq!(
        crate::item_nbt::field(components, "minecraft:container_loot"),
        Some(&Tag::Compound(vec![
            (
                "loot_table".into(),
                Tag::String("minecraft:chests/end_city_treasure".into())
            ),
            ("seed".into(), Tag::Long(19))
        ]))
    );
}

#[test]
fn malformed_component_sources_fail_with_exact_diagnostics() {
    let tables = tables("white_banner");
    for (components, diagnostic) in [
        (
            Tag::Int(1),
            "block loot block entity: components must be a compound",
        ),
        (
            Tag::Compound(vec![
                ("minecraft:rarity".into(), Tag::String("rare".into())),
                ("minecraft:rarity".into(), Tag::String("common".into())),
            ]),
            "block loot block entity: components contains duplicate names",
        ),
    ] {
        let error = roll(
            &tables,
            "minecraft:white_banner",
            Some(&Tag::Compound(vec![("components".into(), components)])),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), diagnostic);
    }
}

#[test]
fn silk_hive_copies_full_occupants_and_named_honey_state_only_on_silk_branch() {
    let tables = tables("beehive");
    let block = id("minecraft:beehive");
    let properties = vec![("honey_level".into(), "3".into())];
    let bees = Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements: vec![Tag::Compound(vec![
            (
                "entity_data".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:bee".into())),
                    ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
                    ("CustomName".into(), Tag::String("Keeper".into())),
                ]),
            ),
            ("ticks_in_hive".into(), Tag::Int(17)),
            ("min_ticks_in_hive".into(), Tag::Int(600)),
        ])],
    });
    let entity = Tag::Compound(vec![("bees".into(), bees.clone())]);
    for silk in [false, true] {
        let tool = if silk {
            LootContextItem::new(id("minecraft:diamond_axe"))
                .try_with_enchantment(id("minecraft:silk_touch"), 1)
                .unwrap()
        } else {
            LootContextItem::empty()
        };
        let context = BlockLootContext::try_new(
            &block,
            &properties,
            &tool,
            LootRandomBinding::new(tables.block_random_sequence(&block).cloned(), 1),
        )
        .unwrap()
        .with_block_entity(&entity);
        let drops = tables.evaluate_block(&context).unwrap().unwrap();
        assert_eq!(drops.len(), 1);
        if silk {
            assert_eq!(
                drops[0].components.as_deref(),
                Some(&Tag::Compound(vec![
                    ("minecraft:bees".into(), bees.clone()),
                    (
                        "minecraft:block_state".into(),
                        Tag::Compound(vec![("honey_level".into(), Tag::String("3".into())),])
                    ),
                ]))
            );
        } else {
            assert!(drops[0].components.is_none());
        }
    }
}

#[test]
fn intact_pots_copy_decorations_while_cracked_pots_expand_the_same_four_sherds() {
    let tables = tables("decorated_pot");
    let block = id("minecraft:decorated_pot");
    let sherds = [
        "minecraft:angler_pottery_sherd",
        "minecraft:brick",
        "minecraft:archer_pottery_sherd",
        "minecraft:brick",
    ];
    let decorations = Tag::List(ListTag {
        element_type: tag_type::STRING,
        elements: sherds
            .iter()
            .map(|name| Tag::String((*name).into()))
            .collect(),
    });
    let entity = Tag::Compound(vec![("sherds".into(), decorations.clone())]);
    let tool = LootContextItem::empty();
    for cracked in [false, true] {
        let properties = vec![("cracked".into(), cracked.to_string())];
        let context = BlockLootContext::try_new(
            &block,
            &properties,
            &tool,
            LootRandomBinding::new(tables.block_random_sequence(&block).cloned(), 1),
        )
        .unwrap()
        .with_block_entity(&entity);
        let drops = tables.evaluate_block(&context).unwrap().unwrap();
        if cracked {
            assert_eq!(
                drops,
                sherds
                    .iter()
                    .map(|name| LootDrop::single(id(name)))
                    .collect::<Vec<_>>()
            );
        } else {
            assert_eq!(drops.len(), 1);
            assert_eq!(drops[0].item, block);
            assert_eq!(
                drops[0].components.as_deref(),
                Some(&Tag::Compound(vec![(
                    "minecraft:pot_decorations".into(),
                    decorations.clone()
                ),]))
            );
        }
    }
}

#[test]
fn every_vanilla_block_copy_component_has_a_compiled_drop_owner() {
    let owners = [
        "white_banner",
        "beehive",
        "bee_nest",
        "player_head",
        "skeleton_skull",
        "blue_shulker_box",
        "decorated_pot",
    ];
    let mut included = std::collections::BTreeSet::new();
    for owner in owners {
        let tables = tables(owner);
        let block = id(&format!("minecraft:{owner}"));
        assert!(tables.block_needs_entity(&block), "{owner}");
        for component in [
            "custom_name",
            "container",
            "lock",
            "container_loot",
            "bees",
            "item_name",
            "tooltip_display",
            "banner_patterns",
            "rarity",
            "profile",
            "note_block_sound",
            "pot_decorations",
        ] {
            if tables.block_copies_component(&block, &format!("minecraft:{component}")) {
                included.insert(component);
            }
        }
    }
    assert_eq!(
        included,
        std::collections::BTreeSet::from([
            "custom_name",
            "container",
            "lock",
            "container_loot",
            "bees",
            "item_name",
            "tooltip_display",
            "banner_patterns",
            "rarity",
            "profile",
            "note_block_sound",
            "pot_decorations",
        ])
    );
}
