use super::super::dictionary;
use super::*;
use crate::{BlockPos, BlockStateId, ChestBlockEntity, ChunkPos, ScheduledBlockTick};
use mc_data::blocks::solaris_required_blocks_report;
use mc_data::items::solaris_required_items;
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

const OVERLAYS: TableDefinition<u32, &[u8]> = TableDefinition::new("test_overlays");

fn state(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}

fn database(path: &std::path::Path) -> Database {
    let database = Database::create(path).unwrap();
    let transaction = database.begin_write().unwrap();
    dictionary::initialize(&transaction).unwrap();
    transaction.open_table(OVERLAYS).unwrap();
    transaction.commit().unwrap();
    database
}

fn save(
    database: &Database,
    base: &ChunkSection,
    effective: &ChunkSection,
    blocks: &BlockRegistry,
) -> Vec<u8> {
    let transaction = database.begin_write().unwrap();
    let bytes = {
        let mut dictionary = Writer::open(&transaction).unwrap();
        let mut overlays = transaction.open_table(OVERLAYS).unwrap();
        let previous = overlays.get(0).unwrap();
        let bytes = overlay(
            base,
            effective,
            blocks,
            &mut dictionary,
            previous.as_ref().map(|value| value.value()),
        )
        .unwrap()
        .unwrap();
        drop(previous);
        overlays.insert(0, bytes.as_slice()).unwrap();
        bytes
    };
    transaction.commit().unwrap();
    bytes
}

fn load(database: &Database, base: &ChunkSection, blocks: &BlockRegistry) -> ChunkSection {
    let transaction = database.begin_read().unwrap();
    let dictionary = dictionary::Reader::open(&transaction).unwrap();
    let overlays = transaction.open_table(OVERLAYS).unwrap();
    let record = overlays.get(0).unwrap().unwrap();
    let Tag::Compound(fields) = compose_section(base, record.value(), blocks, &dictionary).unwrap()
    else {
        panic!("section compound");
    };
    crate::anvil::chunk_nbt::decode_block_section(&fields, blocks, state(blocks, "minecraft:air"))
        .unwrap()
}

fn equal(actual: &ChunkSection, expected: &ChunkSection) {
    for index in 0..SECTION_VOLUME {
        let (x, y, z) = coordinates(index);
        assert_eq!(actual.get(x, y, z), expected.get(x, y, z), "cell {index}");
    }
}

fn varied_base(blocks: &BlockRegistry) -> ChunkSection {
    let air = state(blocks, "minecraft:air");
    let choices = [
        air,
        state(blocks, "minecraft:stone"),
        state(blocks, "minecraft:water"),
    ];
    let mut section = ChunkSection::filled(air, air);
    let mut random = 0x12345678u32;
    for index in 0..SECTION_VOLUME {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        let (x, y, z) = coordinates(index);
        section.set(x, y, z, choices[random as usize % choices.len()]);
    }
    section
}

#[test]
fn sparse_dense_sparse_transitions_reopen_without_changing_effective_cells() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let air = state(&blocks, "minecraft:air");
    let stone = state(&blocks, "minecraft:stone");
    let base = varied_base(&blocks);
    let mut sparse = base.clone();
    sparse.set(
        1,
        1,
        1,
        if base.get(1, 1, 1) == stone {
            air
        } else {
            stone
        },
    );
    let dense = ChunkSection::filled(stone, air);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.redb");
    let mut database = database(&path);
    for (effective, expected) in [
        (&sparse, Representation::Sparse),
        (&dense, Representation::Dense),
        (&sparse, Representation::Sparse),
    ] {
        let bytes = save(&database, &base, effective, &blocks);
        assert_eq!(representation(&bytes).unwrap(), expected);
        drop(database);
        database = Database::open(&path).unwrap();
        equal(&load(&database, &base, &blocks), effective);
    }
    let transaction = database.begin_write().unwrap();
    let mut dictionary = Writer::open(&transaction).unwrap();
    assert!(
        overlay(&base, &base, &blocks, &mut dictionary, None)
            .unwrap()
            .is_none()
    );
}

