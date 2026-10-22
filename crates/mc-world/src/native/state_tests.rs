use super::*;
use mc_data::{Identifier, blocks::solaris_required_blocks_report, items::solaris_required_items};
use mc_nbt::Tag;

fn blocks() -> BlockRegistry {
    BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap()
}
fn state(blocks: &BlockRegistry, name: &str) -> crate::BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}
fn base(blocks: &BlockRegistry) -> Chunk {
    Chunk::empty(
        ChunkPos { x: -2, z: 7 },
        state(blocks, "minecraft:air"),
        Identifier::parse("minecraft:plains").unwrap(),
    )
}
fn entity(lsn: u64) -> NativeEntitySnapshot {
    NativeEntitySnapshot {
        identity: [1; 16],
        dimension: 0,
        position: ChunkPos { x: -2, z: 7 },
        generated_slot: Some([9; 32]),
        journal_lsn: lsn,
        payload: Some(Tag::Compound(vec![
            ("Health".into(), Tag::Float(17.0)),
            (
                "Inventory".into(),
                Tag::String("owner-encoded-inventory".into()),
            ),
        ])),
    }
}

#[test]
fn s05_s06_generated_entity_move_death_and_slot_survive_restart_and_replay() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.redb");
    let blocks = blocks();
    let items = solaris_required_items();
    let store = NativeStore::create(&path, [8; 32]).unwrap();
    let first = entity(1);
    store
        .apply_batch(&[], std::slice::from_ref(&first), &blocks, &items)
        .unwrap();
    let mut moved = entity(2);
    moved.position = ChunkPos { x: 93, z: -11 };
    moved.dimension = 2;
    store
        .apply_batch(&[], std::slice::from_ref(&moved), &blocks, &items)
        .unwrap();
    assert_eq!(
        store.apply_batch(&[], &[first], &blocks, &items).unwrap(),
        0
    );
    drop(store);
    let store = NativeStore::open(&path, [8; 32]).unwrap();
    let current = store.entity([1; 16]).unwrap().unwrap();
    assert_eq!(current.position, moved.position);
    assert_eq!(current.dimension, 2);
    assert_eq!(current.payload, moved.payload);
    assert_eq!(store.record_counts().unwrap().persistent_entities, 1);
    assert_eq!(store.generated_slot([9; 32]).unwrap(), Some([1; 16]));
    let mut duplicate = entity(3);
    duplicate.identity = [2; 16];
    assert!(
        store
            .apply_batch(&[], &[duplicate], &blocks, &items)
            .is_err()
    );
    moved.payload = None;
    moved.journal_lsn = 3;
    store.apply_batch(&[], &[moved], &blocks, &items).unwrap();
    drop(store);
    let store = NativeStore::open(&path, [8; 32]).unwrap();
    assert!(store.entity([1; 16]).unwrap().unwrap().payload.is_none());
    assert_eq!(store.generated_slot([9; 32]).unwrap(), Some([1; 16]));
    let mut resurrected = entity(4);
    resurrected.dimension = 2;
    assert!(
        store
            .apply_batch(&[], &[resurrected], &blocks, &items)
            .is_err()
    );
}

#[test]
fn native_owner_checkpoint_moves_and_tombstones_generated_entities_atomically() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.redb");
    let store = NativeStore::create(&path, [8; 32]).unwrap();
    assert!(store.load_entity_checkpoint().unwrap().is_none());
    let metadata = Tag::Compound(vec![("Sequence".into(), Tag::Long(3))]);
    let first = entity(0);
    store
        .save_entity_checkpoint(metadata.clone(), std::slice::from_ref(&first))
        .unwrap();
    let mut moved = first.clone();
    moved.position = ChunkPos { x: 93, z: -11 };
    moved.dimension = 2;
    store
        .save_entity_checkpoint(metadata.clone(), &[moved.clone()])
        .unwrap();
    drop(store);
    let store = NativeStore::open(&path, [8; 32]).unwrap();
    let (_, restored) = store.load_entity_checkpoint().unwrap().unwrap();
    assert_eq!(restored[0].position, moved.position);
    assert_eq!(restored[0].dimension, 2);
    assert_eq!(restored[0].journal_lsn, 2);
    store.save_entity_checkpoint(metadata.clone(), &[]).unwrap();
    let (restored_metadata, restored) = store.load_entity_checkpoint().unwrap().unwrap();
    assert_eq!(restored_metadata, metadata);
    assert!(restored[0].payload.is_none());
    assert_eq!(restored[0].generated_slot, first.generated_slot);
    assert_eq!(store.generated_slot([9; 32]).unwrap(), Some(first.identity));
    let changed_metadata = Tag::Compound(vec![("Sequence".into(), Tag::Long(4))]);
    assert!(
        store
            .save_entity_checkpoint(changed_metadata, &[moved])
            .is_err()
    );
    assert_eq!(store.load_entity_checkpoint().unwrap().unwrap().0, metadata);
    assert_eq!(
        store.entity(first.identity).unwrap().unwrap().journal_lsn,
        3
    );
}

