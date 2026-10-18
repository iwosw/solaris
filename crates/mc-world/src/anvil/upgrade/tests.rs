use super::{nbt, *};
use crate::{BlockRegistry, BlockStateId};
use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::entity_nbt::{VanillaEntity, entity_from_fields};
use mc_data::player_nbt::{VanillaPlayer, player_from_fields};
use mc_nbt::{ListTag, Tag, tag_type};
use std::collections::BTreeMap;
use std::io::Cursor;

fn s(value: &str) -> Tag {
    Tag::String(value.into())
}
fn c(fields: Vec<(&str, Tag)>) -> Tag {
    nbt::compound(fields)
}
fn l(elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: elements.first().map_or(tag_type::END, Tag::type_id),
        elements,
    })
}
fn old_item(name: &str, count: i8) -> Tag {
    c(vec![("id", s(name)), ("Count", Tag::Byte(count))])
}
fn items() -> mc_data::items::ItemRegistry {
    mc_data::items::solaris_required_items()
}
fn blocks() -> BlockRegistry {
    let names = [
        "minecraft:air",
        "minecraft:short_grass",
        "minecraft:chest",
        "minecraft:iron_chain",
    ];
    BlockRegistry::from_report(
        &names
            .into_iter()
            .enumerate()
            .map(|(id, name)| BlockReport {
                id: Identifier::parse(name).unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: id as u32,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            })
            .collect::<Vec<_>>(),
    )
    .unwrap()
}
fn old_chunk() -> Tag {
    let sections = (-4..20)
        .map(|y| {
            c(vec![
                ("Y", Tag::Byte(y)),
                (
                    "block_states",
                    c(vec![(
                        "palette",
                        l(vec![c(vec![(
                            "Name",
                            s(if y == 0 {
                                "minecraft:chest"
                            } else {
                                "minecraft:grass"
                            }),
                        )])]),
                    )]),
                ),
                (
                    "biomes",
                    c(vec![("palette", l(vec![s("minecraft:plains")]))]),
                ),
                ("BlockLight", Tag::ByteArray(vec![0; 2048])),
            ])
        })
        .collect();
    let mut item = old_item("minecraft:stone", 3);
    nbt::set(&mut item, "Slot", Tag::Byte(0));
    c(vec![
        ("DataVersion", Tag::Int(2860)),
        ("xPos", Tag::Int(0)),
        ("zPos", Tag::Int(0)),
        ("yPos", Tag::Int(-4)),
        ("Status", s("full")),
        ("sections", l(sections)),
        (
            "__context",
            c(vec![("dimension", s("minecraft:overworld"))]),
        ),
        (
            "block_entities",
            l(vec![c(vec![
                ("id", s("minecraft:chest")),
                ("x", Tag::Int(1)),
                ("y", Tag::Int(0)),
                ("z", Tag::Int(1)),
                ("Items", l(vec![item])),
            ])]),
        ),
        (
            "block_ticks",
            l(vec![c(vec![
                ("i", s("minecraft:grass")),
                ("x", Tag::Int(0)),
                ("y", Tag::Int(16)),
                ("z", Tag::Int(0)),
                ("t", Tag::Int(12)),
                ("p", Tag::Int(0)),
            ])]),
        ),
        ("InhabitedTime", Tag::Long(800)),
        (
            "mod:opaque",
            c(vec![("id", s("minecraft:grass")), ("Count", Tag::Byte(7))]),
        ),
    ])
}
fn old_entity(id: &str) -> Tag {
    c(vec![
        ("id", s(id)),
        ("UUID", Tag::IntArray(vec![0, 0, 0, 7])),
        (
            "Pos",
            l(vec![Tag::Double(1.0), Tag::Double(65.0), Tag::Double(1.0)]),
        ),
        (
            "Motion",
            l(vec![Tag::Double(0.0), Tag::Double(0.0), Tag::Double(0.0)]),
        ),
        ("Rotation", l(vec![Tag::Float(0.0), Tag::Float(0.0)])),
        ("OnGround", Tag::Byte(1)),
        ("Health", Tag::Float(10.0)),
        ("FallDistance", Tag::Float(2.5)),
    ])
}
fn roundtrip(tag: &Tag) -> Tag {
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", tag).unwrap();
    mc_nbt::read_named(&mut Cursor::new(bytes)).unwrap().1
}

#[test]
fn chunk118_reaches_existing2612_reader_with_inventory_and_ticks() {
    let mut tag = old_chunk();
    let opaque = nbt::get(&tag, "mod:opaque").unwrap().clone();
    let report = upgrade_chunk(&mut tag, 2860).unwrap();
    assert_eq!((report.from, report.to), (2860, 4790));
    assert!(
        report
            .steps
            .windows(2)
            .all(|pair| pair[0].version <= pair[1].version)
    );
    assert_eq!(nbt::get(&tag, "DataVersion"), Some(&Tag::Int(4790)));
    assert_eq!(nbt::get(&tag, "mod:opaque"), Some(&opaque));
    let decoded = crate::anvil::chunk_nbt::chunk_from_nbt_with_items(
        &roundtrip(&tag),
        &blocks(),
        Some(&items()),
        &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        crate::OVERWORLD_GEOMETRY,
    )
    .unwrap();
    assert_eq!(decoded.get_block(0, 16, 0), Some(BlockStateId(1)));
    let chest = decoded
        .chests
        .get(&crate::BlockPos { x: 1, y: 0, z: 1 })
        .unwrap();
    assert_eq!(chest.slots[0].count, 3);
    assert_eq!(decoded.scheduled_block_ticks.len(), 1);
    assert_eq!(decoded.status, "minecraft:full");
}