#[test]
fn all_air_replacement_survives_reopen_and_runtime_registry_reordering() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let air = state(&blocks, "minecraft:air");
    let base = ChunkSection::filled(state(&blocks, "minecraft:stone"), air);
    let effective = ChunkSection::filled(air, air);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.redb");
    let database = database(&path);
    let bytes = save(&database, &base, &effective, &blocks);
    assert_eq!(representation(&bytes).unwrap(), Representation::Dense);
    drop(database);
    let mut report = solaris_required_blocks_report();
    let total: u32 = report.iter().map(|block| block.states.len() as u32).sum();
    for block in &mut report {
        for state in &mut block.states {
            state.id = total - 1 - state.id;
        }
    }
    let reordered = BlockRegistry::from_report(&report).unwrap();
    let reordered_air = state(&reordered, "minecraft:air");
    let reordered_base = ChunkSection::filled(state(&reordered, "minecraft:stone"), reordered_air);
    let database = Database::open(&path).unwrap();
    equal(
        &load(&database, &reordered_base, &reordered),
        &ChunkSection::filled(reordered_air, reordered_air),
    );
}

#[test]
fn aborted_overlay_transaction_leaves_neither_dictionary_additions_nor_references() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.redb");
    let database = database(&path);
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let air = state(&blocks, "minecraft:air");
    let base = ChunkSection::filled(air, air);
    let mut effective = base.clone();
    effective.set(2, 3, 4, state(&blocks, "minecraft:stone"));
    let transaction = database.begin_write().unwrap();
    {
        let mut dictionary = Writer::open(&transaction).unwrap();
        let bytes = overlay(&base, &effective, &blocks, &mut dictionary, None)
            .unwrap()
            .unwrap();
        transaction
            .open_table(OVERLAYS)
            .unwrap()
            .insert(0, bytes.as_slice())
            .unwrap();
    }
    transaction.abort().unwrap();
    drop(database);
    let database = Database::open(&path).unwrap();
    let transaction = database.begin_read().unwrap();
    assert_eq!(
        transaction
            .open_table(dictionary::STATES)
            .unwrap()
            .len()
            .unwrap(),
        0
    );
    assert_eq!(
        transaction
            .open_table(dictionary::IDS)
            .unwrap()
            .len()
            .unwrap(),
        0
    );
    assert!(
        transaction
            .open_table(OVERLAYS)
            .unwrap()
            .get(0)
            .unwrap()
            .is_none()
    );
    drop(transaction);
    save(&database, &base, &effective, &blocks);
    equal(&load(&database, &base, &blocks), &effective);
}

