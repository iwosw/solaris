use super::*;
use crate::anvil::region;
use crate::chunk::{MAX_Y, MIN_SECTION_Y, MIN_Y};
use mc_data::loot::chest_26_1_2::ChestLootCatalog;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(rel)
}

fn top_non_air_y(chunk: &Chunk, x: u8, z: u8, air: BlockStateId) -> Option<i32> {
    (MIN_Y..MAX_Y)
        .rev()
        .find(|&y| chunk.get_block(x, y, z) != Some(air))
}

fn extra<'a>(chunk: &'a Chunk, key: &str) -> Option<&'a Tag> {
    chunk
        .extras
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

fn non_metadata_extras(chunk: &Chunk) -> Vec<&(String, Tag)> {
    chunk
        .extras
        .iter()
        .filter(|(name, _)| !VANILLA_METADATA_KEYS.contains(&name.as_str()))
        .collect()
}

fn assert_normalized_metadata(original: &Chunk, decoded: &Chunk) {
    let expected_data_version = match extra(original, "DataVersion") {
        Some(Tag::Int(value)) => *value,
        _ => TARGET_DATA_VERSION,
    };
    let expected_last_update = match extra(original, "LastUpdate") {
        Some(Tag::Long(value)) => *value,
        _ => 0,
    };
    let expected_inhabited_time = match extra(original, "InhabitedTime") {
        Some(Tag::Long(value)) => *value,
        _ => 0,
    };
    assert_eq!(
        extra(decoded, "DataVersion"),
        Some(&Tag::Int(expected_data_version))
    );
    assert_eq!(
        extra(decoded, "LastUpdate"),
        Some(&Tag::Long(expected_last_update))
    );
    assert_eq!(
        extra(decoded, "InhabitedTime"),
        Some(&Tag::Long(expected_inhabited_time))
    );
}

#[test]
fn unopened_chest_preserves_vanilla_table_seed_and_materialized_components() {
    let items = mc_data::items::solaris_required_items();
    let source = vec![
        (
            "LootTable".into(),
            Tag::String("minecraft:chests/simple_dungeon".into()),
        ),
        (
            "LootTableSeed".into(),
            Tag::Long(-9_223_372_036_854_775_000),
        ),
    ];
    let pending = decode_chest(&source, &items).unwrap();
    assert_eq!(
        pending.loot_table.as_ref().unwrap().as_str(),
        "minecraft:chests/simple_dungeon"
    );
    assert_eq!(pending.loot_seed, -9_223_372_036_854_775_000);
    assert!(pending.slots.iter().all(FurnaceSlot::is_empty));
    let position = BlockPos {
        x: -17,
        y: 64,
        z: 33,
    };
    let encoded = encode_chest(&position, &pending, &items, "minecraft:chest").unwrap();
    let fields = expect_compound(&encoded, "chest").unwrap();
    assert!(!fields.iter().any(|(key, _)| key == "Items"));
    assert_eq!(decode_chest(fields, &items).unwrap(), pending);

    let mut populated = ChestBlockEntity::default();
    let book = Identifier::parse("minecraft:enchanted_book").unwrap();
    populated.slots[4].item_id = items.id_of(&book).unwrap();
    populated.slots[4].count = 1;
    populated.slots[4].extended_mut().stored_enchantments = vec![mc_data::ItemEnchantment {
        id: Identifier::parse("minecraft:looting").unwrap(),
        level: 3,
    }];
    let encoded = encode_chest(&position, &populated, &items, "minecraft:barrel").unwrap();
    let fields = expect_compound(&encoded, "barrel").unwrap();
    assert!(
        !fields
            .iter()
            .any(|(key, _)| key == "LootTable" || key == "LootTableSeed")
    );
    assert_eq!(decode_chest(fields, &items).unwrap(), populated);
}
#[test]
fn furnace_timers_accept_integral_tags_and_reject_overflow() {
    let items = ItemRegistry::from_report(&[]);
    let names = [
        "lit_time_remaining",
        "lit_total_time",
        "cooking_time_spent",
        "cooking_total_time",
    ];
    for tag in [
        Tag::Byte(100),
        Tag::Short(100),
        Tag::Int(100),
        Tag::Long(100),
    ] {
        let fields: Vec<_> = names
            .iter()
            .map(|name| ((*name).to_owned(), tag.clone()))
            .collect();
        let furnace = decode_furnace(&fields, &items).unwrap();
        assert_eq!(furnace.burn_remaining, 100);
        assert_eq!(furnace.burn_total, 100);
        assert_eq!(furnace.cook_progress, 100);
        assert_eq!(furnace.cook_total, 100);
    }
    for name in names {
        for value in [i64::MIN, -32769, 32768, i64::MAX] {
            assert!(matches!(
                decode_furnace(&[(name.to_owned(), Tag::Long(value))], &items),
                Err(ChunkNbtError::NumericOutOfRange { field, value: actual })
                    if field == name && actual == value
            ));
        }
        for value in [i32::MIN, -32769, 32768, i32::MAX] {
            assert!(matches!(
                decode_furnace(&[(name.to_owned(), Tag::Int(value))], &items),
                Err(ChunkNbtError::NumericOutOfRange { .. })
            ));
        }
        for value in [i16::MIN, i16::MAX] {
            assert!(
                decode_furnace(&[(name.to_owned(), Tag::Int(i32::from(value)))], &items).is_ok()
            );
        }
        for tag in [
            Tag::Float(100.0),
            Tag::Double(100.0),
            Tag::String("100".into()),
        ] {
            assert!(matches!(
                decode_furnace(&[(name.to_owned(), tag)], &items),
                Err(ChunkNbtError::WrongType { .. })
            ));
        }
    }
}

#[test]
fn furnace_family_block_entity_ids_follow_block_state() {
    fn block_report(id: u32, name: &str) -> mc_data::blocks::BlockReport {
        mc_data::blocks::BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }
    }

    let registry = BlockRegistry::from_report(&[
        block_report(0, "minecraft:air"),
        block_report(1, "minecraft:furnace"),
        block_report(2, "minecraft:smoker"),
        block_report(3, "minecraft:blast_furnace"),
    ])
    .unwrap();
    let items = ItemRegistry::from_report(&[]);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome);
    let entries = [
        (
            BlockPos { x: 1, y: 64, z: 1 },
            BlockStateId(1),
            "minecraft:furnace",
        ),
        (
            BlockPos { x: 2, y: 64, z: 1 },
            BlockStateId(2),
            "minecraft:smoker",
        ),
        (
            BlockPos { x: 3, y: 64, z: 1 },
            BlockStateId(3),
            "minecraft:blast_furnace",
        ),
    ];
    for &(pos, state, _) in &entries {
        chunk
            .set_block(pos.x as u8, pos.y, pos.z as u8, state)
            .unwrap();
        let mut furnace = FurnaceBlockEntity::default();
        furnace
            .recipes_used
            .insert("minecraft:test_smelting".to_string(), 3);
        chunk.furnaces.insert(pos, furnace);
    }

    let mut root = chunk_to_nbt_with_items(&chunk, &registry, Some(&items)).unwrap();
    let Tag::Compound(root_cmp) = &mut root else {
        panic!("chunk root must be a compound");
    };
    let Tag::List(block_entities) = root_cmp
        .iter_mut()
        .find(|(key, _)| key == "block_entities")
        .map(|(_, tag)| tag)
        .expect("block_entities list")
    else {
        panic!("block_entities must be a list");
    };
    // The Constantinople import contains Int timers, unlike vanilla 26.1.2's
    // Short writer. Exercise the complete chunk importer for every furnace kind.
    for entity in &mut block_entities.elements {
        let Tag::Compound(fields) = entity else {
            panic!("block entity must be a compound");
        };
        for (name, tag) in fields {
            if matches!(
                name.as_str(),
                "lit_time_remaining"
                    | "lit_total_time"
                    | "cooking_time_spent"
                    | "cooking_total_time"
            ) {
                let Tag::Short(value) = tag else {
                    panic!("exported furnace timer must be a short");
                };
                *tag = Tag::Int(i32::from(*value));
            }
        }
    }
    let ids: Vec<&str> = block_entities
        .elements
        .iter()
        .map(|tag| {
            let Tag::Compound(cmp) = tag else {
                panic!("block entity must be a compound");
            };
            get_string(cmp, "id").unwrap().as_str()
        })
        .collect();
    assert_eq!(
        ids,
        entries.iter().map(|(_, _, id)| *id).collect::<Vec<&str>>()
    );

    let decoded = chunk_from_nbt_with_items(
        &root,
        &registry,
        Some(&items),
        &ChestLootCatalog::new(),
        crate::OVERWORLD_GEOMETRY,
    )
    .unwrap();
    for &(pos, _, _) in &entries {
        assert_eq!(
            decoded.furnaces[&pos]
                .recipes_used
                .get("minecraft:test_smelting"),
            Some(&3)
        );
    }
}

