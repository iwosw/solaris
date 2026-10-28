use mc_data::Identifier;
use mc_data::blocks::solaris_required_blocks_report;
use mc_data::items::solaris_required_items;

use super::*;
use crate::{BlockStateId, ChunkSection};

fn state(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}

fn empty(blocks: &BlockRegistry, position: ChunkPos) -> Chunk {
    Chunk::empty(
        position,
        state(blocks, "minecraft:air"),
        Identifier::parse("minecraft:plains").unwrap(),
    )
}

#[test]
fn dense_all_air_is_a_replacement_not_an_absent_override() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let air = state(&blocks, "minecraft:air");
    let stone = state(&blocks, "minecraft:stone");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let store = NativeStore::create(&path, [7; 32]).unwrap();
    let mut baseline = empty(&blocks, ChunkPos { x: -3, z: 4 });
    baseline.sections[0] = ChunkSection::filled(stone, air);
    let mut removed = baseline.clone();
    removed.sections[0] = ChunkSection::filled(air, air);
    removed.set_world_journal_lsn(1);
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &baseline,
                effective: &removed,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    assert_eq!(store.record_counts().unwrap().overlay_sections, 1);
    drop(store);
    let store = NativeStore::open(&path, [7; 32]).unwrap();
    let restored = store
        .compose(
            0,
            &baseline,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                assert_eq!(restored.sections[0].get(x, y, z), air);
            }
        }
    }
}

#[test]
fn decorated_pot_26_3_native_reopen_migrates_old_sherds_without_losing_items() {
    use mc_nbt::{ListTag, Tag, tag_type};
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pot.redb");
    let baseline = empty(&blocks, ChunkPos { x: 0, z: 0 });
    let pos = crate::chunk::BlockPos { x: 1, y: 64, z: 1 };
    let item = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:stone".into())),
        ("count".into(), Tag::Int(3)),
    ]);
    let old = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:decorated_pot".into())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        (
            "sherds".into(),
            Tag::List(ListTag {
                element_type: tag_type::STRING,
                elements: vec![Tag::String("minecraft:angler_pottery_sherd".into())],
            }),
        ),
        ("item".into(), item.clone()),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &old).unwrap();
    let mut changed = baseline.clone();
    changed
        .set_block(1, 64, 1, state(&blocks, "minecraft:decorated_pot"))
        .unwrap();
    changed.block_entities.insert(pos, bytes);
    changed.set_world_journal_lsn(1);
    let store = NativeStore::create(&path, [9; 32]).unwrap();
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &baseline,
                effective: &changed,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    drop(store);
    let store = NativeStore::open(&path, [9; 32]).unwrap();
    let mut restored = store
        .compose(
            0,
            &baseline,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    let Tag::Compound(mut fields) =
        mc_nbt::read_network(&mut restored.block_entities[&pos].as_slice()).unwrap()
    else {
        panic!()
    };
    assert_eq!(mc_data::item_nbt::field(&fields, "item"), Some(&item));
    let Some(Tag::Compound(sides)) = mc_data::item_nbt::field(&fields, "sherds") else {
        panic!("an existing native world's legacy pot must migrate on read")
    };
    let Some(Tag::Compound(back)) = mc_data::item_nbt::field(sides, "back") else {
        panic!()
    };
    assert_eq!(
        mc_data::item_nbt::field(back, "id"),
        Some(&Tag::String("minecraft:angler_pottery_sherd".into()))
    );
    let Some((_, Tag::Compound(sides))) = fields.iter_mut().find(|(key, _)| key == "sherds") else {
        panic!()
    };
    let Some((_, Tag::Compound(back))) = sides.iter_mut().find(|(key, _)| key == "back") else {
        panic!()
    };
    back.push(("count".into(), Tag::Int(2)));
    back.push((
        "components".into(),
        Tag::Compound(vec![
            ("minecraft:rarity".into(), Tag::String("rare".into())),
            (
                "minecraft:custom_data".into(),
                Tag::Compound(vec![(
                    "sherds".into(),
                    Tag::String("opaque data is not a pot record".into()),
                )]),
            ),
        ]),
    ));
    let canonical = Tag::Compound(fields);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &canonical).unwrap();
    restored.block_entities.insert(pos, bytes);
    restored.set_world_journal_lsn(2);
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &baseline,
                effective: &restored,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    drop(store);
    let store = NativeStore::open(&path, [9; 32]).unwrap();
    let restored = store
        .compose(
            0,
            &baseline,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    assert_eq!(
        mc_nbt::read_network(&mut restored.block_entities[&pos].as_slice()).unwrap(),
        canonical,
        "canonical template count, components, opaque custom data and stored contents survive another save/reopen"
    );
}