#[test]
fn entity_region118_reaches_existing2612_disk_reader() {
    let mut entity = old_entity("minecraft:item");
    nbt::set(&mut entity, "Item", old_item("minecraft:stone", 4));
    let mut tag = c(vec![
        ("DataVersion", Tag::Int(2860)),
        ("Position", Tag::IntArray(vec![0, 0])),
        ("Entities", l(vec![entity])),
    ]);
    upgrade_entity_region(&mut tag, 2860).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &tag).unwrap();
    crate::anvil::write_region(
        directory.path().join("r.0.0.mca"),
        &[crate::anvil::ChunkPayload {
            local_x: 0,
            local_z: 0,
            timestamp: 0,
            uncompressed_nbt: bytes,
        }],
    )
    .unwrap();
    let decoded = crate::anvil::read_entity_regions(directory.path(), &items()).unwrap();
    let entity = &decoded[&(0, 0)].entities[0];
    assert_eq!(entity.id, "minecraft:item");
    assert_eq!(entity.item.as_ref().unwrap().count, 4);
    assert_eq!(entity.pos, [1.0, 65.0, 1.0]);
}

#[test]
fn entity118_reaches_shared2612_reader_and_keeps_unknown_fields() {
    let mut tag = old_entity("minecraft:cat");
    nbt::set(&mut tag, "DataVersion", Tag::Int(2860));
    nbt::set(&mut tag, "CatType", Tag::Int(4));
    nbt::set(&mut tag, "mod:data", c(vec![("CatType", Tag::Int(4))]));
    upgrade_entity(&mut tag, 2860).unwrap();
    assert_eq!(
        nbt::get(&tag, "variant"),
        Some(&s("minecraft:british_shorthair"))
    );
    assert_eq!(nbt::get(&tag, "fall_distance"), Some(&Tag::Double(2.5)));
    let decoded = entity_from_fields(
        &items(),
        VanillaEntity::default(),
        nbt::fields(&roundtrip(&tag)).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded.id, "minecraft:cat");
    assert_eq!(decoded.uuid, uuid::Uuid::from_u128(7));
    assert_eq!(
        nbt::get(nbt::get(&tag, "mod:data").unwrap(), "CatType"),
        Some(&Tag::Int(4))
    );
}

#[test]
fn player118_reaches_shared2612_reader_with_equipment_cutover() {
    let mut main = old_item("minecraft:stone", 5);
    nbt::set(&mut main, "Slot", Tag::Byte(0));
    let mut feet = old_item("minecraft:iron_boots", 1);
    nbt::set(&mut feet, "Slot", Tag::Byte(100));
    let mut offhand = old_item("minecraft:stone", 2);
    nbt::set(&mut offhand, "Slot", Tag::Byte(-106));
    let mut tag = c(vec![
        ("DataVersion", Tag::Int(2860)),
        ("Inventory", l(vec![main, feet, offhand])),
        (
            "Pos",
            l(vec![
                Tag::Double(12.0),
                Tag::Double(80.0),
                Tag::Double(-7.0),
            ]),
        ),
        ("SelectedItemSlot", Tag::Int(0)),
        ("Health", Tag::Float(18.0)),
        ("SpawnX", Tag::Int(10)),
        ("SpawnY", Tag::Int(70)),
        ("SpawnZ", Tag::Int(20)),
        ("SpawnDimension", s("minecraft:overworld")),
    ]);
    upgrade_player(&mut tag, 2860).unwrap();
    let decoded = player_from_fields(
        &items(),
        VanillaPlayer::default(),
        nbt::fields(&roundtrip(&tag)).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded.inventory.len(), 1);
    assert_eq!(decoded.inventory[0].1.count, 5);
    assert!(
        decoded
            .equipment
            .iter()
            .any(|(slot, stack)| slot == "feet" && stack.count == 1)
    );
    assert!(
        decoded
            .equipment
            .iter()
            .any(|(slot, stack)| slot == "offhand" && stack.count == 2)
    );
    assert_eq!(decoded.pos, [12.0, 80.0, -7.0]);
    assert_eq!(decoded.health, 18.0);
    assert!(nbt::get(&tag, "SpawnX").is_none());
    assert!(nbt::get(&tag, "respawn").is_some());
}