#[test]
fn barrel_block_entity_id_follows_block_state() {
    fn block_report(id: u32, name: &str) -> mc_data::blocks::BlockReport {
        mc_data::blocks::BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }
    }

    let registry = BlockRegistry::from_report(&[
        block_report(0, "minecraft:air"),
        block_report(1, "minecraft:chest"),
        block_report(2, "minecraft:barrel"),
    ])
    .unwrap();
    let items = ItemRegistry::from_report(&[]);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome);
    let entries = [
        (
            BlockPos { x: 1, y: 64, z: 1 },
            BlockStateId(1),
            "minecraft:chest",
        ),
        (
            BlockPos { x: 2, y: 64, z: 1 },
            BlockStateId(2),
            "minecraft:barrel",
        ),
    ];
    for &(pos, state, _) in &entries {
        chunk
            .set_block(pos.x as u8, pos.y, pos.z as u8, state)
            .unwrap();
        chunk.chests.insert(pos, ChestBlockEntity::default());
    }

    let root = chunk_to_nbt_with_items(&chunk, &registry, Some(&items)).unwrap();
    let Tag::Compound(root_cmp) = &root else {
        panic!("chunk root must be a compound");
    };
    let Tag::List(block_entities) = root_cmp
        .iter()
        .find(|(key, _)| key == "block_entities")
        .map(|(_, tag)| tag)
        .expect("block_entities list")
    else {
        panic!("block_entities must be a list");
    };
    let ids: Vec<&str> = block_entities
        .elements
        .iter()
        .map(|tag| {
            let Tag::Compound(cmp) = tag else {
                panic!("block entity must be a compound");
            };
            get_string(cmp, "id").unwrap().as_str()
        })
        .collect();
    assert_eq!(
        ids,
        entries.iter().map(|(_, _, id)| *id).collect::<Vec<&str>>()
    );

    let decoded = chunk_from_nbt_with_items(
        &root,
        &registry,
        Some(&items),
        &ChestLootCatalog::new(),
        crate::OVERWORLD_GEOMETRY,
    )
    .unwrap();
    for &(pos, _, _) in &entries {
        assert!(decoded.chests.contains_key(&pos));
    }
}

#[test]
fn hopper_block_entity_items_round_trip_through_anvil() {
    fn block_report(id: u32, name: &str) -> mc_data::blocks::BlockReport {
        mc_data::blocks::BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }
    }

    let registry = BlockRegistry::from_report(&[
        block_report(0, "minecraft:air"),
        block_report(1, "minecraft:hopper"),
    ])
    .unwrap();
    let items = ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:cobblestone").unwrap(),
            protocol_id: 10,
        },
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 11,
        },
    ]);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome);
    let pos = BlockPos { x: 4, y: 64, z: 5 };
    chunk.set_block(4, 64, 5, BlockStateId(1)).unwrap();
    let mut hopper = HopperBlockEntity {
        transfer_cooldown: 5,
        ..Default::default()
    };
    hopper.slots[0] = FurnaceSlot {
        count: 64,
        item_id: 10,
        damage: Some(7),
        enchantments: vec![mc_data::ItemEnchantment {
            id: Identifier::parse("minecraft:efficiency").unwrap(),
            level: 1,
        }],
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
        components: None,
    };
    hopper.slots[4] = FurnaceSlot {
        count: 3,
        item_id: 11,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
        components: None,
    };
    chunk.hoppers.insert(pos, hopper.clone());

    let root = chunk_to_nbt_with_items(&chunk, &registry, Some(&items)).unwrap();
    let Tag::Compound(root_cmp) = &root else {
        panic!("chunk root must be a compound");
    };
    let Tag::List(block_entities) = root_cmp
        .iter()
        .find(|(key, _)| key == "block_entities")
        .map(|(_, tag)| tag)
        .expect("block_entities list")
    else {
        panic!("block_entities must be a list");
    };
    assert_eq!(block_entities.elements.len(), 1);
    let Tag::Compound(cmp) = &block_entities.elements[0] else {
        panic!("hopper block entity must be a compound");
    };
    assert_eq!(get_string(cmp, "id").unwrap(), "minecraft:hopper");
    assert_eq!(get_int(cmp, "TransferCooldown").unwrap(), 5);

    let decoded = chunk_from_nbt_with_items(
        &root,
        &registry,
        Some(&items),
        &ChestLootCatalog::new(),
        crate::OVERWORLD_GEOMETRY,
    )
    .unwrap();
    assert_eq!(decoded.hoppers.get(&pos), Some(&hopper));
}

#[test]
fn typed_container_slots_accept_vanilla_bytes_and_existing_ints() {
    let reports: Vec<_> = [
        "minecraft:air",
        "minecraft:chest",
        "minecraft:furnace",
        "minecraft:hopper",
        "minecraft:brewing_stand",
    ]
    .into_iter()
    .enumerate()
    .map(|(id, name)| mc_data::blocks::BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: id as u32,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    })
    .collect();
    let registry = BlockRegistry::from_report(&reports).unwrap();
    let items = mc_data::items::solaris_required_items();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let positions = std::array::from_fn::<_, 4, _>(|index| BlockPos {
        x: index as i32 + 1,
        y: 64,
        z: 1,
    });
    for (index, pos) in positions.iter().enumerate() {
        chunk
            .set_block(
                pos.x as u8,
                pos.y,
                pos.z as u8,
                BlockStateId(index as u32 + 1),
            )
            .unwrap();
    }
    let stack = FurnaceSlot {
        item_id: items
            .id_of(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap(),
        count: 7,
        custom_name: Some("slot codec custody".into()),
        ..Default::default()
    };
    let mut chest = ChestBlockEntity::default();
    chest.slots[26] = stack.clone();
    chunk.chests.insert(positions[0], chest.clone());
    let mut furnace = FurnaceBlockEntity::default();
    furnace.slots[2] = stack.clone();
    chunk.furnaces.insert(positions[1], furnace.clone());
    let mut hopper = HopperBlockEntity {
        transfer_cooldown: 7,
        ..Default::default()
    };
    hopper.slots[4] = stack.clone();
    chunk.hoppers.insert(positions[2], hopper.clone());
    let mut stand = BrewingStandBlockEntity {
        brew_time: 263,
        fuel: 19,
        ..Default::default()
    };
    stand.slots[3] = stack;
    stand.ingredient = Some(stand.slots[3].item_id);
    chunk.brewing_stands.insert(positions[3], stand.clone());

    for byte_slots in [false, true] {
        let mut root = chunk_to_nbt_with_items(&chunk, &registry, Some(&items)).unwrap();
        let Tag::Compound(fields) = &mut root else {
            panic!("chunk compound")
        };
        let (_, Tag::List(entities)) = fields
            .iter_mut()
            .find(|(name, _)| name == "block_entities")
            .unwrap()
        else {
            panic!("block entity list");
        };
        for entity in &mut entities.elements {
            let Tag::Compound(fields) = entity else {
                panic!("block entity compound")
            };
            fields.push((
                "CustomName".into(),
                Tag::String("Preserved container".into()),
            ));
            fields.push((
                "lock".into(),
                Tag::Compound(vec![("components".into(), Tag::Compound(vec![]))]),
            ));
            fields.push((
                "components".into(),
                Tag::Compound(vec![("test:metadata".into(), Tag::Long(17))]),
            ));
            let (_, Tag::List(stacks)) =
                fields.iter_mut().find(|(name, _)| name == "Items").unwrap()
            else {
                panic!("inventory list");
            };
            for stack in &mut stacks.elements {
                let Tag::Compound(fields) = stack else {
                    panic!("item compound")
                };
                let (_, slot) = fields.iter_mut().find(|(name, _)| name == "Slot").unwrap();
                let value = match slot {
                    Tag::Int(value) => *value,
                    Tag::Byte(value) => i32::from(*value),
                    _ => panic!("numeric inventory slot"),
                };
                *slot = if byte_slots {
                    Tag::Byte(value as i8)
                } else {
                    Tag::Int(value)
                };
            }
        }
        let mut decoded = chunk_from_nbt_with_items(
            &root,
            &registry,
            Some(&items),
            &ChestLootCatalog::new(),
            crate::OVERWORLD_GEOMETRY,
        )
        .unwrap();
        assert_eq!(decoded.chests[&positions[0]], chest);
        assert_eq!(decoded.furnaces[&positions[1]], furnace);
        assert_eq!(decoded.hoppers[&positions[2]], hopper);
        assert_eq!(decoded.brewing_stands[&positions[3]], stand);
        // Typed inventory edits must not discard metadata or resurrect old items.
        decoded
            .chests
            .get_mut(&positions[0])
            .unwrap()
            .slots
            .fill(FurnaceSlot::default());
        decoded
            .furnaces
            .get_mut(&positions[1])
            .unwrap()
            .slots
            .fill(FurnaceSlot::default());
        decoded
            .hoppers
            .get_mut(&positions[2])
            .unwrap()
            .slots
            .fill(FurnaceSlot::default());
        decoded
            .brewing_stands
            .get_mut(&positions[3])
            .unwrap()
            .slots
            .fill(FurnaceSlot::default());
        let encoded = chunk_to_nbt_with_items(&decoded, &registry, Some(&items)).unwrap();
        let fields = expect_compound(&encoded, "chunk").unwrap();
        for entity in &get_list(fields, "block_entities").unwrap().elements {
            let fields = expect_compound(entity, "container").unwrap();
            assert_eq!(
                get_string(fields, "CustomName").unwrap(),
                "Preserved container"
            );
            assert!(matches!(
                get_field(fields, "lock").unwrap(),
                Tag::Compound(_)
            ));
            assert!(matches!(
                get_field(fields, "components").unwrap(),
                Tag::Compound(_)
            ));
            assert!(get_list(fields, "Items").unwrap().elements.is_empty());
        }
    }
}