#[test]
fn overlay_corruption_fails_closed_instead_of_returning_baseline() {
    let temp = tempfile::tempdir().unwrap();
    let database = database(&temp.path().join("state.redb"));
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let air = state(&blocks, "minecraft:air");
    let base = ChunkSection::filled(air, air);
    let mut effective = base.clone();
    effective.set(1, 2, 3, state(&blocks, "minecraft:stone"));
    let bytes = save(&database, &base, &effective, &blocks);
    let transaction = database.begin_read().unwrap();
    let dictionary = dictionary::Reader::open(&transaction).unwrap();
    let mut checksum = bytes.clone();
    *checksum.last_mut().unwrap() ^= 1;
    let mut trailing = bytes.clone();
    trailing.push(0);
    let mut oversized = bytes.clone();
    oversized[1..5].copy_from_slice(&((MAX_OVERLAY_BODY + 1) as u32).to_be_bytes());
    let mut invalid_mode = bytes.clone();
    invalid_mode[0] = 255;
    for corrupt in [
        checksum,
        trailing,
        oversized,
        invalid_mode,
        bytes[..bytes.len() - 1].to_vec(),
    ] {
        assert!(compose_section(&base, &corrupt, &blocks, &dictionary).is_err());
    }
    let mut unknown = Vec::new();
    append_palette(&mut unknown, &[u32::MAX]);
    assert!(
        compose_section(
            &base,
            &compress(DENSE, &unknown).unwrap(),
            &blocks,
            &dictionary
        )
        .is_err()
    );
    let mut unsorted = vec![0, 2, 0, 7, 0, 0, 0, 7, 0, 0];
    append_palette(&mut unsorted, &[0]);
    assert!(
        compose_section(
            &base,
            &compress(SPARSE, &unsorted).unwrap(),
            &blocks,
            &dictionary
        )
        .is_err()
    );
    let mut bounds = vec![0, 1, 0x10, 0, 0, 0];
    append_palette(&mut bounds, &[0]);
    assert!(
        compose_section(
            &base,
            &compress(SPARSE, &bounds).unwrap(),
            &blocks,
            &dictionary
        )
        .is_err()
    );
    let mut packed = Vec::new();
    append_palette(&mut packed, &[0, 1]);
    packed.resize(packed.len() + 512, 0);
    *packed.last_mut().unwrap() = 0x80;
    // Two entries use every bit in the last word, so exercise malformed length.
    packed.push(0);
    assert!(
        compose_section(
            &base,
            &compress(DENSE, &packed).unwrap(),
            &blocks,
            &dictionary
        )
        .is_err()
    );
}

#[test]
fn representation_changes_preserve_block_entities_and_scheduled_metadata() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let air = state(&blocks, "minecraft:air");
    let stone = state(&blocks, "minecraft:stone");
    let chest = state(&blocks, "minecraft:chest");
    let mut base = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        air,
        Identifier::parse("minecraft:plains").unwrap(),
    );
    base.sections[0] = varied_base(&blocks);
    let pos = BlockPos {
        x: 1,
        y: base.geometry().min_y() + 1,
        z: 1,
    };
    let mut sparse = base.clone();
    sparse.set_block(1, pos.y, 1, chest).unwrap();
    sparse.chests.insert(pos, ChestBlockEntity::default());
    sparse.schedule_block_tick(ScheduledBlockTick::new(
        pos,
        Identifier::parse("minecraft:chest").unwrap(),
        37,
        -1,
    ));
    let mut dense = sparse.clone();
    dense.sections[0] = ChunkSection::filled(stone, air);
    dense.sections[0].set(1, 1, 1, chest);
    let temp = tempfile::tempdir().unwrap();
    let database = database(&temp.path().join("state.redb"));
    for (effective, expected) in [
        (&sparse, Representation::Sparse),
        (&dense, Representation::Dense),
        (&sparse, Representation::Sparse),
    ] {
        let bytes = save(
            &database,
            &base.sections[0],
            &effective.sections[0],
            &blocks,
        );
        assert_eq!(representation(&bytes).unwrap(), expected);
        let transaction = database.begin_read().unwrap();
        let dictionary = dictionary::Reader::open(&transaction).unwrap();
        let mut root = decode(&metadata(effective, &blocks, &items).unwrap()).unwrap();
        let sections = sections_mut(&mut root).unwrap();
        for (index, section) in effective.sections.iter().enumerate() {
            let encoded = if index == 0 {
                compose_section(&base.sections[0], &bytes, &blocks, &dictionary).unwrap()
            } else {
                encode_block_section(section, &blocks).unwrap()
            };
            compound_mut(&mut sections[index])
                .unwrap()
                .push(("block_states".into(), encoded));
        }
        let mut restored = crate::anvil::chunk_nbt::chunk_from_nbt_with_items_at_position_at_tick(
            &root,
            base.pos,
            &blocks,
            Some(&items),
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
            crate::OVERWORLD_GEOMETRY,
            0,
        )
        .unwrap();
        super::super::ticks::restore(&root, &mut restored, &blocks).unwrap();
        assert_eq!(restored.chests, effective.chests);
        assert_eq!(restored.get_block(1, pos.y, 1), Some(chest));
        assert_eq!(
            restored.scheduled_block_ticks(),
            effective.scheduled_block_ticks()
        );
        equal(&restored.sections[0], &effective.sections[0]);
    }
}