#[test]
fn failed_participant_rolls_back_every_overlay_and_watermark() {
    let temp = tempfile::tempdir().unwrap();
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let store = NativeStore::create(&temp.path().join("state.redb"), [1; 32]).unwrap();
    let first = empty(&blocks, ChunkPos { x: -3, z: 7 });
    let second = empty(&blocks, ChunkPos { x: 9, z: -4 });
    let mut changed = first.clone();
    changed
        .set_block(1, 64, 1, state(&blocks, "minecraft:stone"))
        .unwrap();
    changed.set_world_journal_lsn(1);
    let mut unstamped = second.clone();
    unstamped
        .set_block(1, 64, 1, state(&blocks, "minecraft:water"))
        .unwrap();
    let result = store.apply_snapshots(
        &[
            NativeChunkSnapshot {
                dimension: 0,
                base: &first,
                effective: &changed,
                dirty_sections: None,
            },
            NativeChunkSnapshot {
                dimension: 0,
                base: &second,
                effective: &unstamped,
                dirty_sections: None,
            },
        ],
        &blocks,
        &items,
    );
    assert!(matches!(
        result,
        Err(NativeError::Invalid(
            "changed snapshot must have a journal decision"
        ))
    ));
    assert_eq!(
        store.record_counts().unwrap(),
        NativeRecordCounts::default()
    );
    let restored = store
        .compose(
            0,
            &first,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    assert_eq!(
        restored.get_block(1, 64, 1),
        Some(state(&blocks, "minecraft:air"))
    );
    assert_eq!(restored.world_journal_lsn(), 0);
}

#[test]
fn stale_lsn_cannot_restore_old_blocks_and_revert_keeps_scheduled_history() {
    let temp = tempfile::tempdir().unwrap();
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let path = temp.path().join("state.redb");
    let store = NativeStore::create(&path, [2; 32]).unwrap();
    let baseline = empty(&blocks, ChunkPos { x: 0, z: 0 });
    let mut old = baseline.clone();
    old.set_block(1, 64, 1, state(&blocks, "minecraft:stone"))
        .unwrap();
    old.set_world_journal_lsn(1);
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &baseline,
                effective: &old,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    let mut reverted = baseline.clone();
    reverted.set_world_journal_lsn(2);
    reverted.schedule_block_tick(crate::ScheduledBlockTick::new(
        crate::BlockPos { x: 1, y: 64, z: 1 },
        Identifier::parse("minecraft:stone").unwrap(),
        17,
        1,
    ));
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &baseline,
                effective: &reverted,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    assert_eq!(
        store
            .apply_snapshots(
                &[NativeChunkSnapshot {
                    dimension: 0,
                    base: &baseline,
                    effective: &old,
                    dirty_sections: None,
                }],
                &blocks,
                &items,
            )
            .unwrap(),
        0
    );
    assert_eq!(store.record_counts().unwrap().overlay_sections, 0);
    drop(store);
    let store = NativeStore::open(&path, [2; 32]).unwrap();
    let restored = store
        .compose(
            0,
            &baseline,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    assert_eq!(
        restored.get_block(1, 64, 1),
        Some(state(&blocks, "minecraft:air"))
    );
    assert_eq!(restored.world_journal_lsn(), 2);
    assert_eq!(restored.scheduled_block_ticks()[0].trigger_tick, 17);
}

#[test]
fn sparse_stable_states_survive_registry_reordering_and_unknown_states_fail_closed() {
    let report = solaris_required_blocks_report();
    let blocks = BlockRegistry::from_report(&report).unwrap();
    let items = solaris_required_items();
    let temp = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&temp.path().join("state.redb"), [3; 32]).unwrap();
    let baseline = empty(&blocks, ChunkPos { x: 0, z: 0 });
    let mut changed = baseline.clone();
    changed
        .set_block(1, 64, 1, state(&blocks, "minecraft:water"))
        .unwrap();
    changed.set_world_journal_lsn(1);
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &baseline,
                effective: &changed,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    let mut reordered = report.clone();
    let mut next_id = 0;
    for block in reordered.iter_mut().rev() {
        for state in &mut block.states {
            state.id = next_id;
            next_id += 1;
        }
    }
    let reordered = BlockRegistry::from_report(&reordered).unwrap();
    assert_ne!(
        state(&blocks, "minecraft:water"),
        state(&reordered, "minecraft:water")
    );
    let restored = store
        .compose(
            0,
            &empty(&reordered, baseline.pos),
            &reordered,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    assert_eq!(
        restored.get_block(1, 64, 1),
        Some(state(&reordered, "minecraft:water"))
    );
    let mut unknown = report;
    let water = unknown
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:water")
        .unwrap();
    water.id = Identifier::parse("solaris:missing_water").unwrap();
    let unknown = BlockRegistry::from_report(&unknown).unwrap();
    assert!(
        store
            .compose(
                0,
                &empty(&unknown, baseline.pos),
                &unknown,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new()
            )
            .is_err()
    );
}

#[test]
fn missing_database_wrong_identity_and_imported_source_switch_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.redb");
    assert!(NativeStore::open(&path, [4; 32]).is_err());
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let store = NativeStore::create(&path, [4; 32]).unwrap();
    assert!(NativeStore::create(&path, [4; 32]).is_err());
    let chunk = empty(&blocks, ChunkPos { x: -1, z: -1 });
    store.import_chunk(0, &chunk, &blocks, &items).unwrap();
    assert!(store.import_chunk(0, &chunk, &blocks, &items).is_err());
    drop(store);
    assert!(NativeStore::open(&path, [5; 32]).is_err());
}

#[test]
fn native_save_reopen_preserves_both_boundary_light_sections() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let baseline = empty(&blocks, ChunkPos { x: 0, z: 0 });
    let mut effective = baseline.clone();
    let stone = state(&blocks, "minecraft:stone");
    effective.set_block(1, 80, 1, stone);
    effective.boundary_section_lights = [
        crate::SectionLight {
            block: Some(crate::LightSection::uniform(0)),
            sky: Some(crate::LightSection::uniform(255)),
        },
        crate::SectionLight {
            block: Some(crate::LightSection::uniform(17)),
            sky: Some(crate::LightSection::uniform(238)),
        },
    ];
    effective.set_world_journal_lsn(1);
    let store = NativeStore::create(&path, [8; 32]).unwrap();
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &baseline,
                effective: &effective,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    drop(store);
    let reopened = NativeStore::open(&path, [8; 32]).unwrap();
    let restored = reopened
        .compose(
            0,
            &baseline,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    assert_eq!(
        restored.boundary_section_lights,
        effective.boundary_section_lights
    );
    assert_eq!(restored.geometry(), crate::OVERWORLD_GEOMETRY);
    assert_eq!(restored.get_block(1, 80, 1), Some(stone));
}