/// The big M2 acceptance test: load every chunk in the real
/// .mca, decode it, re-encode it, decode again, and verify the
/// modelled state is bit-identical between the two decoded
/// chunks. The local oracle and sidecar make this an opt-in gate.
#[test]
#[ignore = "requires local .analysis/test-world and 26.1.2 blocks report"]
fn round_trip_real_vanilla_chunks() {
    let region_path = workspace_path(".analysis/test-world/region/r.0.0.mca");
    let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
    assert!(
        region_path.is_file() && blocks_path.is_file(),
        "need both {} and {}",
        region_path.display(),
        blocks_path.display()
    );
    let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
    let registry = BlockRegistry::from_report(&report).unwrap();
    let payloads = region::read_region(&region_path).unwrap();

    let mut probed = 0usize;
    for payload in &payloads {
        let mut cur = Cursor::new(&payload.uncompressed_nbt[..]);
        let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
        let chunk1 =
            chunk_from_nbt(&root, &registry, crate::OVERWORLD_GEOMETRY).expect("decode #1");

        let root2 = chunk_to_nbt(&chunk1, &registry).expect("encode");
        let mut buf2 = Vec::new();
        mc_nbt::write_named(&mut buf2, "", &root2).unwrap();
        let mut cur2 = Cursor::new(&buf2[..]);
        let (_, root3) = mc_nbt::read_named(&mut cur2).unwrap();
        let chunk2 =
            chunk_from_nbt(&root3, &registry, crate::OVERWORLD_GEOMETRY).expect("decode #2");

        assert_eq!(chunk1.pos, chunk2.pos, "ChunkPos");
        assert_eq!(chunk1.status, chunk2.status, "Status");

        for y in MIN_Y..MAX_Y {
            for z in 0..16u8 {
                for x in 0..16u8 {
                    assert_eq!(
                        chunk1.get_block(x, y, z),
                        chunk2.get_block(x, y, z),
                        "block at ({x},{y},{z}) in chunk {:?}",
                        chunk1.pos
                    );
                }
            }
        }

        for (i, (b1, b2)) in chunk1.biomes.iter().zip(&chunk2.biomes).enumerate() {
            for y in 0..4u8 {
                for z in 0..4u8 {
                    for x in 0..4u8 {
                        assert_eq!(
                            b1.get(x, y, z),
                            b2.get(x, y, z),
                            "biome at ({x},{y},{z}) in section {i} of {:?}",
                            chunk1.pos
                        );
                    }
                }
            }
        }

        let hm1: HashSet<&String> = chunk1.heightmaps.keys().collect();
        let hm2: HashSet<&String> = chunk2.heightmaps.keys().collect();
        assert_eq!(hm1, hm2, "Heightmaps key set");
        for (name, h1) in &chunk1.heightmaps {
            let h2 = chunk2.heightmaps.get(name).unwrap();
            assert_eq!(
                h1.to_long_array(),
                h2.to_long_array(),
                "heightmap {name} in {:?}",
                chunk1.pos,
            );
        }

        assert_eq!(
            chunk1.block_entities, chunk2.block_entities,
            "block entities in {:?}",
            chunk1.pos
        );
        assert_eq!(
            chunk1.scheduled_block_ticks(),
            chunk2.scheduled_block_ticks(),
            "scheduled block ticks in {:?}",
            chunk1.pos
        );
        assert_eq!(
            chunk1.scheduled_fluid_ticks(),
            chunk2.scheduled_fluid_ticks(),
            "scheduled fluid ticks in {:?}",
            chunk1.pos
        );

        // Unknown extras stay byte-stable; vanilla's mandatory metadata is
        // normalized when absent from older Solaris chunks.
        assert_eq!(
            non_metadata_extras(&chunk1),
            non_metadata_extras(&chunk2),
            "extras count in {:?}",
            chunk1.pos,
        );
        assert_normalized_metadata(&chunk1, &chunk2);

        probed += 1;
    }
    assert!(probed > 0, "test region must have at least one chunk");
    eprintln!("round-tripped {probed} chunks");
}

#[test]
#[ignore = "requires vanilla-generated .analysis/test-world root extras"]
fn real_test_world_carries_dropped_root_fields_in_extras() {
    let region_path = workspace_path(".analysis/test-world/region/r.0.0.mca");
    let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
    assert!(
        region_path.is_file() && blocks_path.is_file(),
        "need both {} and {}",
        region_path.display(),
        blocks_path.display()
    );
    let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
    let registry = BlockRegistry::from_report(&report).unwrap();
    let payloads = region::read_region(&region_path).unwrap();

    let mut chunks_with_extras = 0usize;
    let mut seen_keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for payload in &payloads {
        let mut cur = Cursor::new(&payload.uncompressed_nbt[..]);
        let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
        let chunk = chunk_from_nbt(&root, &registry, crate::OVERWORLD_GEOMETRY).unwrap();
        let vanilla_extras = chunk
            .extras
            .iter()
            .filter(|(key, _)| key != "SolarisJournalLsn")
            .collect::<Vec<_>>();
        if !vanilla_extras.is_empty() {
            chunks_with_extras += 1;
            for (k, _) in vanilla_extras {
                seen_keys.insert(k.clone());
            }
        }
    }

    // Vanilla-generated oracle chunks have at least DataVersion +
    // InhabitedTime + LastUpdate.
    assert!(
        chunks_with_extras > 0,
        "vanilla oracle world carries no unmodelled root fields"
    );
    for required in &["DataVersion", "InhabitedTime", "LastUpdate"] {
        assert!(
            seen_keys.contains(*required),
            "extras key set should contain {required}; got {:?}",
            seen_keys,
        );
    }
}

/// M6.a: a chunk is decoded from the real test world, one block is
/// mutated via `set_block_and_update`, then `chunk_to_payload` +
/// `write_region` + `read_region` round-trips it back through a
/// fresh `.mca` file. The mutated cell must read back as the new
/// state, every other modelled field plus `extras` must survive.
#[test]
#[ignore = "requires local .analysis/test-world and 26.1.2 blocks report"]
fn round_trip_modified_chunk_through_disk() {
    let region_path = workspace_path(".analysis/test-world/region/r.0.0.mca");
    let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
    assert!(
        region_path.is_file() && blocks_path.is_file(),
        "need both {} and {}",
        region_path.display(),
        blocks_path.display()
    );
    let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
    let registry = BlockRegistry::from_report(&report).unwrap();
    let payloads = region::read_region(&region_path).unwrap();

    let original_payload = payloads
        .iter()
        .find(|p| p.local_x == 0 && p.local_z == 0)
        .expect("test world has chunk (0,0)");
    let mut cur = Cursor::new(&original_payload.uncompressed_nbt[..]);
    let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
    let mut chunk = chunk_from_nbt(&root, &registry, crate::OVERWORLD_GEOMETRY).unwrap();

    // Mutation: top block under spawn → a different solid state.
    let air = registry
        .block(&Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .unwrap();
    let stone = registry
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .map(|b| b.default)
        .unwrap();
    let dirt = registry
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .unwrap();
    let edit_y = top_non_air_y(&chunk, 0, 0, air).expect("origin column has terrain");
    let current = chunk.get_block(0, edit_y, 0).expect("edit cell present");
    let new_state = if current == stone { dirt } else { stone };
    let prev = chunk
        .set_block_and_update(0, edit_y, 0, new_state, air)
        .expect("y in range");
    assert_ne!(
        prev, new_state,
        "test world cell was already the replacement state — pick another"
    );
    assert!(chunk.dirty, "set_block_and_update marks chunk dirty");

    // Encode, write a fresh single-chunk region, read it back.
    let payload = chunk_to_payload(&chunk, &registry, 1_700_000_000).unwrap();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    region::write_region(tmp.path(), &[payload]).unwrap();
    let reread = region::read_region(tmp.path()).unwrap();
    let reread_payload = reread
        .iter()
        .find(|p| p.local_x == 0 && p.local_z == 0)
        .expect("written chunk must read back");
    let mut cur2 = Cursor::new(&reread_payload.uncompressed_nbt[..]);
    let (_, root2) = mc_nbt::read_named(&mut cur2).unwrap();
    let chunk2 = chunk_from_nbt(&root2, &registry, crate::OVERWORLD_GEOMETRY).unwrap();

    // Mutation survived.
    assert_eq!(chunk2.get_block(0, edit_y, 0), Some(new_state));
    // Same chunk position.
    assert_eq!(chunk2.pos, chunk.pos);
    // Heightmap is whatever set_block_and_update produced.
    let hms_before: std::collections::BTreeSet<&String> = chunk.heightmaps.keys().collect();
    let hms_after: std::collections::BTreeSet<&String> = chunk2.heightmaps.keys().collect();
    assert_eq!(hms_before, hms_after, "heightmap key set must survive");
    assert_eq!(non_metadata_extras(&chunk), non_metadata_extras(&chunk2));
    assert_normalized_metadata(&chunk, &chunk2);
    // Timestamp survived through the write/read pair.
    assert_eq!(reread_payload.timestamp, 1_700_000_000);
}

fn nibble_pattern() -> Vec<i8> {
    // Distinct values per byte so we can pin the byte order.
    // Cast through u8 so the literal 0..=255 wraps cleanly into
    // i8's -128..=127 range.
    (0..LIGHT_LAYER_BYTES)
        .map(|i| (i & 0xFF) as u8 as i8)
        .collect()
}

fn build_section_with_light(
    y: i8,
    block_light: Option<Vec<i8>>,
    sky_light: Option<Vec<i8>>,
) -> Tag {
    let mut s: Vec<(String, Tag)> = vec![
        ("Y".into(), Tag::Byte(y)),
        (
            "block_states".into(),
            Tag::Compound(vec![(
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![Tag::Compound(vec![(
                        "Name".into(),
                        Tag::String("minecraft:air".into()),
                    )])],
                }),
            )]),
        ),
        (
            "biomes".into(),
            Tag::Compound(vec![(
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::STRING,
                    elements: vec![Tag::String("minecraft:plains".into())],
                }),
            )]),
        ),
    ];
    if let Some(bl) = block_light {
        s.push(("BlockLight".into(), Tag::ByteArray(bl)));
    }
    if let Some(sl) = sky_light {
        s.push(("SkyLight".into(), Tag::ByteArray(sl)));
    }
    Tag::Compound(s)
}