#[test]
fn s10_entity_failure_aborts_blocks_metadata_and_watermarks_together() {
    let root = tempfile::tempdir().unwrap();
    let blocks = blocks();
    let items = solaris_required_items();
    let store = NativeStore::create(&root.path().join("state.redb"), [3; 32]).unwrap();
    let base = base(&blocks);
    let mut effective = base.clone();
    effective
        .set_block(1, 64, 1, state(&blocks, "minecraft:water"))
        .unwrap();
    effective.set_world_journal_lsn(1);
    let invalid = NativeEntitySnapshot {
        journal_lsn: 0,
        ..entity(1)
    };
    assert!(
        store
            .apply_batch(
                &[NativeChunkSnapshot {
                    dimension: 0,
                    base: &base,
                    effective: &effective,
                    dirty_sections: None,
                }],
                &[invalid],
                &blocks,
                &items,
            )
            .is_err()
    );
    assert_eq!(
        store.record_counts().unwrap(),
        NativeRecordCounts::default()
    );
    assert_eq!(
        store
            .compose(
                0,
                &base,
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new()
            )
            .unwrap()
            .get_block(1, 64, 1),
        base.get_block(1, 64, 1)
    );
}

#[test]
fn s13_schema_one_is_explicitly_unsupported_and_lost_database_is_not_fresh() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.redb");
    assert!(NativeStore::open(&path, [3; 32]).is_err());
    let store = NativeStore::create(&path, [3; 32]).unwrap();
    let transaction = store.database.begin_write().unwrap();
    {
        let mut header = transaction.open_table(HEADER).unwrap();
        header
            .insert("schema", 1_u32.to_be_bytes().as_slice())
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(store);
    assert!(matches!(
        NativeStore::open(&path, [3; 32]),
        Err(NativeError::UnsupportedSchema(1))
    ));
    let error = NativeStore::open(&path, [3; 32]).err().unwrap().to_string();
    assert_eq!(
        error,
        "unsupported native schema 1; explicit offline conversion required"
    );
    std::fs::remove_file(&path).unwrap();
    assert!(NativeStore::open(&path, [3; 32]).is_err());
    assert!(!path.exists());
}