#[test]
fn small_compressed_savings_keep_previous_representation() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let air = state(&blocks, "minecraft:air");
    let stone = state(&blocks, "minecraft:stone");
    let base = ChunkSection::filled(air, air);
    let mut effective = base.clone();
    effective.set(1, 2, 3, stone);
    let temp = tempfile::tempdir().unwrap();
    let database = database(&temp.path().join("state.redb"));
    let sparse = save(&database, &base, &effective, &blocks);
    assert_eq!(representation(&sparse).unwrap(), Representation::Sparse);
    let transaction = database.begin_write().unwrap();
    let dense = {
        let mut dictionary = Writer::open(&transaction).unwrap();
        let ids = [
            dictionary.intern(air, &blocks).unwrap(),
            dictionary.intern(stone, &blocks).unwrap(),
        ];
        let mut body = Vec::new();
        append_palette(&mut body, &ids);
        let target = 2 * 256 + 3 * 16 + 1;
        for word in 0..SECTION_VOLUME / 64 {
            let value = if word == target / 64 {
                1u64 << (target % 64)
            } else {
                0
            };
            body.extend_from_slice(&value.to_be_bytes());
        }
        let dense = compress(DENSE, &body).unwrap();
        assert!(dense.len().saturating_sub(sparse.len()) < 32);
        transaction
            .open_table(OVERLAYS)
            .unwrap()
            .insert(0, dense.as_slice())
            .unwrap();
        dense
    };
    transaction.commit().unwrap();
    equal(&load(&database, &base, &blocks), &effective);
    let retained = save(&database, &base, &effective, &blocks);
    assert_eq!(representation(&retained).unwrap(), Representation::Dense);
    equal(&load(&database, &base, &blocks), &effective);
    assert_eq!(decompress(&retained).unwrap(), decompress(&dense).unwrap());
}

#[test]
fn persisted_mixed_overlay_keeps_names_and_properties_with_reordered_registry() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let air = state(&blocks, "minecraft:air");
    let stone = state(&blocks, "minecraft:stone");
    let water = state(&blocks, "minecraft:water");
    let base = ChunkSection::filled(air, air);
    let mut effective = base.clone();
    effective.set(1, 2, 3, stone);
    effective.set(4, 5, 6, water);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.redb");
    let database = database(&path);
    save(&database, &base, &effective, &blocks);
    drop(database);
    let mut report = solaris_required_blocks_report();
    let total: u32 = report.iter().map(|block| block.states.len() as u32).sum();
    for block in &mut report {
        for state in &mut block.states {
            state.id = total - 1 - state.id;
        }
    }
    let reordered = BlockRegistry::from_report(&report).unwrap();
    let new_air = state(&reordered, "minecraft:air");
    let database = Database::open(&path).unwrap();
    let restored = load(
        &database,
        &ChunkSection::filled(new_air, new_air),
        &reordered,
    );
    let expected = [(1, 2, 3, stone), (4, 5, 6, water), (0, 0, 0, air)];
    for (x, y, z, original) in expected {
        let original = blocks.by_id(original).unwrap();
        let actual = reordered.by_id(restored.get(x, y, z)).unwrap();
        assert_eq!(actual.block.id, original.block.id);
        assert_eq!(actual.properties, original.properties);
    }
}