fn build_chunk_root_with_geometry(
    y_pos: i32,
    section_count: usize,
    section_overrides: Vec<Tag>,
) -> Tag {
    let mut sections = (0..section_count)
        .map(|index| {
            build_section_with_light(i8::try_from(y_pos + index as i32).unwrap(), None, None)
        })
        .collect::<Vec<_>>();
    for section in section_overrides {
        let compound = expect_compound(&section, "section override").unwrap();
        let section_y = get_section_y(compound, "Y").unwrap();
        let index = usize::try_from(section_y - y_pos).unwrap();
        sections[index] = section;
    }
    Tag::Compound(vec![
        ("xPos".into(), Tag::Int(0)),
        ("zPos".into(), Tag::Int(0)),
        ("yPos".into(), Tag::Int(y_pos)),
        ("Status".into(), Tag::String("minecraft:full".into())),
        (
            "sections".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: sections,
            }),
        ),
    ])
}

fn build_chunk_root_with_sections(section_overrides: Vec<Tag>) -> Tag {
    build_chunk_root_with_geometry(
        MIN_SECTION_Y,
        (MAX_Y - MIN_Y) as usize / SECTION_DIM,
        section_overrides,
    )
}

fn tiny_registry() -> BlockRegistry {
    // Two blocks suffice for the M4.a tests: air and stone, each
    // with a single default state. Built inline so the test
    // doesn't depend on the full vanilla report being on disk.
    use std::collections::BTreeMap;
    let report = vec![
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 1,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
    ];
    BlockRegistry::from_report(&report).unwrap()
}

#[test]
fn strict_position_decode_rejects_same_and_cross_region_mismatch() {
    let registry = tiny_registry();
    for (expected, actual) in [
        (ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }),
        (ChunkPos { x: 31, z: -1 }, ChunkPos { x: 32, z: -1 }),
    ] {
        let mut root = build_chunk_root_with_sections(Vec::new());
        let Tag::Compound(entries) = &mut root else {
            unreachable!("chunk root fixture is a compound");
        };
        for (name, value) in entries {
            match name.as_str() {
                "xPos" => *value = Tag::Int(actual.x),
                "zPos" => *value = Tag::Int(actual.z),
                _ => {}
            }
        }

        assert!(matches!(
            chunk_from_nbt_with_items_at_position_at_tick(&root, expected, &registry, None, &ChestLootCatalog::new(), crate::OVERWORLD_GEOMETRY, 0),
            Err(ChunkNbtError::PositionMismatch {
                expected_x,
                expected_z,
                actual_x,
                actual_z,
            }) if expected_x == expected.x
                && expected_z == expected.z
                && actual_x == actual.x
                && actual_z == actual.z
        ));
    }
}

#[test]
fn strict_payload_decode_requires_exact_nbt_consumption() {
    let registry = tiny_registry();
    let expected = ChunkPos { x: -4, z: 7 };
    let chunk = Chunk::empty(
        expected,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let payload = chunk_to_payload(&chunk, &registry, 0).unwrap();

    let decoded = chunk_from_payload_with_items_at_position_at_tick(
        &payload.uncompressed_nbt,
        expected,
        &registry,
        None,
        &ChestLootCatalog::new(),
        crate::OVERWORLD_GEOMETRY,
        0,
    )
    .unwrap();
    assert_eq!(decoded.pos, expected);

    let mut trailing = payload.uncompressed_nbt;
    trailing.push(0x7F);
    assert!(matches!(
        chunk_from_payload_with_items_at_position_at_tick(
            &trailing,
            expected,
            &registry,
            None,
            &ChestLootCatalog::new(),
            crate::OVERWORLD_GEOMETRY,
            0
        ),
        Err(ChunkNbtError::TrailingNbtBytes { trailing: 1 })
    ));
}

#[test]
fn strict_payload_serializer_rejects_a_foreign_expected_key() {
    let registry = tiny_registry();
    let chunk = Chunk::empty(
        ChunkPos { x: 9, z: 10 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );

    assert!(matches!(
        chunk_to_payload_with_items_at_tick_for_position(
            &chunk,
            ChunkPos { x: 8, z: 10 },
            &registry,
            None,
            0,
            0,
        ),
        Err(ChunkNbtError::PositionMismatch {
            expected_x: 8,
            expected_z: 10,
            actual_x: 9,
            actual_z: 10,
        })
    ));
}

#[test]
fn offline_position_repair_canonicalizes_required_coordinate_fields() {
    let registry = tiny_registry();
    let expected = ChunkPos { x: 17, z: -33 };
    let mut root = build_chunk_root_with_sections(Vec::new());
    let Tag::Compound(entries) = &mut root else {
        unreachable!("chunk root fixture is a compound");
    };
    entries.retain(|(name, _)| name != "zPos");
    let x = entries
        .iter_mut()
        .find(|(name, _)| name == "xPos")
        .expect("fixture xPos");
    x.1 = Tag::String("wrong type".into());
    entries.push(("xPos".into(), Tag::Int(999)));

    assert!(repair_chunk_nbt_position(&mut root, expected).unwrap());
    assert!(!repair_chunk_nbt_position(&mut root, expected).unwrap());
    let Tag::Compound(entries) = &root else {
        unreachable!("repaired root remains a compound");
    };
    assert_eq!(entries.iter().filter(|(name, _)| name == "xPos").count(), 1);
    assert_eq!(entries.iter().filter(|(name, _)| name == "zPos").count(), 1);
    let decoded = chunk_from_nbt_with_items_at_position_at_tick(
        &root,
        expected,
        &registry,
        None,
        &ChestLootCatalog::new(),
        crate::OVERWORLD_GEOMETRY,
        0,
    )
    .unwrap();
    assert_eq!(decoded.pos, expected);
}

#[test]
fn encode_unknown_block_state_returns_error_without_panicking() {
    let registry = tiny_registry();
    let chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(7),
        Identifier::parse("minecraft:plains").unwrap(),
    );

    let encoded = std::panic::catch_unwind(|| chunk_to_nbt(&chunk, &registry));
    let error = encoded
        .expect("unknown block state encoding must not panic")
        .expect_err("unknown block state encoding must fail");

    assert!(matches!(error, ChunkNbtError::UnknownBlockStateId(7)));
}

fn scheduled_tick_tag(id: &str, x: i32, y: i32, z: i32, delay: i32, priority: i32) -> Tag {
    Tag::Compound(vec![
        ("i".into(), Tag::String(id.into())),
        ("x".into(), Tag::Int(x)),
        ("y".into(), Tag::Int(y)),
        ("z".into(), Tag::Int(z)),
        ("t".into(), Tag::Int(delay)),
        ("p".into(), Tag::Int(priority)),
    ])
}

fn add_root_field(root: &mut Tag, key: &str, value: Tag) {
    let Tag::Compound(entries) = root else {
        panic!("test root must be compound");
    };
    entries.push((key.into(), value));
}

fn root_fields(root: &Tag) -> &[(String, Tag)] {
    match root {
        Tag::Compound(entries) => entries,
        other => panic!("expected root compound, got {other:?}"),
    }
}

fn root_field<'a>(root: &'a Tag, name: &str) -> Option<&'a Tag> {
    root_fields(root)
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