#[test]
fn p09_corrupt_overlay_is_not_clean_base_and_readonly_never_repairs() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.redb");
    let blocks = blocks();
    let items = solaris_required_items();
    let store = NativeStore::create(&path, [3; 32]).unwrap();
    let base = base(&blocks);
    let transaction = store.database.begin_write().unwrap();
    {
        let mut overlays = transaction.open_table(OVERLAYS).unwrap();
        overlays
            .insert(section_key(0, base.pos, 4).as_slice(), &[255_u8][..])
            .unwrap();
    }
    transaction.commit().unwrap();
    assert!(
        store
            .compose(
                0,
                &base,
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new()
            )
            .is_err()
    );
    drop(store);
    let before = std::fs::read(&path).unwrap();
    let readonly = NativeReadOnlyStore::open_with_cache(&path, [3; 32], 1024 * 1024).unwrap();
    assert!(
        readonly
            .compose(
                0,
                &base,
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new()
            )
            .is_err()
    );
    drop(readonly);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn s10_process_crash_inside_real_transaction_recovers_old_snapshot() {
    const CHILD: &str = "SOLARIS_W4_CRASH_CHILD";
    if let Ok(path) = std::env::var(CHILD) {
        let store = NativeStore::open(Path::new(&path), [5; 32]).unwrap();
        let transaction = store.database.begin_write().unwrap();
        {
            let mut slots = transaction.open_table(state::INITIALIZED).unwrap();
            slots
                .insert([8_u8; 32].as_slice(), [4_u8; 16].as_slice())
                .unwrap();
        }
        // Exit without dropping/committing transaction or clean database shutdown.
        std::process::exit(37);
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.redb");
    drop(NativeStore::create(&path, [5; 32]).unwrap());
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("native::state_tests::s10_process_crash_inside_real_transaction_recovers_old_snapshot")
        .arg("--nocapture")
        .env(CHILD, &path)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(37));
    let store = NativeStore::open(&path, [5; 32]).unwrap();
    assert_eq!(store.generated_slot([8; 32]).unwrap(), None);
    assert_eq!(
        store.record_counts().unwrap(),
        NativeRecordCounts::default()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn s10_real_file_growth_failure_aborts_entity_batch() {
    const CHILD: &str = "SOLARIS_W4_FILE_LIMIT_CHILD";
    if let Ok(path) = std::env::var(CHILD) {
        let blocks = blocks();
        let items = solaris_required_items();
        let store = NativeStore::open(Path::new(&path), [6; 32]).unwrap();
        let entities = (1..=32)
            .map(|id| NativeEntitySnapshot {
                identity: [id; 16],
                generated_slot: None,
                payload: Some(Tag::Compound(vec![(
                    "LargeOwnerState".into(),
                    Tag::ByteArray(vec![42; 1024 * 1024]),
                )])),
                ..entity(1)
            })
            .collect::<Vec<_>>();
        assert!(store.apply_batch(&[], &entities, &blocks, &items).is_err());
        std::process::exit(38);
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.redb");
    drop(NativeStore::create(&path, [6; 32]).unwrap());
    let kib_limit = std::fs::metadata(&path).unwrap().len().div_ceil(1024) + 2048;
    let status = std::process::Command::new("sh").arg("-c").arg(format!("trap '' XFSZ; ulimit -f {kib_limit}; exec \"$1\" --exact native::state_tests::s10_real_file_growth_failure_aborts_entity_batch --nocapture")).arg("w4-file-limit").arg(std::env::current_exe().unwrap()).env(CHILD, &path).status().unwrap();
    assert_eq!(status.code(), Some(38));
    let store = NativeStore::open(&path, [6; 32]).unwrap();
    assert_eq!(
        store.record_counts().unwrap(),
        NativeRecordCounts::default()
    );
}

#[test]
fn s02_s07_reverted_chest_blocks_keep_consumed_inventory_and_runtime_history() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.redb");
    let blocks = blocks();
    let items = solaris_required_items();
    let mut baseline = base(&blocks);
    let position = crate::BlockPos {
        x: baseline.pos.x * 16 + 1,
        y: 64,
        z: baseline.pos.z * 16 + 1,
    };
    baseline
        .set_block(1, 64, 1, state(&blocks, "minecraft:chest"))
        .unwrap();
    baseline.dirty = false;
    let mut chest = crate::ChestBlockEntity::default();
    chest.slots[0].count = 7;
    chest.slots[0].item_id = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    baseline.chests.insert(position, chest);
    let mut broken = baseline.clone();
    broken
        .set_block(1, 64, 1, state(&blocks, "minecraft:air"))
        .unwrap();
    broken.chests.clear();
    broken.set_world_journal_lsn(1);
    let store = NativeStore::create(&path, [9; 32]).unwrap();
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &baseline,
                effective: &broken,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    let mut restored = baseline.clone();
    restored
        .chests
        .insert(position, crate::ChestBlockEntity::default());
    restored
        .extras
        .push(("InhabitedTime".into(), Tag::Long(123)));
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
    assert_eq!(store.record_counts().unwrap().overlay_sections, 0);
    drop(store);
    let store = NativeStore::open(&path, [9; 32]).unwrap();
    let effective = store
        .compose(
            0,
            &baseline,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    assert_eq!(effective.get_block(1, 64, 1), baseline.get_block(1, 64, 1));
    assert_eq!(
        effective.chests[&position],
        crate::ChestBlockEntity::default()
    );
    assert!(
        effective
            .extras
            .contains(&("InhabitedTime".into(), Tag::Long(123)))
    );
    assert_eq!(effective.world_journal_lsn(), 2);
}

#[test]
fn historical_tombstones_coexist_with_large_count_scalable_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let metadata = Tag::Compound(vec![("Sequence".into(), Tag::Long(1))]);
    let store = NativeStore::create(&root.path().join("state.redb"), [8; 32]).unwrap();
    let historical = (1..=32)
        .map(|id| NativeEntitySnapshot {
            identity: [id; 16],
            generated_slot: None,
            payload: None,
            ..entity(0)
        })
        .collect::<Vec<_>>();
    store
        .save_entity_checkpoint(metadata.clone(), &historical)
        .unwrap();

    // Each record remains below the individual 16 MiB limit. The aggregate is
    // count-scalable: retained tombstones must not push a valid checkpoint over
    // a fixed 64 MiB budget.
    let proposed = (100..104)
        .map(|id| NativeEntitySnapshot {
            identity: [id; 16],
            generated_slot: None,
            payload: Some(Tag::Compound(vec![(
                "Blob".into(),
                Tag::ByteArray(vec![0; 16 * 1024 * 1024 - 128]),
            )])),
            ..entity(0)
        })
        .collect::<Vec<_>>();
    let control = NativeStore::create(&root.path().join("control.redb"), [8; 32]).unwrap();
    control
        .save_entity_checkpoint(metadata.clone(), &proposed)
        .unwrap();
    assert_eq!(
        control.load_entity_checkpoint().unwrap().unwrap().1.len(),
        4
    );
    // A small crafted metadata after-image stays loadable next to large valid
    // records; only the per-record metadata bound applies.
    let small_metadata = Tag::Compound(vec![("Blob".into(), Tag::ByteArray(vec![0; 1024]))]);
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &small_metadata).unwrap();
    let transaction = control.database.begin_write().unwrap();
    {
        let mut header = transaction.open_table(HEADER).unwrap();
        header
            .insert("entity_checkpoint", bytes.as_slice())
            .unwrap();
    }
    transaction.commit().unwrap();
    assert_eq!(
        control.load_entity_checkpoint().unwrap().unwrap().1.len(),
        4
    );
    drop(control);

    let changed = Tag::Compound(vec![("Sequence".into(), Tag::Long(2))]);
    store
        .save_entity_checkpoint(changed.clone(), &proposed)
        .unwrap();
    drop(proposed);
    drop(store);
    let store = NativeStore::open(&root.path().join("state.redb"), [8; 32]).unwrap();
    let (loaded_metadata, retained) = store.load_entity_checkpoint().unwrap().unwrap();
    assert_eq!(loaded_metadata, changed);
    assert_eq!(retained.len(), historical.len() + 4);
    assert_eq!(
        retained.iter().filter(|row| row.payload.is_none()).count(),
        historical.len()
    );
    assert!(
        retained
            .iter()
            .filter(|row| row.payload.is_some())
            .all(|row| row.journal_lsn == 2)
    );
    assert!(store.entity([100; 16]).unwrap().is_some());
}

#[test]
fn count_scalable_checkpoint_above_64mib_round_trips() {
    use std::collections::BTreeMap;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.redb");
    let store = NativeStore::create(&path, [8; 32]).unwrap();
    let metadata = Tag::Compound(vec![("Sequence".into(), Tag::Long(7))]);
    const COUNT: u64 = 70;
    const BLOB: usize = 1024 * 1024;
    let snapshots = (0..COUNT)
        .map(|id| {
            let mut identity = [0xA5; 16];
            identity[0..8].copy_from_slice(&id.to_be_bytes());
            NativeEntitySnapshot {
                identity,
                generated_slot: None,
                payload: Some(Tag::Compound(vec![(
                    "Blob".into(),
                    Tag::ByteArray(vec![i8::try_from(id % 128).unwrap(); BLOB]),
                )])),
                ..entity(0)
            }
        })
        .collect::<Vec<_>>();
    store
        .save_entity_checkpoint(metadata.clone(), &snapshots)
        .unwrap();
    drop(store);
    let store = NativeStore::open(&path, [8; 32]).unwrap();
    let (loaded_metadata, loaded) = store.load_entity_checkpoint().unwrap().unwrap();
    assert_eq!(loaded_metadata, metadata);
    assert_eq!(loaded.len(), usize::try_from(COUNT).unwrap());
    let by_identity: BTreeMap<[u8; 16], &NativeEntitySnapshot> =
        loaded.iter().map(|row| (row.identity, row)).collect();
    assert_eq!(by_identity.len(), usize::try_from(COUNT).unwrap());
    for spot in [0, COUNT / 2, COUNT - 1] {
        let mut identity = [0xA5; 16];
        identity[0..8].copy_from_slice(&spot.to_be_bytes());
        let row = by_identity[&identity];
        match &row.payload {
            Some(Tag::Compound(fields)) => {
                let (_, value) = fields.iter().find(|(k, _)| k == "Blob").unwrap();
                match value {
                    Tag::ByteArray(blob) => {
                        assert_eq!(blob.len(), BLOB);
                        assert!(blob.iter().all(|b| *b == i8::try_from(spot % 128).unwrap()));
                    }
                    _ => panic!("spot payload must stay a byte array"),
                }
            }
            _ => panic!("spot payload must stay a compound"),
        }
    }
    drop(store);

    // Individual corruption limits stay fail-closed.
    let root = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&root.path().join("oversized.redb"), [8; 32]).unwrap();
    let metadata = Tag::Compound(vec![("Sequence".into(), Tag::Long(1))]);
    let oversized = NativeEntitySnapshot {
        identity: [0xFF; 16],
        generated_slot: None,
        payload: Some(Tag::Compound(vec![(
            "Blob".into(),
            Tag::ByteArray(vec![0; 16 * 1024 * 1024]),
        )])),
        ..entity(0)
    };
    assert!(matches!(
        store.save_entity_checkpoint(metadata.clone(), std::slice::from_ref(&oversized)),
        Err(NativeError::Invalid(
            "native entity record exceeds byte budget"
        )),
    ));
    let oversized_metadata = Tag::Compound(vec![(
        "Blob".into(),
        Tag::ByteArray(vec![0; 16 * 1024 * 1024]),
    )]);
    assert!(matches!(
        store.save_entity_checkpoint(oversized_metadata, &[]),
        Err(NativeError::Invalid("native record exceeds byte budget")),
    ));
    assert!(store.load_entity_checkpoint().unwrap().is_none());
}