#[test]
fn level118_emits_modern_nbt_and_saved_data_without_filesystem_actions() {
    let mut tag = c(vec![(
        "Data",
        c(vec![
            ("DataVersion", Tag::Int(2860)),
            ("LevelName", s("Hand-built old world")),
            ("Difficulty", Tag::Byte(2)),
            ("DifficultyLocked", Tag::Byte(1)),
            ("DayTime", Tag::Long(1234)),
            ("Time", Tag::Long(6789)),
            ("SpawnX", Tag::Int(16)),
            ("SpawnY", Tag::Int(70)),
            ("SpawnZ", Tag::Int(-32)),
            ("raining", Tag::Byte(1)),
            ("rainTime", Tag::Int(200)),
            ("GameRules", c(vec![("doDaylightCycle", s("true"))])),
            (
                "WorldGenSettings",
                c(vec![
                    ("seed", Tag::Long(42)),
                    ("generate_features", Tag::Byte(1)),
                    ("bonus_chest", Tag::Byte(0)),
                    ("dimensions", Tag::Compound(vec![])),
                ]),
            ),
            ("mod:preserved", Tag::LongArray(vec![9, 8, 7])),
        ]),
    )]);
    let report = upgrade_level(&mut tag, 2860).unwrap();
    let tag = roundtrip(&tag);
    let data = nbt::get(&tag, "Data").unwrap();
    assert_eq!(nbt::get(data, "DataVersion"), Some(&Tag::Int(4790)));
    assert!(nbt::get(data, "difficulty_settings").is_some());
    assert!(nbt::get(data, "WorldGenSettings").is_none());
    assert!(nbt::get(data, "world_gen_settings").is_none());
    assert_eq!(
        nbt::get(data, "LevelName"),
        Some(&s("Hand-built old world"))
    );
    assert_eq!(
        nbt::get(data, "mod:preserved"),
        Some(&Tag::LongArray(vec![9, 8, 7]))
    );
    let worldgen = report
        .saved_data
        .iter()
        .find(|record| record.path == "data/minecraft/world_gen_settings.dat")
        .unwrap();
    let worldgen = roundtrip(&worldgen.tag);
    let worldgen = nbt::get(&worldgen, "data").unwrap();
    assert_eq!(nbt::get(worldgen, "seed"), Some(&Tag::Long(42)));
    assert_eq!(
        nbt::get(worldgen, "generate_structures"),
        Some(&Tag::Byte(1))
    );
    assert!(
        report
            .saved_data
            .iter()
            .any(|record| record.path == "data/minecraft/world_clocks.dat")
    );
    for record in report.saved_data {
        let tag = roundtrip(&record.tag);
        assert_eq!(nbt::get(&tag, "DataVersion"), Some(&Tag::Int(4790)));
        assert!(nbt::get(&tag, "data").is_some());
    }
}