#[test]
fn fresh_chunk_disk_payload_has_current_vanilla_metadata() {
    let registry = tiny_registry();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 3, z: -2 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.extras.push(("InhabitedTime".into(), Tag::Long(77)));

    let payload = chunk_to_payload_with_items_at_tick(&chunk, &registry, None, 1_700_000_000, 123)
        .expect("fresh chunk serializes");
    let file = tempfile::NamedTempFile::new().unwrap();
    region::write_region(file.path(), &[payload]).unwrap();
    let payload = region::read_region(file.path()).unwrap().remove(0);
    let mut cursor = Cursor::new(payload.uncompressed_nbt);
    let (_, root) = mc_nbt::read_named(&mut cursor).unwrap();

    assert_eq!(
        root_field(&root, "DataVersion"),
        Some(&Tag::Int(TARGET_DATA_VERSION))
    );
    assert_eq!(root_field(&root, "LastUpdate"), Some(&Tag::Long(123)));
    assert_eq!(root_field(&root, "InhabitedTime"), Some(&Tag::Long(77)));
    for key in VANILLA_METADATA_KEYS {
        assert_eq!(
            root_fields(&root)
                .iter()
                .filter(|(name, _)| name == key)
                .count(),
            1,
            "{key} must be emitted exactly once"
        );
    }

    let decoded = chunk_from_nbt(&root, &registry, crate::OVERWORLD_GEOMETRY).unwrap();
    assert_eq!(
        decoded
            .extras
            .iter()
            .find(|(name, _)| name == "InhabitedTime")
            .map(|(_, value)| value),
        Some(&Tag::Long(77))
    );
}

fn scheduled_tick_list(elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    })
}

#[test]
fn java_1165_dfu_missing_y_position_uses_authoritative_geometry() {
    for (y_pos, section_count, geometry) in [
        (-4, 24, crate::OVERWORLD_GEOMETRY),
        (0, 16, ChunkGeometry::new(0, 256).unwrap()),
    ] {
        let mut root = build_chunk_root_with_geometry(y_pos, section_count, Vec::new());
        let Tag::Compound(fields) = &mut root else {
            unreachable!();
        };
        // Vanilla 2841 updates only an existing yPos; real 1.16.5 chunks lack it.
        fields.retain(|(key, _)| key != "yPos");
        let registry = tiny_registry();
        let chunk = chunk_from_nbt(&root, &registry, geometry)
            .expect("vanilla DFU leaves missing yPos absent");
        assert_eq!(chunk.geometry(), geometry);
        let encoded = chunk_to_nbt(&chunk, &registry).unwrap();
        assert_eq!(
            get_int(expect_compound(&encoded, "root").unwrap(), "yPos").unwrap(),
            y_pos,
        );
    }
}

#[test]
fn java_1165_dfu_byte_y_position_reaches_modern_chunk_reader() {
    for (y_pos, section_count, geometry) in [
        (-4, 24, crate::OVERWORLD_GEOMETRY),
        (0, 16, ChunkGeometry::new(0, 256).unwrap()),
    ] {
        let mut root = build_chunk_root_with_geometry(y_pos, section_count, Vec::new());
        let Tag::Compound(fields) = &mut root else {
            unreachable!();
        };
        // When yPos exists, ChunkProtoTickListFix updates it to a byte.
        fields.iter_mut().find(|(key, _)| key == "yPos").unwrap().1 =
            Tag::Byte(i8::try_from(y_pos).unwrap());
        let registry = tiny_registry();
        let chunk = chunk_from_nbt(&root, &registry, geometry)
            .expect("vanilla's DFU byte yPos must reach the modern reader");
        assert_eq!(chunk.geometry(), geometry);
        let encoded = chunk_to_nbt(&chunk, &registry).unwrap();
        assert_eq!(
            get_int(expect_compound(&encoded, "root").unwrap(), "yPos").unwrap(),
            y_pos,
        );
    }
}

#[test]
fn java_1165_dfu_mixed_section_y_widths_reach_modern_chunk_reader() {
    let mut root = build_chunk_root_with_sections(Vec::new());
    let Tag::Compound(fields) = &mut root else {
        unreachable!();
    };
    let Tag::List(sections) = &mut fields
        .iter_mut()
        .find(|(key, _)| key == "sections")
        .unwrap()
        .1
    else {
        unreachable!();
    };
    for section in sections.elements.iter_mut().step_by(2) {
        let Tag::Compound(fields) = section else {
            unreachable!();
        };
        let value = &mut fields.iter_mut().find(|(key, _)| key == "Y").unwrap().1;
        let Tag::Byte(y) = value else {
            unreachable!();
        };
        // HeightAndBiomeFix creates int Y values; carried sections stay bytes.
        *value = Tag::Int(i32::from(*y));
    }
    let registry = tiny_registry();
    let chunk = chunk_from_nbt(&root, &registry, crate::OVERWORLD_GEOMETRY)
        .expect("vanilla's DFU mixes byte and int section Y values");
    let encoded = chunk_to_nbt(&chunk, &registry).unwrap();
    let sections = get_list(expect_compound(&encoded, "root").unwrap(), "sections").unwrap();
    for section in &sections.elements {
        let fields = expect_compound(section, "section").unwrap();
        assert!(matches!(get_field(fields, "Y"), Ok(Tag::Byte(_))));
    }
}

#[test]
fn round_trips_non_overworld_with_authoritative_geometry() {
    let root = build_chunk_root_with_geometry(0, 16, Vec::new());

    let registry = tiny_registry();
    let chunk = chunk_from_nbt(&root, &registry, ChunkGeometry::new(0, 256).unwrap())
        .expect("decode custom geometry");
    assert_eq!(chunk.geometry().min_y(), 0);
    assert_eq!(chunk.geometry().max_y(), 256);

    let encoded = chunk_to_nbt(&chunk, &registry).expect("encode custom geometry");
    let encoded_root = expect_compound(&encoded, "root").unwrap();
    assert_eq!(get_int(encoded_root, "yPos").unwrap(), 0);

    let decoded = chunk_from_nbt(&encoded, &registry, ChunkGeometry::new(0, 256).unwrap())
        .expect("decode round-trip");
    assert_eq!(decoded.geometry(), chunk.geometry());
}

#[test]
fn absent_or_sparse_sections_use_vanilla_empty_defaults() {
    let registry = tiny_registry();
    let air = registry.block(&id("minecraft:air")).unwrap().default;
    for geometry in [
        crate::OVERWORLD_GEOMETRY,
        ChunkGeometry::new(0, 256).unwrap(),
    ] {
        for mode in 0..4 {
            let min_section = geometry.min_y() / SECTION_DIM as i32;
            let mut root =
                build_chunk_root_with_geometry(min_section, usize::from(mode == 2), Vec::new());
            let Tag::Compound(fields) = &mut root else {
                unreachable!()
            };
            fields.retain(|(name, _)| name != "yPos" && (mode != 1 || name != "sections"));
            let status = if mode == 2 {
                "minecraft:full"
            } else {
                "minecraft:empty"
            };
            fields
                .iter_mut()
                .find(|(name, _)| name == "Status")
                .unwrap()
                .1 = Tag::String(status.into());
            fields.push(("HeightMap".into(), Tag::IntArray(vec![64; 256])));
            if mode == 3 {
                let (_, Tag::List(sections)) = fields
                    .iter_mut()
                    .find(|(name, _)| name == "sections")
                    .unwrap()
                else {
                    unreachable!()
                };
                sections.elements.push(Tag::Compound(vec![
                    ("Y".into(), Tag::Byte(min_section as i8)),
                    (
                        "SkyLight".into(),
                        Tag::ByteArray(vec![-1; LIGHT_LAYER_BYTES]),
                    ),
                ]));
            }
            let chunk = chunk_from_nbt(&root, &registry, geometry).unwrap();
            assert_eq!(chunk.status, status);
            assert_eq!(
                extra(&chunk, "HeightMap"),
                Some(&Tag::IntArray(vec![64; 256]))
            );
            for y in geometry.min_y()..geometry.max_y() {
                assert_eq!(chunk.get_block(0, y, 0), Some(air));
            }
            if mode == 3 {
                assert_eq!(chunk.section_lights[0].sky.as_ref().unwrap().byte(0), 255);
            }
            let encoded = chunk_to_nbt(&chunk, &registry).unwrap();
            let reopened = chunk_from_nbt(&encoded, &registry, geometry).unwrap();
            assert_eq!(reopened.status, status);
            assert_eq!(extra(&reopened, "HeightMap"), extra(&chunk, "HeightMap"));
        }
    }
}

