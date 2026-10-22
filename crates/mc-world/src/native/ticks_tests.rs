use super::*;
use crate::native::{NativeChunkSnapshot, NativeStore};
use mc_data::{blocks::solaris_required_blocks_report, items::solaris_required_items};

fn fixtures() -> (BlockRegistry, Chunk) {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let chunk = Chunk::empty(
        crate::ChunkPos { x: -1, z: 2 },
        air,
        Identifier::parse("minecraft:plains").unwrap(),
    );
    (blocks, chunk)
}

#[test]
fn native_ticks_preserve_past_and_far_future_u64_deadlines_and_sequence_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let (blocks, base) = fixtures();
    let items = solaris_required_items();
    let mut effective = base.clone();
    let position = BlockPos {
        x: -15,
        y: 64,
        z: 33,
    };
    effective.schedule_fluid_tick(ScheduledFluidTick::new(
        position,
        Identifier::parse("minecraft:water").unwrap(),
        20,
        1,
    ));
    effective.schedule_block_tick(ScheduledBlockTick::new(
        position,
        Identifier::parse("minecraft:stone").unwrap(),
        u64::MAX - 10,
        -1,
    ));
    effective.set_world_journal_lsn(1);
    effective.extras.push(("LastUpdate".into(), Tag::Long(123)));
    let store = NativeStore::create(&path, [19; 32]).unwrap();
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &base,
                effective: &effective,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    drop(store);
    let store = NativeStore::open(&path, [19; 32]).unwrap();
    let actual = store
        .compose(
            0,
            &base,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    assert_eq!(
        actual.scheduled_block_ticks(),
        effective.scheduled_block_ticks()
    );
    assert_eq!(
        actual.scheduled_fluid_ticks(),
        effective.scheduled_fluid_ticks()
    );
    assert!(
        actual
            .extras
            .contains(&("LastUpdate".into(), Tag::Long(123)))
    );
    assert!(
        !actual
            .extras
            .iter()
            .any(|(key, _)| key == BLOCK || key == FLUID)
    );
}

#[test]
fn full_anvil_tick_payload_stays_exact_vanilla_relative_delay_encoding() {
    let (blocks, mut chunk) = fixtures();
    let items = solaris_required_items();
    let position = BlockPos {
        x: -15,
        y: 64,
        z: 33,
    };
    chunk.schedule_fluid_tick(ScheduledFluidTick::new(
        position,
        Identifier::parse("minecraft:water").unwrap(),
        99,
        2,
    ));
    let root =
        crate::anvil::chunk_nbt::chunk_to_nbt_with_items_at_tick(&chunk, &blocks, Some(&items), 20)
            .unwrap();
    let Tag::Compound(fields) = root else {
        panic!("compound");
    };
    assert!(!fields.iter().any(|(key, _)| key == BLOCK || key == FLUID));
    let actual = &fields
        .iter()
        .find(|(key, _)| key == "fluid_ticks")
        .unwrap()
        .1;
    let expected = Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements: vec![Tag::Compound(vec![
            ("i".into(), Tag::String("minecraft:water".into())),
            ("x".into(), Tag::Int(-15)),
            ("y".into(), Tag::Int(64)),
            ("z".into(), Tag::Int(33)),
            ("t".into(), Tag::Int(79)),
            ("p".into(), Tag::Int(2)),
        ])],
    });
    let actual = Tag::Compound(vec![("fluid_ticks".into(), actual.clone())]);
    let expected = Tag::Compound(vec![("fluid_ticks".into(), expected)]);
    assert_eq!(
        codec::encode(&actual).unwrap(),
        codec::encode(&expected).unwrap()
    );
}

