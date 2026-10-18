use super::*;
use mc_data::blocks::solaris_required_blocks_report;
use redb::{Database, ReadableDatabase};

fn registry() -> BlockRegistry {
    BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap()
}

fn state(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}

fn database(path: &std::path::Path) -> Database {
    let database = Database::create(path).unwrap();
    let transaction = database.begin_write().unwrap();
    initialize(&transaction).unwrap();
    transaction.commit().unwrap();
    database
}

#[test]
fn dictionary_ids_are_world_wide_and_survive_runtime_id_reordering() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.redb");
    let blocks = registry();
    let water = state(&blocks, "minecraft:water");
    let stone = state(&blocks, "minecraft:stone");
    let database = database(&path);
    let transaction = database.begin_write().unwrap();
    let (water_id, stone_id) = {
        let mut writer = Writer::open(&transaction).unwrap();
        (
            writer.intern(water, &blocks).unwrap(),
            writer.intern(stone, &blocks).unwrap(),
        )
    };
    transaction.commit().unwrap();
    let transaction = database.begin_write().unwrap();
    {
        let mut writer = Writer::open(&transaction).unwrap();
        assert_eq!(writer.intern(stone, &blocks).unwrap(), stone_id);
        assert_eq!(writer.intern(water, &blocks).unwrap(), water_id);
    }
    transaction.commit().unwrap();
    drop(database);
    let mut report = solaris_required_blocks_report();
    let total: u32 = report.iter().map(|block| block.states.len() as u32).sum();
    for block in &mut report {
        for state in &mut block.states {
            state.id = total - 1 - state.id;
        }
    }
    let reordered = BlockRegistry::from_report(&report).unwrap();
    assert_ne!(stone, state(&reordered, "minecraft:stone"));
    let database = Database::open(&path).unwrap();
    let transaction = database.begin_read().unwrap();
    let reader = Reader::open(&transaction).unwrap();
    assert_eq!(
        reader.resolve(water_id, &reordered).unwrap(),
        state(&reordered, "minecraft:water")
    );
    assert_eq!(
        reader.resolve(stone_id, &reordered).unwrap(),
        state(&reordered, "minecraft:stone")
    );
    assert_eq!(db(reader.states.len()).unwrap(), 2);
}

#[test]
fn dictionary_refuses_removed_states_and_invalid_properties() {
    let temp = tempfile::tempdir().unwrap();
    let database = database(&temp.path().join("state.redb"));
    let blocks = registry();
    let stone = state(&blocks, "minecraft:stone");
    let malformed = [
        Tag::Compound(vec![(
            "Name".into(),
            Tag::String("minecraft:removed_block".into()),
        )]),
        Tag::Compound(vec![
            ("Name".into(), Tag::String("minecraft:stone".into())),
            (
                "Properties".into(),
                Tag::Compound(vec![(
                    "removed_property".into(),
                    Tag::String("true".into()),
                )]),
            ),
        ]),
        Tag::Compound(vec![
            ("Name".into(), Tag::String("minecraft:water".into())),
            (
                "Properties".into(),
                Tag::Compound(vec![
                    ("level".into(), Tag::String("0".into())),
                    ("level".into(), Tag::String("0".into())),
                ]),
            ),
        ]),
        Tag::Compound(vec![
            ("Name".into(), Tag::String("minecraft:water".into())),
            (
                "Properties".into(),
                Tag::Compound(vec![("level".into(), Tag::Int(0))]),
            ),
        ]),
    ];
    for tag in malformed {
        // Bypass the native encoder to seed deliberately corrupt persisted NBT.
        let mut bytes = Vec::new();
        mc_nbt::write_named(&mut bytes, "", &tag).unwrap();
        let transaction = database.begin_write().unwrap();
        {
            let mut states = transaction.open_table(STATES).unwrap();
            let mut ids = transaction.open_table(IDS).unwrap();
            states.insert(0, bytes.as_slice()).unwrap();
            ids.insert(bytes.as_slice(), 0).unwrap();
        }
        transaction.commit().unwrap();
        let transaction = database.begin_read().unwrap();
        // Index mismatches and semantic corruption both fail closed.
        match Reader::open(&transaction) {
            Ok(reader) => assert!(reader.resolve(0, &blocks).is_err()),
            Err(NativeError::Invalid(_)) => {}
            Err(error) => panic!("unexpected {error}"),
        }
        drop(transaction);
        let transaction = database.begin_write().unwrap();
        {
            transaction.open_table(STATES).unwrap().remove(0).unwrap();
            transaction
                .open_table(IDS)
                .unwrap()
                .remove(bytes.as_slice())
                .unwrap();
        }
        transaction.commit().unwrap();
    }
    let transaction = database.begin_write().unwrap();
    let id = {
        let mut writer = Writer::open(&transaction).unwrap();
        writer.intern(stone, &blocks).unwrap()
    };
    transaction.commit().unwrap();
    let mut report = solaris_required_blocks_report();
    report
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:stone")
        .unwrap()
        .id = Identifier::parse("minecraft:removed_stone").unwrap();
    let changed = BlockRegistry::from_report(&report).unwrap();
    let transaction = database.begin_read().unwrap();
    let reader = Reader::open(&transaction).unwrap();
    assert!(reader.resolve(id, &changed).is_err());
    assert!(reader.resolve(u32::MAX, &blocks).is_err());
}

#[test]
fn dictionary_missing_or_disagreeing_indexes_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let database = Database::create(temp.path().join("state.redb")).unwrap();
    let transaction = database.begin_read().unwrap();
    assert!(Reader::open(&transaction).is_err());
    drop(transaction);
    let transaction = database.begin_write().unwrap();
    initialize(&transaction).unwrap();
    let blocks = registry();
    let bytes = canonical(state(&blocks, "minecraft:stone"), &blocks).unwrap();
    transaction
        .open_table(STATES)
        .unwrap()
        .insert(0, bytes.as_slice())
        .unwrap();
    transaction.commit().unwrap();
    let transaction = database.begin_read().unwrap();
    assert!(Reader::open(&transaction).is_err());
}