#[test]
fn rejects_duplicate_section_y_in_known_geometry() {
    let mut root = build_chunk_root_with_geometry(0, 16, Vec::new());
    let Tag::Compound(fields) = &mut root else {
        unreachable!();
    };
    let Tag::List(sections) = &mut fields
        .iter_mut()
        .find(|(name, _)| name == "sections")
        .expect("synthetic chunk has sections")
        .1
    else {
        unreachable!();
    };
    let Tag::Compound(last_section) = sections.elements.last_mut().unwrap() else {
        unreachable!();
    };
    let y = last_section
        .iter_mut()
        .find(|(name, _)| name == "Y")
        .expect("synthetic section has Y");
    y.1 = Tag::Byte(14);

    assert!(matches!(
        chunk_from_nbt(&root, &tiny_registry(), ChunkGeometry::new(0, 256).unwrap()),
        Err(ChunkNbtError::InvalidSectionShape(14))
    ));
}

#[test]
fn decodes_scheduled_block_ticks_from_block_ticks() {
    let mut root = build_chunk_root_with_sections(Vec::new());
    add_root_field(
        &mut root,
        "block_ticks",
        scheduled_tick_list(vec![
            scheduled_tick_tag("minecraft:stone", 2, 64, 2, 5, 0),
            scheduled_tick_tag("minecraft:air", 1, 64, 1, 5, 0),
            scheduled_tick_tag("minecraft:stone", 3, 64, 3, 1, 1),
        ]),
    );

    let mut chunk =
        chunk_from_nbt(&root, &tiny_registry(), crate::OVERWORLD_GEOMETRY).expect("decode");
    let ticks = chunk.scheduled_block_ticks();
    assert_eq!(ticks.len(), 3);
    assert_eq!(ticks[0].trigger_tick, 1);
    assert_eq!(ticks[0].priority, 1);
    assert_eq!(ticks[0].block.as_str(), "minecraft:stone");

    let due = chunk.drain_due_block_ticks(5, usize::MAX);
    assert_eq!(
        due.iter().map(|tick| tick.pos.x).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

#[test]
fn decodes_scheduled_fluid_ticks_from_fluid_ticks() {
    let mut root = build_chunk_root_with_sections(Vec::new());
    add_root_field(
        &mut root,
        "fluid_ticks",
        scheduled_tick_list(vec![
            scheduled_tick_tag("minecraft:water", 2, 64, 2, 5, 0),
            scheduled_tick_tag("minecraft:lava", 1, 64, 1, 5, 0),
            scheduled_tick_tag("minecraft:water", 3, 64, 3, 1, 1),
        ]),
    );

    let mut chunk =
        chunk_from_nbt(&root, &tiny_registry(), crate::OVERWORLD_GEOMETRY).expect("decode");
    let ticks = chunk.scheduled_fluid_ticks();
    assert_eq!(ticks.len(), 3);
    assert_eq!(ticks[0].trigger_tick, 1);
    assert_eq!(ticks[0].priority, 1);
    assert_eq!(ticks[0].fluid.as_str(), "minecraft:water");

    let due = chunk.drain_due_fluid_ticks(5, usize::MAX);
    assert_eq!(
        due.iter().map(|tick| tick.pos.x).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

#[test]
fn block_and_fluid_ticks_are_modelled_not_extras() {
    let mut root = build_chunk_root_with_sections(Vec::new());
    add_root_field(
        &mut root,
        "block_ticks",
        scheduled_tick_list(vec![scheduled_tick_tag("minecraft:stone", 1, 64, 1, 2, 0)]),
    );
    add_root_field(
        &mut root,
        "fluid_ticks",
        scheduled_tick_list(vec![scheduled_tick_tag("minecraft:water", 1, 64, 1, 2, 0)]),
    );

    let chunk = chunk_from_nbt(&root, &tiny_registry(), crate::OVERWORLD_GEOMETRY).expect("decode");

    assert_eq!(chunk.scheduled_block_ticks().len(), 1);
    assert_eq!(chunk.scheduled_fluid_ticks().len(), 1);
    assert!(!chunk.extras.iter().any(|(key, _)| key == "block_ticks"));
    assert!(!chunk.extras.iter().any(|(key, _)| key == "fluid_ticks"));
}

#[test]
fn encodes_scheduled_block_ticks_without_duplicate_root_key() {
    let registry = tiny_registry();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    assert!(chunk.schedule_block_tick(ScheduledBlockTick::new(
        BlockPos { x: 1, y: 64, z: 1 },
        Identifier::parse("minecraft:stone").unwrap(),
        7,
        -1,
    )));
    chunk.extras.push((
        "block_ticks".into(),
        scheduled_tick_list(vec![scheduled_tick_tag("minecraft:air", 2, 64, 2, 1, 0)]),
    ));

    let root = chunk_to_nbt(&chunk, &registry).expect("encode");
    let block_tick_fields: Vec<&Tag> = root_fields(&root)
        .iter()
        .filter_map(|(key, value)| (key == "block_ticks").then_some(value))
        .collect();

    assert_eq!(block_tick_fields.len(), 1);
    let Tag::List(list) = block_tick_fields[0] else {
        panic!("block_ticks must encode as list");
    };
    assert_eq!(list.element_type, tag_type::COMPOUND);
    assert_eq!(list.elements.len(), 1);
}

#[test]
fn encodes_scheduled_fluid_ticks_without_duplicate_root_key() {
    let registry = tiny_registry();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    assert!(chunk.schedule_fluid_tick(ScheduledFluidTick::new(
        BlockPos { x: 1, y: 64, z: 1 },
        Identifier::parse("minecraft:water").unwrap(),
        7,
        -1,
    )));
    chunk.extras.push((
        "fluid_ticks".into(),
        scheduled_tick_list(vec![scheduled_tick_tag("minecraft:lava", 2, 64, 2, 1, 0)]),
    ));

    let root = chunk_to_nbt(&chunk, &registry).expect("encode");
    let fluid_tick_fields: Vec<&Tag> = root_fields(&root)
        .iter()
        .filter_map(|(key, value)| (key == "fluid_ticks").then_some(value))
        .collect();

    assert_eq!(fluid_tick_fields.len(), 1);
    let Tag::List(list) = fluid_tick_fields[0] else {
        panic!("fluid_ticks must encode as list");
    };
    assert_eq!(list.element_type, tag_type::COMPOUND);
    assert_eq!(list.elements.len(), 1);
}

#[test]
fn scheduled_ticks_round_trip_through_disk() {
    let registry = tiny_registry();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    assert!(chunk.schedule_block_tick(ScheduledBlockTick::new(
        BlockPos { x: 1, y: 64, z: 1 },
        Identifier::parse("minecraft:stone").unwrap(),
        12,
        0,
    )));
    assert!(chunk.schedule_fluid_tick(ScheduledFluidTick::new(
        BlockPos { x: 2, y: 64, z: 2 },
        Identifier::parse("minecraft:water").unwrap(),
        8,
        -1,
    )));

    let payload = chunk_to_payload(&chunk, &registry, 1_700_000_001).unwrap();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    region::write_region(tmp.path(), &[payload]).unwrap();
    let reread = region::read_region(tmp.path()).unwrap();
    let mut cur = Cursor::new(&reread[0].uncompressed_nbt[..]);
    let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
    let decoded = chunk_from_nbt(&root, &registry, crate::OVERWORLD_GEOMETRY).unwrap();

    assert_eq!(decoded.scheduled_block_ticks().len(), 1);
    assert_eq!(decoded.scheduled_block_ticks()[0].trigger_tick, 12);
    assert_eq!(decoded.scheduled_block_ticks()[0].priority, 0);
    assert_eq!(decoded.scheduled_fluid_ticks().len(), 1);
    assert_eq!(decoded.scheduled_fluid_ticks()[0].trigger_tick, 8);
    assert_eq!(decoded.scheduled_fluid_ticks()[0].priority, -1);
    assert!(!decoded.extras.iter().any(|(key, _)| key == "fluid_ticks"));
}

#[test]
fn overdue_scheduled_block_tick_import_is_due_on_first_processed_tick() {
    let mut root = build_chunk_root_with_sections(Vec::new());
    add_root_field(
        &mut root,
        "block_ticks",
        scheduled_tick_list(vec![
            scheduled_tick_tag("minecraft:stone", 1, 64, 1, -9, -1),
            scheduled_tick_tag("minecraft:stone", 2, 64, 1, -9, -1),
            scheduled_tick_tag("minecraft:stone", 3, 64, 1, 9, 0),
        ]),
    );
    let mut chunk = chunk_from_nbt(&root, &tiny_registry(), crate::OVERWORLD_GEOMETRY).unwrap();
    let due = chunk.drain_due_block_ticks(1, usize::MAX);
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].trigger_tick, 0);
    assert_eq!(due[0].priority, -1);
    assert_eq!(due[0].sequence(), 0);
    assert_eq!(due[1].sequence(), 1);
    assert_eq!(chunk.scheduled_block_ticks()[0].trigger_tick, 9);
}

#[test]
fn overdue_scheduled_fluid_tick_import_is_due_on_first_processed_tick() {
    let mut root = build_chunk_root_with_sections(Vec::new());
    add_root_field(
        &mut root,
        "fluid_ticks",
        scheduled_tick_list(vec![
            scheduled_tick_tag("minecraft:water", 1, 64, 1, -9, -1),
            scheduled_tick_tag("minecraft:water", 2, 64, 1, -9, -1),
            scheduled_tick_tag("minecraft:water", 3, 64, 1, 9, 0),
        ]),
    );
    let mut chunk = chunk_from_nbt(&root, &tiny_registry(), crate::OVERWORLD_GEOMETRY).unwrap();
    let due = chunk.drain_due_fluid_ticks(1, usize::MAX);
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].trigger_tick, 0);
    assert_eq!(due[0].priority, -1);
    assert_eq!(due[0].sequence(), 0);
    assert_eq!(due[1].sequence(), 1);
    assert_eq!(chunk.scheduled_fluid_ticks()[0].trigger_tick, 9);
}