#[test]
fn inspected_origins_match_dictionary_states_and_validate_the_whole_record() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let air = state(&blocks, "minecraft:air");
    let stone = state(&blocks, "minecraft:stone");
    let base = varied_base(&blocks);
    let mut sparse = base.clone();
    let target = 2 * 256 + 3 * 16 + 1;
    let final_state = if base.get(1, 2, 3) == stone {
        air
    } else {
        stone
    };
    sparse.set(1, 2, 3, final_state);
    let dense = ChunkSection::filled(stone, air);
    let temp = tempfile::tempdir().unwrap();
    let database = database(&temp.path().join("state.redb"));
    let bytes = save(&database, &base, &sparse, &blocks);
    {
        let transaction = database.begin_read().unwrap();
        let dictionary = dictionary::Reader::open(&transaction).unwrap();
        let (mode, id) = inspect_state(&bytes, target).unwrap().unwrap();
        assert_eq!(mode, SPARSE);
        assert_eq!(dictionary.resolve(id, &blocks).unwrap(), final_state);
        assert_eq!(inspect_state(&bytes, 0).unwrap(), None);
    }
    let bytes = save(&database, &base, &dense, &blocks);
    {
        let transaction = database.begin_read().unwrap();
        let dictionary = dictionary::Reader::open(&transaction).unwrap();
        for index in [0, target, SECTION_VOLUME - 1] {
            let (mode, id) = inspect_state(&bytes, index).unwrap().unwrap();
            assert_eq!(mode, DENSE);
            assert_eq!(dictionary.resolve(id, &blocks).unwrap(), stone);
        }
        assert!(inspect_state(&bytes, SECTION_VOLUME).is_err());
    }
    // The requested first cell is valid, but the later duplicate index is not.
    let mut invalid = vec![0, 2, 0, 7, 0, 0, 0, 7, 0, 0];
    append_palette(&mut invalid, &[0]);
    assert!(inspect_state(&compress(SPARSE, &invalid).unwrap(), 7).is_err());
    // Five states use 3 bits and leave the high bit in each u64 unused.
    let mut padding = Vec::new();
    append_palette(&mut padding, &[0, 1, 2, 3, 4]);
    padding.resize(padding.len() + SECTION_VOLUME.div_ceil(21) * 8, 0);
    padding[22] = 0x80;
    assert!(inspect_state(&compress(DENSE, &padding).unwrap(), 0).is_err());
    let mut out_of_palette = Vec::new();
    append_palette(&mut out_of_palette, &[0, 1, 2]);
    out_of_palette.resize(out_of_palette.len() + 1024, 0);
    out_of_palette[21] = 3;
    assert!(inspect_state(&compress(DENSE, &out_of_palette).unwrap(), 0).is_err());
}

#[test]
fn native_baseline_discards_palette_history_without_mutating_capture() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let air = state(&blocks, "minecraft:air");
    let stone = state(&blocks, "minecraft:stone");
    let mut chunk = Chunk::empty(
        ChunkPos { x: -2, z: 7 },
        air,
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for name in ["minecraft:stone", "minecraft:dirt", "minecraft:water"] {
        chunk.sections[0].set(0, 0, 0, state(&blocks, name));
    }
    chunk.sections[0].set(0, 0, 0, air);
    let snapshot = chunk.clone();
    let mut canonical = chunk.clone();
    canonical.sections[0] = ChunkSection::filled(air, air);
    assert_eq!(
        baseline(&chunk, &blocks, &items).unwrap(),
        baseline(&canonical, &blocks, &items).unwrap()
    );
    assert_eq!(snapshot.sections[0].palette().unwrap().len(), 4);
    assert_eq!(chunk.sections[0].palette().unwrap().len(), 4);
    let encoded = baseline(&chunk, &blocks, &items).unwrap();
    let root = decode(&encoded).unwrap();
    let restored = crate::anvil::chunk_nbt::chunk_from_nbt_with_items_at_position_at_tick(
        &root,
        chunk.pos,
        &blocks,
        Some(&items),
        &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        chunk.geometry(),
        0,
    )
    .unwrap();
    equal(&restored.sections[0], &chunk.sections[0]);
    assert!(restored.sections[0].palette().is_none());
    chunk.sections[0].set(1, 0, 0, stone);
    assert_eq!(snapshot.sections[0].get(1, 0, 0), air);
}