#[test]
fn item118_componentization_and_text_reaches2612_item_reader() {
    let mut tag = old_item("minecraft:stone", 3);
    nbt::set(
        &mut tag,
        "tag",
        c(vec![
            (
                "display",
                c(vec![("Name", s("{\"text\":\"Old stone\",\"bold\":true}"))]),
            ),
            ("mod:opaque", Tag::Int(7)),
        ]),
    );
    upgrade_item(&mut tag, 2860).unwrap();
    let tag = roundtrip(&tag);
    assert!(nbt::get(&tag, "Count").is_none());
    assert!(nbt::get(&tag, "tag").is_none());
    let decoded = mc_data::item_nbt::stack_from_fields(&items(), nbt::fields(&tag).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(decoded.count, 3);
    let components = nbt::get(&tag, "components").unwrap();
    assert_eq!(
        nbt::get(
            nbt::get(components, "minecraft:custom_data").unwrap(),
            "mod:opaque"
        ),
        Some(&Tag::Int(7))
    );
    assert!(matches!(
        nbt::get(components, "minecraft:custom_name"),
        Some(Tag::Compound(_))
    ));
}

#[test]
fn unsupported_versions_and_mismatch_are_rejected_before_mutation() {
    let tag = c(vec![
        ("DataVersion", Tag::Int(2860)),
        ("opaque", Tag::Int(9)),
    ]);
    for version in [2229, 0, -1, 4791] {
        let mut copy = tag.clone();
        assert!(matches!(
            upgrade_entity(&mut copy, version),
            Err(UpgradeError::UnsupportedVersion(_))
        ));
        assert_eq!(copy, tag);
    }
    let mut copy = tag.clone();
    assert!(matches!(
        upgrade_player(&mut copy, 3000),
        Err(UpgradeError::VersionMismatch { .. })
    ));
    assert_eq!(copy, tag);
}
#[test]
fn target_version_is_idempotent_and_skips_earlier_fixes() {
    let mut tag = c(vec![
        ("DataVersion", Tag::Int(4790)),
        ("Count", Tag::Byte(7)),
        ("mod:field", s("minecraft:grass")),
    ]);
    let original = tag.clone();
    let report = upgrade_item(&mut tag, 4790).unwrap();
    assert_eq!(tag, original);
    assert!(report.steps.is_empty());
}
#[test]
fn unknown_custom_data_is_never_interpreted_as_a_typed_record() {
    let mut tag = c(vec![
        ("DataVersion", Tag::Int(4000)),
        ("id", s("minecraft:stone")),
        ("count", Tag::Int(2)),
        (
            "components",
            c(vec![(
                "minecraft:custom_data",
                c(vec![
                    ("id", s("minecraft:boat")),
                    ("Type", s("oak")),
                    ("Count", Tag::Byte(4)),
                    ("CustomName", s("{bad JSON}")),
                ]),
            )]),
        ),
    ]);
    let opaque = nbt::get(
        nbt::get(&tag, "components").unwrap(),
        "minecraft:custom_data",
    )
    .unwrap()
    .clone();
    upgrade_item(&mut tag, 4000).unwrap();
    assert_eq!(
        nbt::get(
            nbt::get(&tag, "components").unwrap(),
            "minecraft:custom_data"
        ),
        Some(&opaque)
    );
}
#[test]
fn malformed_typed_children_do_not_receive_modern_data_version() {
    let mut tag = c(vec![
        ("DataVersion", Tag::Int(2860)),
        ("Inventory", Tag::Int(3)),
    ]);
    assert!(upgrade_player(&mut tag, 2860).is_err());
    assert_eq!(nbt::get(&tag, "DataVersion"), Some(&Tag::Int(2860)));
}

#[test]
fn chunk_dimension_context_is_transient_and_original_context_is_restored() {
    let mut tag = old_chunk();
    nbt::remove(&mut tag, "__context");
    upgrade_chunk_in_dimension(&mut tag, 2860, "minecraft:the_nether", None).unwrap();
    assert!(nbt::get(&tag, "__context").is_none());
    assert!(nbt::get(&tag, "blending_data").is_none());
    let mut tag = old_chunk();
    let original = c(vec![
        ("dimension", s("mod:unknown")),
        ("extra", Tag::Int(9)),
    ]);
    nbt::set(&mut tag, "__context", original.clone());
    upgrade_chunk_in_dimension(
        &mut tag,
        2860,
        "minecraft:overworld",
        Some("minecraft:noise"),
    )
    .unwrap();
    assert_eq!(nbt::get(&tag, "__context"), Some(&original));
    let mut rejected = c(vec![("__context", original.clone())]);
    assert!(matches!(
        upgrade_chunk_in_dimension(
            &mut rejected,
            MIN_DATA_VERSION - 1,
            "minecraft:overworld",
            Some("minecraft:noise"),
        ),
        Err(UpgradeError::UnsupportedVersion(version)) if version == MIN_DATA_VERSION - 1,
    ));
    assert_eq!(nbt::get(&rejected, "__context"), Some(&original));
}

#[test]
fn source3818_executes_later_dfu_subversions() {
    let mut tag = old_item("minecraft:stone", 2);
    nbt::set(&mut tag, "DataVersion", Tag::Int(3818));
    let report = upgrade_item(&mut tag, 3818).unwrap();
    assert_eq!(nbt::get(&tag, "count"), Some(&Tag::Int(2)));
    assert!(
        report
            .steps
            .iter()
            .any(|step| step.version == 3818 && step.fix == "ItemStackComponentizationFix")
    );
}

#[test]
fn arrow3818_created_item_is_componentized_in_same_step() {
    let mut tag = old_entity("minecraft:arrow");
    nbt::set(&mut tag, "DataVersion", Tag::Int(2860));
    nbt::set(&mut tag, "Potion", s("minecraft:swiftness"));
    upgrade_entity(&mut tag, 2860).unwrap();
    let item = nbt::get(&tag, "item").unwrap();
    assert!(nbt::get(item, "Count").is_none());
    assert_eq!(nbt::get(item, "count"), Some(&Tag::Int(1)));
    let components = nbt::get(item, "components").unwrap();
    assert_eq!(
        nbt::get(
            nbt::get(components, "minecraft:potion_contents").unwrap(),
            "potion"
        ),
        Some(&s("minecraft:swiftness"))
    );
}

#[test]
fn trade3828_removes_empty_second_cost_but_preserves_real_and_modded_items() {
    for id in [
        "minecraft:villager",
        "minecraft:zombie_villager",
        "minecraft:wandering_trader",
    ] {
        let mut tag = old_entity(id);
        nbt::set(
            &mut tag,
            "Offers",
            c(vec![(
                "Recipes",
                l(vec![
                    c(vec![(
                        "buyB",
                        c(vec![("id", s("minecraft:air")), ("count", Tag::Int(7))]),
                    )]),
                    c(vec![(
                        "buyB",
                        c(vec![("id", s("minecraft:stone")), ("count", Tag::Int(0))]),
                    )]),
                    c(vec![(
                        "buyB",
                        c(vec![("id", s("mod:coin")), ("count", Tag::Int(2))]),
                    )]),
                    c(vec![("buyB", c(vec![]))]),
                ]),
            )]),
        );
        let mut report = UpgradeReport::default();
        entities::fix_node(&mut tag, NodeKind::Entity, 3828, &mut report).unwrap();
        let recipes =
            nbt::list(nbt::get(nbt::get(&tag, "Offers").unwrap(), "Recipes").unwrap()).unwrap();
        assert!(nbt::get(&recipes[0], "buyB").is_none());
        assert!(nbt::get(&recipes[1], "buyB").is_none());
        assert!(nbt::get(&recipes[2], "buyB").is_some());
        assert!(nbt::get(&recipes[3], "buyB").is_none());
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.fix == "EmptyItemInVillagerTradeFix")
        );
    }
}

#[test]
fn banner4054_and_nested_components_reach_modern_text_shape() {
    let mut banner = c(vec![
        ("id", s("minecraft:banner")),
        (
            "CustomName",
            s("{\"translate\":\"block.minecraft.ominous_banner\",\"color\":\"gold\"}"),
        ),
    ]);
    for version in [3825, 4054, 4290] {
        walk::visit(
            &mut banner,
            NodeKind::BlockEntity,
            version,
            &mut UpgradeReport::default(),
        )
        .unwrap();
    }
    let components = nbt::get(&banner, "components").unwrap();
    assert_eq!(
        nbt::get(components, "minecraft:rarity"),
        Some(&s("uncommon"))
    );
    assert_eq!(
        nbt::get(
            nbt::get(components, "minecraft:item_name").unwrap(),
            "translate"
        ),
        Some(&s("block.minecraft.ominous_banner"))
    );
    assert!(
        nbt::get(
            nbt::get(components, "minecraft:item_name").unwrap(),
            "color"
        )
        .is_none()
    );
}