#[test]
fn scheduled_tick_signed_delay_extremes_saturate_without_losing_order_metadata() {
    for base_tick in [0, 1, 100, u64::MAX - 1, u64::MAX] {
        for delay in [i32::MIN, -9, 0, i32::MAX] {
            let expected = if delay < 0 {
                base_tick.saturating_sub(u64::from(delay.unsigned_abs()))
            } else {
                base_tick.saturating_add(delay as u64)
            };
            for (key, name) in [
                ("block_ticks[]", "minecraft:stone"),
                ("fluid_ticks[]", "minecraft:water"),
            ] {
                let tag = scheduled_tick_tag(name, 1, 64, 1, delay, -2);
                let tick = decode_scheduled_tick_entry(&tag, key, base_tick, 42).unwrap();
                assert_eq!(
                    tick.trigger_tick, expected,
                    "{key}: base={base_tick}, delay={delay}"
                );
                assert_eq!(tick.priority, -2);
                assert_eq!(tick.sequence, 42);
            }
        }
    }
}

#[test]
fn overdue_tick_deadlines_and_order_survive_signed_anvil_roundtrip() {
    let registry = tiny_registry();
    let position = ChunkPos { x: 0, z: 0 };
    let base_tick = 100;
    let mut root = build_chunk_root_with_sections(Vec::new());
    for (key, name) in [
        ("block_ticks", "minecraft:stone"),
        ("fluid_ticks", "minecraft:water"),
    ] {
        add_root_field(
            &mut root,
            key,
            scheduled_tick_list(vec![
                scheduled_tick_tag(name, 1, 64, 1, -9, 2),
                scheduled_tick_tag(name, 2, 64, 1, -3, -2),
            ]),
        );
    }
    let imported = chunk_from_nbt_with_items_at_position_at_tick(
        &root,
        position,
        &registry,
        None,
        &ChestLootCatalog::new(),
        crate::OVERWORLD_GEOMETRY,
        base_tick,
    )
    .unwrap();
    assert_eq!(
        imported.scheduled_block_ticks()[0].trigger_tick,
        base_tick - 9
    );
    assert_eq!(
        imported.scheduled_block_ticks()[1].trigger_tick,
        base_tick - 3
    );
    assert_eq!(
        imported.scheduled_fluid_ticks()[0].trigger_tick,
        base_tick - 9
    );
    assert_eq!(
        imported.scheduled_fluid_ticks()[1].trigger_tick,
        base_tick - 3
    );
    let exported = chunk_to_nbt_with_items_at_tick(&imported, &registry, None, base_tick).unwrap();
    let mut payload = Vec::new();
    mc_nbt::write_named(&mut payload, "", &exported).unwrap();
    let mut restored = chunk_from_payload_with_items_at_position_at_tick(
        &payload,
        position,
        &registry,
        None,
        &ChestLootCatalog::new(),
        crate::OVERWORLD_GEOMETRY,
        base_tick,
    )
    .unwrap();
    assert_eq!(
        restored.scheduled_block_ticks(),
        imported.scheduled_block_ticks()
    );
    assert_eq!(
        restored.scheduled_fluid_ticks(),
        imported.scheduled_fluid_ticks()
    );
    let blocks = restored.drain_due_block_ticks(base_tick + 1, usize::MAX);
    let fluids = restored.drain_due_fluid_ticks(base_tick + 1, usize::MAX);
    assert_eq!(
        blocks.iter().map(|tick| tick.pos.x).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        fluids.iter().map(|tick| tick.pos.x).collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn signed_anvil_tick_export_checks_both_i32_bounds_without_u64_cast_wrap() {
    assert_eq!(anvil_tick_delay(i32::MAX as u64, 0).unwrap(), i32::MAX);
    assert_eq!(anvil_tick_delay(0, 1_u64 << 31).unwrap(), i32::MIN);
    assert_eq!(anvil_tick_delay(u64::MAX - 9, u64::MAX).unwrap(), -9);
    for (deadline, current_tick) in [
        (i32::MAX as u64 + 1, 0),
        (0, (1_u64 << 31) + 1),
        (u64::MAX, 0),
        (0, u64::MAX),
    ] {
        let expected = i128::from(deadline) - i128::from(current_tick);
        let pos = BlockPos { x: 1, y: 64, z: 1 };
        let block = ScheduledBlockTick::new(
            pos,
            Identifier::parse("minecraft:stone").unwrap(),
            deadline,
            0,
        );
        let fluid = ScheduledFluidTick::new(
            pos,
            Identifier::parse("minecraft:water").unwrap(),
            deadline,
            0,
        );
        for result in [
            encode_scheduled_block_ticks(&[block], current_tick),
            encode_scheduled_fluid_ticks(&[fluid], current_tick),
        ] {
            assert!(
                matches!(result, Err(ChunkNbtError::TickDelayOutOfRange(delay)) if delay == expected)
            );
        }
    }
}

#[test]
fn decodes_per_section_light_arrays_when_present() {
    let pattern = nibble_pattern();
    let root = build_chunk_root_with_sections(vec![
        build_section_with_light(-4, Some(pattern.clone()), Some(pattern.clone())),
        build_section_with_light(0, None, Some(pattern.clone())),
    ]);
    let chunk = chunk_from_nbt(&root, &tiny_registry(), crate::OVERWORLD_GEOMETRY).expect("decode");

    let expected: Vec<u8> = pattern.iter().map(|&b| b as u8).collect();

    // Section Y=-4 → idx 0: both layers present.
    assert_eq!(
        chunk.section_lights[0]
            .block
            .as_ref()
            .map(|layer| layer.to_vec()),
        Some(expected.clone())
    );
    assert_eq!(
        chunk.section_lights[0]
            .sky
            .as_ref()
            .map(|layer| layer.to_vec()),
        Some(expected.clone())
    );

    // Section Y=0 → idx 4: only sky present.
    assert_eq!(chunk.section_lights[4].block, None);
    assert_eq!(
        chunk.section_lights[4]
            .sky
            .as_ref()
            .map(|layer| layer.to_vec()),
        Some(expected)
    );

    // Untouched sections keep the default (no light).
    assert_eq!(chunk.section_lights[1], SectionLight::default());
    assert_eq!(chunk.section_lights[23], SectionLight::default());

    // Layer is exactly LIGHT_LAYER_BYTES bytes; sanity-check the
    // i8 → u8 reinterpretation didn't shift anything.
    let sky = chunk.section_lights[0].sky.as_ref().unwrap();
    assert_eq!(sky.byte(0x00), 0x00);
    assert_eq!(sky.byte(0x7F), 0x7F);
    assert_eq!(sky.byte(0x80), 0x80);
    assert_eq!(sky.byte(0xFF), 0xFF);
}

#[test]
fn encodes_per_section_light_arrays_when_present() {
    let registry = tiny_registry();
    let pattern = nibble_pattern();
    let expected: Vec<u8> = pattern.iter().map(|&byte| byte as u8).collect();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.section_lights[0].block = Some(crate::chunk::LightSection::from_bytes(
        expected.clone().try_into().unwrap(),
    ));
    chunk.section_lights[0].sky = Some(crate::chunk::LightSection::from_bytes(
        expected.clone().try_into().unwrap(),
    ));
    chunk.section_lights[4].sky = Some(crate::chunk::LightSection::from_bytes(
        expected.clone().try_into().unwrap(),
    ));

    let root = chunk_to_nbt(&chunk, &registry).expect("encode");
    let sections = get_optional_list(expect_compound(&root, "root").unwrap(), "sections")
        .unwrap()
        .unwrap();

    let first = expect_compound(&sections.elements[0], "sections[0]").unwrap();
    assert_eq!(
        first
            .iter()
            .find(|(key, _)| key == "BlockLight")
            .map(|(_, tag)| tag),
        Some(&Tag::ByteArray(pattern.clone()))
    );
    assert_eq!(
        first
            .iter()
            .find(|(key, _)| key == "SkyLight")
            .map(|(_, tag)| tag),
        Some(&Tag::ByteArray(pattern.clone()))
    );
    let second = expect_compound(&sections.elements[1], "sections[1]").unwrap();
    assert!(
        second
            .iter()
            .all(|(key, _)| key != "BlockLight" && key != "SkyLight")
    );
    let y_zero = expect_compound(&sections.elements[4], "sections[4]").unwrap();
    assert!(y_zero.iter().all(|(key, _)| key != "BlockLight"));
    assert_eq!(
        y_zero
            .iter()
            .find(|(key, _)| key == "SkyLight")
            .map(|(_, tag)| tag),
        Some(&Tag::ByteArray(pattern))
    );

    let decoded =
        chunk_from_nbt(&root, &registry, crate::OVERWORLD_GEOMETRY).expect("decode encoded light");
    assert_eq!(
        decoded.section_lights[0]
            .block
            .as_ref()
            .map(|layer| layer.to_vec()),
        Some(expected.clone())
    );
    assert_eq!(
        decoded.section_lights[0]
            .sky
            .as_ref()
            .map(|layer| layer.to_vec()),
        Some(expected.clone())
    );
    assert_eq!(decoded.section_lights[4].block, None);
    assert_eq!(
        decoded.section_lights[4]
            .sky
            .as_ref()
            .map(|layer| layer.to_vec()),
        Some(expected)
    );
}

#[test]
fn rejects_wrong_length_light_array() {
    let truncated: Vec<i8> = vec![0; LIGHT_LAYER_BYTES - 1];
    let root =
        build_chunk_root_with_sections(vec![build_section_with_light(0, Some(truncated), None)]);
    match chunk_from_nbt(&root, &tiny_registry(), crate::OVERWORLD_GEOMETRY) {
        Err(ChunkNbtError::LightLengthMismatch {
            field,
            expected,
            got,
        }) => {
            assert_eq!(field, "BlockLight");
            assert_eq!(expected, LIGHT_LAYER_BYTES);
            assert_eq!(got, LIGHT_LAYER_BYTES - 1);
        }
        other => panic!("expected LightLengthMismatch, got {:?}", other),
    }
}

#[test]
fn rejects_wrong_type_light_field() {
    let root = build_chunk_root_with_sections(vec![Tag::Compound(vec![
        ("Y".into(), Tag::Byte(0)),
        (
            "block_states".into(),
            Tag::Compound(vec![(
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![Tag::Compound(vec![(
                        "Name".into(),
                        Tag::String("minecraft:air".into()),
                    )])],
                }),
            )]),
        ),
        (
            "biomes".into(),
            Tag::Compound(vec![(
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::STRING,
                    elements: vec![Tag::String("minecraft:plains".into())],
                }),
            )]),
        ),
        ("SkyLight".into(), Tag::Int(0)),
    ])]);
    match chunk_from_nbt(&root, &tiny_registry(), crate::OVERWORLD_GEOMETRY) {
        Err(ChunkNbtError::WrongType { field, .. }) => assert_eq!(field, "SkyLight"),
        other => panic!("expected WrongType for SkyLight, got {:?}", other),
    }
}