#[test]
fn native_tick_corruption_rejects_missing_fields_foreign_positions_and_unknown_types() {
    let (blocks, mut chunk) = fixtures();
    chunk.schedule_fluid_tick(ScheduledFluidTick::new(
        BlockPos {
            x: -15,
            y: 64,
            z: 33,
        },
        Identifier::parse("minecraft:water").unwrap(),
        99,
        2,
    ));
    let mut root = Tag::Compound(vec![]);
    append(&mut root, &chunk).unwrap();
    for bad in [Tag::Int(9), Tag::String("removed:water".into())] {
        let mut corrupt = root.clone();
        let Tag::Compound(fields) = &mut corrupt else {
            unreachable!()
        };
        let (_, Tag::List(ticks)) = fields.iter_mut().find(|(key, _)| key == FLUID).unwrap() else {
            unreachable!()
        };
        let Tag::Compound(fields) = &mut ticks.elements[0] else {
            unreachable!()
        };
        let key = if matches!(bad, Tag::Int(_)) {
            "x"
        } else {
            "type"
        };
        fields.iter_mut().find(|(name, _)| name == key).unwrap().1 = bad;
        assert!(restore(&corrupt, &mut chunk.clone(), &blocks).is_err());
    }
    let Tag::Compound(mut fields) = root else {
        unreachable!()
    };
    fields.retain(|(key, _)| key != BLOCK);
    assert!(restore(&Tag::Compound(fields), &mut chunk, &blocks).is_err());
}

struct ImportedOnly;
impl crate::ChunkGenerator for ImportedOnly {
    fn generate(&self, _: crate::ChunkPos) -> Chunk {
        panic!("stored imported coverage must not generate");
    }
}

#[test]
fn native_store_preserves_both_flowing_fluid_ids_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let (blocks, mut imported) = fixtures();
    let items = solaris_required_items();
    for (sequence, (name, deadline, priority)) in [
        ("minecraft:flowing_water", 91, 2),
        ("minecraft:flowing_lava", u64::MAX - 9, -2),
    ]
    .into_iter()
    .enumerate()
    {
        let fluid = Identifier::parse(name).unwrap();
        assert!(blocks.block(&fluid).is_none());
        imported.schedule_fluid_tick(ScheduledFluidTick::from_storage(
            BlockPos {
                x: -15 + sequence as i32,
                y: 64,
                z: 33,
            },
            fluid,
            deadline,
            priority,
            sequence as u64,
        ));
    }
    let store = NativeStore::create(&path, [23; 32]).unwrap();
    store.import_chunk(0, &imported, &blocks, &items).unwrap();
    drop(store);
    let store = NativeStore::open(&path, [23; 32]).unwrap();
    let restored = store
        .read_chunk(
            0,
            imported.pos,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
            &ImportedOnly,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.scheduled_fluid_ticks(),
        imported.scheduled_fluid_ticks()
    );
    assert_eq!(
        restored.scheduled_fluid_ticks()[0].fluid.as_str(),
        "minecraft:flowing_water"
    );
    assert_eq!(
        restored.scheduled_fluid_ticks()[1].fluid.as_str(),
        "minecraft:flowing_lava"
    );
    assert_eq!(
        restored.scheduled_fluid_ticks()[1].trigger_tick,
        u64::MAX - 9
    );
}

#[test]
fn native_store_rejects_unknown_fluid_ids_with_list_kind_and_name() {
    let (blocks, base) = fixtures();
    let items = solaris_required_items();
    for name in ["removed:water", "minecraft:stone", "minecraft:empty"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.redb");
        let mut imported = base.clone();
        imported.schedule_fluid_tick(ScheduledFluidTick::new(
            BlockPos {
                x: -15,
                y: 64,
                z: 33,
            },
            Identifier::parse(name).unwrap(),
            99,
            1,
        ));
        let store = NativeStore::create(&path, [24; 32]).unwrap();
        store.import_chunk(0, &imported, &blocks, &items).unwrap();
        drop(store);
        let store = NativeStore::open(&path, [24; 32]).unwrap();
        let error = store
            .read_chunk(
                0,
                imported.pos,
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                &ImportedOnly,
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("native source: unknown native fluid tick type {name}")
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [25; 32]).unwrap();
    let mut imported = base;
    imported.schedule_block_tick(ScheduledBlockTick::new(
        BlockPos {
            x: -15,
            y: 64,
            z: 33,
        },
        Identifier::parse("minecraft:flowing_water").unwrap(),
        99,
        1,
    ));
    store.import_chunk(0, &imported, &blocks, &items).unwrap();
    let error = store
        .read_chunk(
            0,
            imported.pos,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
            &ImportedOnly,
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "native source: unknown native block tick type minecraft:flowing_water"
    );
}