#[test]
fn sign_text_lists_remain_serializable_after_unflattening() {
    // Constantinople r.0.0.mca, chunk (0, 12), sign (1, 83, 195):
    // front_text.messages contains four JSON object strings.
    let mut sign = c(vec![
        ("id", s("minecraft:sign")),
        (
            "front_text",
            c(vec![(
                "messages",
                l((0..4).map(|_| s(r#"{"text":""}"#)).collect()),
            )]),
        ),
    ]);
    walk::visit(
        &mut sign,
        NodeKind::BlockEntity,
        4290,
        &mut UpgradeReport::default(),
    )
    .unwrap();
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &sign).unwrap();
}

#[test]
fn mixed_text_list_components_use_wrappers_and_migrate_later_passes() {
    let mut components = c(vec![(
        "minecraft:lore",
        l(vec![
            s(r#""literal""#),
            s(r#"{"text":"styled","bold":"true"}"#),
        ]),
    )]);
    for version in [4290, 4291, 4292] {
        walk::visit(
            &mut components,
            NodeKind::Components,
            version,
            &mut UpgradeReport::default(),
        )
        .unwrap();
        mc_nbt::write_named(&mut Vec::new(), "", &components).unwrap();
    }
    let Tag::List(lore) = nbt::get(&components, "minecraft:lore").unwrap() else {
        panic!("lore must remain a list");
    };
    assert_eq!(lore.element_type, tag_type::COMPOUND);
    assert_eq!(nbt::get(&lore.elements[0], ""), Some(&s("literal")));
    assert_eq!(nbt::get(&lore.elements[1], "bold"), Some(&Tag::Byte(1)));
}

#[test]
fn legacy_sign_item_hook_supplies_discriminator_before_sign_step() {
    let mut item = old_item("minecraft:oak_sign", 1);
    nbt::set(
        &mut item,
        "tag",
        c(vec![(
            "BlockEntityTag",
            c(vec![("Text1", s("{\"text\":\"Legacy sign\"}"))]),
        )]),
    );
    upgrade_item(&mut item, 2860).unwrap();
    let data = nbt::get(
        nbt::get(&item, "components").unwrap(),
        "minecraft:block_entity_data",
    )
    .unwrap();
    assert_eq!(nbt::get(data, "id"), Some(&s("minecraft:sign")));
    let messages =
        nbt::list(nbt::get(nbt::get(data, "front_text").unwrap(), "messages").unwrap()).unwrap();
    assert_eq!(nbt::get(&messages[0], "text"), Some(&s("Legacy sign")));
}

#[test]
fn unknown_namespaced_entity_and_block_entity_choices_are_opaque() {
    for kind in [NodeKind::Entity, NodeKind::BlockEntity] {
        let mut tag = c(vec![
            ("id", s("mod:opaque")),
            ("Items", l(vec![old_item("minecraft:grass", 3)])),
            ("CustomName", s("{invalid JSON}")),
        ]);
        let original = tag.clone();
        let mut report = UpgradeReport::default();
        for version in [3818, 4290, 4541] {
            walk::visit(&mut tag, kind, version, &mut report).unwrap();
        }
        assert_eq!(tag, original);
        assert_eq!(report.preserved.len(), 1);
    }
}

#[test]
fn wrapped_level_validates_and_stamps_both_declarations_only_on_success() {
    let mut level = c(vec![
        ("DataVersion", Tag::Int(4789)),
        ("Data", c(vec![("DataVersion", Tag::Int(4789))])),
        ("OpaqueRoot", s("retained")),
    ]);
    upgrade_level(&mut level, 4789).unwrap();
    assert_eq!(nbt::get(&level, "DataVersion"), Some(&Tag::Int(4790)));
    assert_eq!(
        nbt::get(nbt::get(&level, "Data").unwrap(), "DataVersion"),
        Some(&Tag::Int(4790))
    );
    assert_eq!(nbt::get(&level, "OpaqueRoot"), Some(&s("retained")));

    let mut mismatch = c(vec![
        ("DataVersion", Tag::Int(4788)),
        ("Data", c(vec![("DataVersion", Tag::Int(4789))])),
    ]);
    let original = mismatch.clone();
    assert!(matches!(
        upgrade_level(&mut mismatch, 4789),
        Err(UpgradeError::VersionMismatch { actual: 4788, .. })
    ));
    assert_eq!(mismatch, original);

    let mut malformed = c(vec![
        ("DataVersion", Tag::Int(2860)),
        (
            "Data",
            c(vec![
                ("DataVersion", Tag::Int(2860)),
                ("GameRules", Tag::Int(1)),
            ]),
        ),
    ]);
    assert!(upgrade_level(&mut malformed, 2860).is_err());
    assert_eq!(nbt::get(&malformed, "DataVersion"), Some(&Tag::Int(2860)));
    assert_eq!(
        nbt::get(nbt::get(&malformed, "Data").unwrap(), "DataVersion"),
        Some(&Tag::Int(2860))
    );
}

#[test]
fn java_1165_version_boundary_and_typed_item_rename() {
    let mut item = old_item("minecraft:grass_path", 1);
    nbt::set(&mut item, "DataVersion", Tag::Int(2586));
    let report = upgrade_item(&mut item, 2586).unwrap();
    assert_eq!((report.from, report.to), (2586, 4790));
    assert_eq!(nbt::get(&item, "id"), Some(&s("minecraft:dirt_path")));
    assert_eq!(nbt::get(&item, "DataVersion"), Some(&Tag::Int(4790)));
    assert!(report.steps.iter().any(|step| step.version == 2680));
    assert!(
        UpgradeError::UnsupportedVersion(2229)
            .to_string()
            .contains("1.15 (2230)")
    );
}

#[test]
fn java_1165_entity_region_upgrades_villager_trade_items() {
    let mut villager = old_entity("minecraft:villager");
    nbt::set(
        &mut villager,
        "Offers",
        c(vec![(
            "Recipes",
            l(vec![c(vec![
                ("buy", old_item("minecraft:grass_path", 2)),
                ("sell", old_item("minecraft:stone", 1)),
                ("uses", Tag::Int(0)),
                ("maxUses", Tag::Int(12)),
            ])]),
        )]),
    );
    let mut tag = c(vec![
        ("DataVersion", Tag::Int(2586)),
        ("Position", Tag::IntArray(vec![-1, 2])),
        ("Entities", l(vec![villager])),
        ("mod:opaque", old_item("minecraft:grass_path", 2)),
    ]);
    upgrade_entity_region(&mut tag, 2586).unwrap();
    let villager = &nbt::list(nbt::get(&tag, "Entities").unwrap()).unwrap()[0];
    let recipe =
        &nbt::list(nbt::get(nbt::get(villager, "Offers").unwrap(), "Recipes").unwrap()).unwrap()[0];
    let buy = nbt::get(recipe, "buy").unwrap();
    assert_eq!(nbt::get(buy, "id"), Some(&s("minecraft:dirt_path")));
    assert_eq!(nbt::get(buy, "count"), Some(&Tag::Int(2)));
    assert_eq!(
        nbt::get(&tag, "Position"),
        Some(&Tag::IntArray(vec![-1, 2]))
    );
    assert_eq!(
        nbt::get(&tag, "mod:opaque"),
        Some(&old_item("minecraft:grass_path", 2))
    );
}

#[test]
fn java_1165_chunk_fixture_reaches_modern_reader_layout() {
    for version in [2230, 2586] {
        let mut chunk = c(vec![
            ("DataVersion", Tag::Int(version)),
            (
                "__context",
                c(vec![
                    ("dimension", s("minecraft:overworld")),
                    ("generator", s("minecraft:noise")),
                ]),
            ),
            (
                "Level",
                c(vec![
                    ("xPos", Tag::Int(-1)),
                    ("zPos", Tag::Int(2)),
                    ("Status", s("full")),
                    ("Biomes", Tag::IntArray(vec![1; 1024])),
                    (
                        "Sections",
                        l(vec![c(vec![
                            ("Y", Tag::Byte(0)),
                            (
                                "Palette",
                                l(vec![c(vec![("Name", s("minecraft:bedrock"))])]),
                            ),
                            ("BlockStates", Tag::LongArray(vec![0; 256])),
                        ])]),
                    ),
                    (
                        "TileEntities",
                        l(vec![c(vec![
                            ("id", s("minecraft:chest")),
                            ("x", Tag::Int(-15)),
                            ("y", Tag::Int(1)),
                            ("z", Tag::Int(33)),
                            ("Items", l(vec![old_item("minecraft:grass_path", 3)])),
                        ])]),
                    ),
                    (
                        "Entities",
                        l(vec![c(vec![
                            ("id", s("minecraft:item_frame")),
                            ("Item", old_item("minecraft:grass_path", 1)),
                        ])]),
                    ),
                    (
                        "Structures",
                        c(vec![("Starts", c(vec![])), ("References", c(vec![]))]),
                    ),
                ]),
            ),
        ]);
        upgrade_chunk_in_dimension(
            &mut chunk,
            version,
            "minecraft:overworld",
            Some("minecraft:noise"),
        )
        .unwrap();
        assert!(nbt::get(&chunk, "Level").is_none());
        assert_eq!(nbt::get(&chunk, "DataVersion"), Some(&Tag::Int(4790)));
        assert!(nbt::get(&chunk, "yPos").is_none());
        let sections = nbt::list(nbt::get(&chunk, "sections").unwrap()).unwrap();
        assert_eq!(sections.len(), 24);
        for section in sections {
            assert!(nbt::get(section, "block_states").is_some());
            let biomes = nbt::get(section, "biomes").unwrap();
            assert_eq!(
                nbt::get(biomes, "palette"),
                Some(&l(vec![s("minecraft:plains")])),
            );
            assert!(nbt::get(section, "Palette").is_none());
            assert!(nbt::get(section, "BlockStates").is_none());
        }
        let chest = &nbt::list(nbt::get(&chunk, "block_entities").unwrap()).unwrap()[0];
        let stack = &nbt::list(nbt::get(chest, "Items").unwrap()).unwrap()[0];
        assert_eq!(nbt::get(stack, "id"), Some(&s("minecraft:dirt_path")));
        assert_eq!(nbt::get(stack, "count"), Some(&Tag::Int(3)));
        let frame = &nbt::list(nbt::get(&chunk, "entities").unwrap()).unwrap()[0];
        assert_eq!(
            nbt::get(nbt::get(frame, "Item").unwrap(), "id"),
            Some(&s("minecraft:dirt_path")),
        );
        assert!(nbt::get(nbt::get(&chunk, "structures").unwrap(), "starts").is_some());
    }
}

#[test]
fn java_1165_entity_schema_children_reach_shared_fixes() {
    let mut rocket = old_entity("minecraft:firework_rocket");
    nbt::set(
        &mut rocket,
        "FireworksItem",
        old_item("minecraft:grass_path", 1),
    );
    upgrade_entity(&mut rocket, 2586).unwrap();
    let stack = nbt::get(&rocket, "FireworksItem").unwrap();
    assert_eq!(nbt::get(stack, "id"), Some(&s("minecraft:dirt_path")));
    assert_eq!(nbt::get(stack, "count"), Some(&Tag::Int(1)));
    assert!(nbt::get(stack, "Count").is_none());

    let mut falling = old_entity("minecraft:falling_block");
    nbt::set(
        &mut falling,
        "TileEntityData",
        c(vec![
            ("id", s("minecraft:chest")),
            ("Items", l(vec![old_item("minecraft:grass_path", 3)])),
        ]),
    );
    upgrade_entity(&mut falling, 2586).unwrap();
    let block = nbt::get(&falling, "TileEntityData").unwrap();
    let stack = &nbt::list(nbt::get(block, "Items").unwrap()).unwrap()[0];
    assert_eq!(nbt::get(stack, "id"), Some(&s("minecraft:dirt_path")));
    assert_eq!(nbt::get(stack, "count"), Some(&Tag::Int(3)));

    let mut cart = old_entity("minecraft:command_block_minecart");
    nbt::set(&mut cart, "LastOutput", s(r#"{"text":"from 1.16.5"}"#));
    upgrade_entity(&mut cart, 2586).unwrap();
    assert_eq!(
        nbt::get(nbt::get(&cart, "LastOutput").unwrap(), "text"),
        Some(&s("from 1.16.5")),
    );
}

#[test]
fn java_1165_trident_stack_keeps_early_block_name_fixes() {
    let mut stack = old_item("minecraft:trident", 1);
    nbt::set(
        &mut stack,
        "tag",
        c(vec![("CanDestroy", l(vec![s("minecraft:grass_path")]))]),
    );
    let mut trident = old_entity("minecraft:trident");
    nbt::set(&mut trident, "Trident", stack);
    walk::visit(
        &mut trident,
        NodeKind::Entity,
        2680,
        &mut UpgradeReport::default(),
    )
    .unwrap();
    let item = nbt::get(&trident, "Trident").unwrap();
    assert_eq!(
        nbt::get(nbt::get(item, "tag").unwrap(), "CanDestroy"),
        Some(&l(vec![s("minecraft:dirt_path")])),
    );
}

#[test]
fn legacy_entity_children_are_typed_before_namespaced_ids() {
    for (id, typed) in [
        ("FireworksRocketEntity", true),
        ("minecraft:fireworks_rocket", false),
        ("mod:FireworksRocketEntity", false),
    ] {
        let mut rocket = c(vec![
            ("id", s(id)),
            (
                "FireworksItem",
                c(vec![("id", Tag::Short(1)), ("Count", Tag::Byte(1))]),
            ),
        ]);
        walk::visit(
            &mut rocket,
            NodeKind::Entity,
            102,
            &mut UpgradeReport::default(),
        )
        .unwrap();
        let item = nbt::get(&rocket, "FireworksItem").unwrap();
        let expected = if typed {
            s("minecraft:stone")
        } else {
            Tag::Short(1)
        };
        assert_eq!(nbt::get(item, "id"), Some(&expected), "{id}");
    }

    let mut falling = c(vec![
        ("id", s("FallingSand")),
        (
            "TileEntityData",
            c(vec![
                ("id", s("Chest")),
                (
                    "Items",
                    l(vec![c(vec![
                        ("id", Tag::Short(1)),
                        ("Count", Tag::Byte(1)),
                    ])]),
                ),
            ]),
        ),
    ]);
    walk::visit(
        &mut falling,
        NodeKind::Entity,
        102,
        &mut UpgradeReport::default(),
    )
    .unwrap();
    let block = nbt::get(&falling, "TileEntityData").unwrap();
    let item = &nbt::list(nbt::get(block, "Items").unwrap()).unwrap()[0];
    assert_eq!(nbt::get(item, "id"), Some(&s("minecraft:stone")));

    let mut riding = c(vec![
        ("id", s("Pig")),
        (
            "Riding",
            c(vec![
                ("id", s("FireworksRocketEntity")),
                (
                    "FireworksItem",
                    c(vec![("id", Tag::Short(1)), ("Count", Tag::Byte(1))]),
                ),
            ]),
        ),
    ]);
    walk::visit(
        &mut riding,
        NodeKind::Entity,
        102,
        &mut UpgradeReport::default(),
    )
    .unwrap();
    let mount = nbt::get(&riding, "Riding").unwrap();
    let stack = nbt::get(mount, "FireworksItem").unwrap();
    assert_eq!(nbt::get(stack, "id"), Some(&s("minecraft:stone")));
}

#[test]
fn legacy_potion_item_child_uses_its_historical_schema() {
    for (id, kind, version, typed) in [
        ("ThrownPotion", NodeKind::Entity, 102, true),
        ("minecraft:potion", NodeKind::Entity, 2509, true),
        ("minecraft:potion", NodeKind::Entity, 2512, false),
        ("ThrownPotion", NodeKind::Entity, 2509, false),
        ("minecraft:potion", NodeKind::Player, 2509, false),
        ("minecraft:arrow", NodeKind::Entity, 2509, false),
        ("mod:potion", NodeKind::Entity, 2509, false),
    ] {
        let mut entity = c(vec![("id", s(id)), ("Potion", s("opaque"))]);
        let result = walk::visit(&mut entity, kind, version, &mut UpgradeReport::default());
        assert_eq!(result.is_err(), typed, "{id} {kind:?} at {version}");
        if !typed {
            assert_eq!(nbt::get(&entity, "Potion"), Some(&s("opaque")));
        }
    }
}

#[test]
fn legacy_aux_negative_stamps_use_the_earliest_schema() {
    let stack = c(vec![("id", Tag::Short(1)), ("Count", Tag::Byte(2))]);
    let mut player = c(vec![
        ("DataVersion", Tag::Int(-2)),
        ("Inventory", l(vec![stack.clone()])),
    ]);
    let report = upgrade_player(&mut player, -2).unwrap();
    assert_eq!(report.from, -2);
    let inventory = nbt::list(nbt::get(&player, "Inventory").unwrap()).unwrap();
    assert_eq!(nbt::get(&inventory[0], "id"), Some(&s("minecraft:stone")));
    assert_eq!(nbt::get(&inventory[0], "count"), Some(&Tag::Int(2)));

    let mut item = stack;
    nbt::set(&mut item, "DataVersion", Tag::Int(-2));
    let report = upgrade_item(&mut item, -2).unwrap();
    assert_eq!(report.from, -2);
    assert_eq!(nbt::get(&item, "id"), Some(&s("minecraft:stone")));
    assert_eq!(nbt::get(&item, "count"), Some(&Tag::Int(2)));
}

#[test]
fn legacy_schema_children_preserve_untyped_owner_remainders() {
    for (kind, id, version, key, value, typed) in [
        (NodeKind::Player, "Pig", 102, "Riding", s("opaque"), false),
        (NodeKind::Player, "Pig", 99, "Equipment", s("opaque"), false),
        (
            NodeKind::Player,
            "Pig",
            102,
            "Passengers",
            s("opaque"),
            false,
        ),
        (NodeKind::Player, "Pig", 102, "Item", s("opaque"), false),
        (NodeKind::Player, "Pig", 102, "Inventory", s("opaque"), true),
        (
            NodeKind::Player,
            "Pig",
            134,
            "RootVehicle",
            c(vec![("Entity", s("opaque"))]),
            false,
        ),
        (
            NodeKind::Player,
            "Pig",
            135,
            "RootVehicle",
            c(vec![("Entity", s("opaque"))]),
            true,
        ),
        (
            NodeKind::Entity,
            "Pig",
            135,
            "RootVehicle",
            c(vec![("Entity", s("opaque"))]),
            false,
        ),
        (
            NodeKind::Player,
            "Pig",
            1021,
            "ShoulderEntityLeft",
            s("opaque"),
            false,
        ),
        (
            NodeKind::Player,
            "Pig",
            1022,
            "ShoulderEntityLeft",
            s("opaque"),
            true,
        ),
        (
            NodeKind::Entity,
            "Pig",
            1022,
            "ShoulderEntityLeft",
            s("opaque"),
            false,
        ),
        (
            NodeKind::Player,
            "minecraft:falling_block",
            1456,
            "BlockState",
            s("opaque"),
            false,
        ),
        (
            NodeKind::Entity,
            "minecraft:falling_block",
            1450,
            "BlockState",
            s("opaque"),
            false,
        ),
        (
            NodeKind::Entity,
            "minecraft:pig",
            1456,
            "BlockState",
            s("opaque"),
            false,
        ),
        (
            NodeKind::Entity,
            "minecraft:falling_block",
            1456,
            "BlockState",
            s("opaque"),
            true,
        ),
        (
            NodeKind::Entity,
            "minecraft:enderman",
            1456,
            "carriedBlockState",
            s("opaque"),
            true,
        ),
        (
            NodeKind::Entity,
            "minecraft:arrow",
            1456,
            "inBlockState",
            s("opaque"),
            true,
        ),
        (
            NodeKind::Entity,
            "minecraft:spectral_arrow",
            1456,
            "inBlockState",
            s("opaque"),
            true,
        ),
        (
            NodeKind::Entity,
            "minecraft:trident",
            1474,
            "inBlockState",
            s("opaque"),
            true,
        ),
        (
            NodeKind::Entity,
            "minecraft:minecart",
            1456,
            "DisplayState",
            s("opaque"),
            true,
        ),
    ] {
        let mut tag = c(vec![("id", s(id)), (key, value)]);
        let before = nbt::get(&tag, key).unwrap().clone();
        let result = walk::visit(&mut tag, kind, version, &mut UpgradeReport::default());
        assert_eq!(result.is_err(), typed, "{kind:?} {id}.{key} at {version}");
        if !typed {
            assert_eq!(nbt::get(&tag, key), Some(&before), "{kind:?} {id}.{key}");
        }
    }
}

#[test]
fn java_115_level_generator_reaches_current_saved_data() {
    let mut tag = c(vec![
        ("DataVersion", Tag::Int(2230)),
        ("RandomSeed", Tag::Long(-812)),
        ("generatorName", s("default")),
        ("generatorVersion", Tag::Int(1)),
        ("MapFeatures", Tag::Byte(0)),
        ("BonusChest", Tag::Byte(1)),
    ]);
    let report = upgrade_level(&mut tag, 2230).unwrap();
    assert_eq!(nbt::get(&tag, "DataVersion"), Some(&Tag::Int(4790)));
    let worldgen = report
        .saved_data
        .iter()
        .find(|record| record.path == "data/minecraft/world_gen_settings.dat")
        .unwrap();
    let data = nbt::get(&worldgen.tag, "data").unwrap();
    assert_eq!(nbt::get(data, "seed"), Some(&Tag::Long(-812)));
    assert_eq!(nbt::get(data, "generate_structures"), Some(&Tag::Byte(0)));
    assert_eq!(nbt::get(data, "bonus_chest"), Some(&Tag::Byte(1)));
    let dimensions = nbt::get(data, "dimensions").unwrap();
    for dimension in [
        "minecraft:overworld",
        "minecraft:the_nether",
        "minecraft:the_end",
    ] {
        assert!(nbt::get(dimensions, dimension).is_some(), "{dimension}");
    }
}