#[test]
#[ignore = "requires vanilla-generated .analysis/test-world baked skylight"]
fn real_test_world_carries_some_baked_skylight() {
    let region_path = workspace_path(".analysis/test-world/region/r.0.0.mca");
    let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
    assert!(
        region_path.is_file() && blocks_path.is_file(),
        "need both {} and {}",
        region_path.display(),
        blocks_path.display()
    );
    let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
    let registry = BlockRegistry::from_report(&report).unwrap();
    let payloads = region::read_region(&region_path).unwrap();

    let mut chunks_with_sky = 0usize;
    let mut sections_with_sky = 0usize;
    for payload in &payloads {
        let mut cur = Cursor::new(&payload.uncompressed_nbt[..]);
        let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
        let chunk = chunk_from_nbt(&root, &registry, crate::OVERWORLD_GEOMETRY).expect("decode");

        let chunk_has_sky = chunk.section_lights.iter().any(|sl| sl.sky.is_some());
        if chunk_has_sky {
            chunks_with_sky += 1;
        }
        for sl in &chunk.section_lights {
            if sl.sky.is_some() {
                sections_with_sky += 1;
            }
        }
    }

    // Vanilla-generated oracle chunks carry baked SkyLight.
    assert!(
        chunks_with_sky > 0,
        "vanilla oracle world carries no baked SkyLight"
    );
    assert!(
        sections_with_sky >= 1,
        "expected ≥1 section with baked SkyLight, got {sections_with_sky}",
    );
}

#[test]
fn container_nbt_rejects_malformed_native_stack_components() {
    let items = mc_data::items::solaris_required_items();
    for (component, value) in [
        (
            mc_data::item_stack::CUSTOM_DATA_COMPONENT,
            Tag::String("not a compound".into()),
        ),
        (mc_data::item_stack::MAX_DAMAGE_COMPONENT, Tag::Int(-1)),
        (
            mc_data::item_stack::CUSTOM_DATA_COMPONENT,
            Tag::Compound(vec![(
                "bytes".into(),
                Tag::ByteArray(vec![0; mc_data::item_stack::MAX_CUSTOM_DATA_BYTES]),
            )]),
        ),
    ] {
        let fields = vec![
            ("id".into(), Tag::String("minecraft:apple".into())),
            ("count".into(), Tag::Int(1)),
            (
                "components".into(),
                Tag::Compound(vec![(component.into(), value)]),
            ),
        ];
        assert!(decode_container_stack(&fields, &items).is_err());
    }
}

#[test]
fn authoritative_geometry_preserves_only_adjacent_light_only_sections() {
    fn append(root: &mut Tag, section: Tag) {
        let Tag::Compound(fields) = root else {
            unreachable!()
        };
        let Tag::List(sections) = &mut fields
            .iter_mut()
            .find(|(name, _)| name == "sections")
            .unwrap()
            .1
        else {
            unreachable!()
        };
        sections.elements.push(section);
    }
    fn light_only(y: i8) -> Tag {
        Tag::Compound(vec![
            ("Y".into(), Tag::Byte(y)),
            (
                "BlockLight".into(),
                Tag::ByteArray(vec![0; LIGHT_LAYER_BYTES]),
            ),
            (
                "SkyLight".into(),
                Tag::ByteArray(vec![-1; LIGHT_LAYER_BYTES]),
            ),
        ])
    }
    let registry = tiny_registry();
    for geometry in [
        crate::OVERWORLD_GEOMETRY,
        ChunkGeometry::new(0, 256).unwrap(),
    ] {
        let min_section = geometry.min_y() / SECTION_DIM as i32;
        let upper_boundary = geometry.max_y() / SECTION_DIM as i32;
        let fixture =
            || build_chunk_root_with_geometry(min_section, geometry.section_count(), Vec::new());
        let mut root = fixture();
        append(&mut root, light_only((min_section - 1) as i8));
        append(&mut root, light_only(upper_boundary as i8));
        let chunk = chunk_from_nbt(&root, &registry, geometry).unwrap();
        assert_eq!(chunk.geometry(), geometry);
        for light in &chunk.boundary_section_lights {
            assert!(light.block.as_ref().unwrap().is_zero());
            assert_eq!(light.sky.as_ref().unwrap().byte(0), 255);
        }
        let encoded = chunk_to_nbt(&chunk, &registry).unwrap();
        let decoded = chunk_from_nbt(&encoded, &registry, geometry).unwrap();
        assert_eq!(
            decoded.boundary_section_lights,
            chunk.boundary_section_lights
        );
        for y in [min_section - 2, upper_boundary + 1] {
            let mut invalid = fixture();
            append(&mut invalid, light_only(y as i8));
            assert!(
                matches!(chunk_from_nbt(&invalid, &registry, geometry), Err(ChunkNbtError::SectionOutOfRange(actual)) if actual == y)
            );
        }
        let mut block_bearing = fixture();
        append(
            &mut block_bearing,
            build_section_with_light((min_section - 1) as i8, None, None),
        );
        assert!(matches!(
            chunk_from_nbt(&block_bearing, &registry, geometry),
            Err(ChunkNbtError::SectionOutOfRange(_))
        ));
        let mut duplicate = root.clone();
        append(&mut duplicate, light_only(upper_boundary as i8));
        assert!(matches!(
            chunk_from_nbt(&duplicate, &registry, geometry),
            Err(ChunkNbtError::InvalidSectionShape(_))
        ));
    }
}
