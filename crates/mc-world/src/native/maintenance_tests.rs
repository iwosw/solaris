use mc_data::Identifier;
use mc_data::blocks::solaris_required_blocks_report;
use mc_data::items::solaris_required_items;
use mc_data::loot::chest_26_1_2::ChestLootCatalog;
use mc_nbt::Tag;
use redb::ReadableDatabase;

use super::super::{NativeChunkSnapshot, NativeEntitySnapshot, NativeStore, chunk_key};
use super::*;
use crate::{BlockRegistry, BlockStateId, Chunk, ChunkPos};

fn blocks() -> BlockRegistry {
    BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap()
}

fn state(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}

fn chunk(blocks: &BlockRegistry, x: i32, z: i32, inhabited: i64) -> Chunk {
    let mut chunk = Chunk::empty(
        ChunkPos { x, z },
        state(blocks, "minecraft:air"),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for bx in 0..16 {
        for bz in 0..16 {
            chunk
                .set_block(bx, 63, bz, state(blocks, "minecraft:stone"))
                .unwrap();
        }
    }
    set_inhabited(&mut chunk, inhabited);
    chunk
}

fn set_inhabited(chunk: &mut Chunk, ticks: i64) {
    chunk.extras.retain(|(key, _)| key != "InhabitedTime");
    chunk
        .extras
        .push(("InhabitedTime".into(), Tag::Long(ticks)));
}

fn request(keep: [i32; 4], max_inhabited_ticks: i64, dry_run: bool) -> NativePruneRequest {
    NativePruneRequest {
        dimension: 0,
        keep_min: [keep[0], keep[1]],
        keep_max: [keep[2], keep[3]],
        max_inhabited_ticks,
        dry_run,
    }
}

fn stored(store: &NativeStore, blocks: &BlockRegistry, x: i32, z: i32) -> bool {
    let items = solaris_required_items();
    store
        .resolve_base(
            0,
            ChunkPos { x, z },
            blocks,
            &items,
            &ChestLootCatalog::new(),
            || Ok(None),
        )
        .unwrap()
        .is_some()
}

#[test]
fn prune_forgets_only_untouched_uninhabited_chunks_outside_the_rectangle() {
    let blocks = blocks();
    let items = solaris_required_items();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let store = NativeStore::create(&path, [5; 32]).unwrap();
    // (10,10) untouched; (11,10) untouched but generated with an inhabited clock;
    // (12,10) player-edited; (13,10) same content, clock advanced then stored;
    // (14,10) owns a persistent entity row; (0,0) untouched inside the rectangle;
    // (15,10) an Anvil-style import with a block no generator made, clock 0.
    let untouched = chunk(&blocks, 10, 10, 0);
    let lived_in = chunk(&blocks, 11, 10, 5_000);
    let edited = chunk(&blocks, 12, 10, 0);
    let clock = chunk(&blocks, 13, 10, 0);
    let tenant = chunk(&blocks, 14, 10, 0);
    let spawn = chunk(&blocks, 0, 0, 0);
    let mut built = chunk(&blocks, 15, 10, 0);
    built
        .set_block(5, 70, 5, state(&blocks, "minecraft:water"))
        .unwrap();
    store
        .fill_generated(
            0,
            &[
                untouched.clone(),
                lived_in.clone(),
                edited.clone(),
                clock.clone(),
                tenant.clone(),
                spawn.clone(),
            ],
            &blocks,
            &items,
        )
        .unwrap();
    store.import_chunk(0, &built, &blocks, &items).unwrap();
    let mut dug = edited.clone();
    dug.set_block(3, 63, 3, state(&blocks, "minecraft:air"))
        .unwrap();
    dug.set_world_journal_lsn(1);
    let mut advanced = clock.clone();
    set_inhabited(&mut advanced, 40);
    advanced.set_world_journal_lsn(2);
    store
        .apply_snapshots(
            &[
                NativeChunkSnapshot {
                    dimension: 0,
                    base: &edited,
                    effective: &dug,
                    dirty_sections: None,
                },
                NativeChunkSnapshot {
                    dimension: 0,
                    base: &clock,
                    effective: &advanced,
                    dirty_sections: None,
                },
            ],
            &blocks,
            &items,
        )
        .unwrap();
    store
        .apply_batch(
            &[],
            &[NativeEntitySnapshot {
                identity: [4; 16],
                dimension: 0,
                position: ChunkPos { x: 14, z: 10 },
                generated_slot: None,
                journal_lsn: 3,
                payload: Some(Tag::Compound(vec![("Health".into(), Tag::Float(20.0))])),
            }],
            &blocks,
            &items,
        )
        .unwrap();

    let keep = [-2, -2, 2, 2];
    let dry = store.prune_pristine(&request(keep, 0, true)).unwrap();
    assert_eq!(
        dry,
        NativePruneReport {
            examined: 7,
            pruned: 1,
            kept_inside_rectangle: 1,
            kept_inhabited: 2,
            kept_changed: 2,
            kept_not_generated: 1,
        }
    );
    assert_eq!(store.record_counts().unwrap().imported_chunks, 7, "dry run");

    // The same chunks with a generous clock limit: the advanced-clock chunk goes too.
    let lenient = store.prune_pristine(&request(keep, 100, true)).unwrap();
    assert_eq!((lenient.pruned, lenient.kept_inhabited), (2, 1));

    let report = store.prune_pristine(&request(keep, 0, false)).unwrap();
    assert_eq!(report, dry);
    drop(store);
    let store = NativeStore::open(&path, [5; 32]).unwrap();
    assert_eq!(store.record_counts().unwrap().imported_chunks, 6);
    assert!(!stored(&store, &blocks, 10, 10));
    for (x, z) in [(11, 10), (12, 10), (13, 10), (14, 10), (15, 10), (0, 0)] {
        assert!(stored(&store, &blocks, x, z), "({x},{z}) must be kept");
    }
    // The imported build survived intact: the generator could not recreate it.
    let imported = store
        .resolve_base(
            0,
            ChunkPos { x: 15, z: 10 },
            &blocks,
            &items,
            &ChestLootCatalog::new(),
            || Ok(None),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        imported.get_block(5, 70, 5),
        Some(state(&blocks, "minecraft:water"))
    );
    // The edit survives the prune of its neighbours.
    let restored = store
        .compose(0, &edited, &blocks, &items, &ChestLootCatalog::new())
        .unwrap();
    assert_eq!(
        restored.get_block(3, 63, 3),
        Some(state(&blocks, "minecraft:air"))
    );
    // A second run finds nothing left to forget.
    let again = store.prune_pristine(&request(keep, 0, false)).unwrap();
    assert_eq!(again.pruned, 0);
}

#[test]
fn prune_drops_the_metadata_and_watermark_rows_with_the_baseline() {
    let blocks = blocks();
    let items = solaris_required_items();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let store = NativeStore::create(&path, [5; 32]).unwrap();
    let base = chunk(&blocks, 30, -30, 0);
    store
        .fill_generated(0, std::slice::from_ref(&base), &blocks, &items)
        .unwrap();
    let mut advanced = base.clone();
    set_inhabited(&mut advanced, 7);
    advanced.set_world_journal_lsn(1);
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &base,
                effective: &advanced,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    let before = store.record_counts().unwrap();
    assert_eq!((before.metadata_chunks, before.applied_chunks), (1, 1));
    let report = store
        .prune_pristine(&request([0, 0, 0, 0], 10, false))
        .unwrap();
    assert_eq!(report.pruned, 1);
    let after = store.record_counts().unwrap();
    assert_eq!(
        (
            after.imported_chunks,
            after.metadata_chunks,
            after.applied_chunks
        ),
        (0, 0, 0)
    );
}

#[test]
fn repack_rewrites_plain_rows_refuses_corrupt_ones_and_compact_keeps_data() {
    let blocks = blocks();
    let items = solaris_required_items();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let position = ChunkPos { x: 4, z: 4 };
    let base = chunk(&blocks, 4, 4, 0);
    let key = chunk_key(0, position);
    let plain = {
        let store = NativeStore::create(&path, [6; 32]).unwrap();
        store.import_chunk(0, &base, &blocks, &items).unwrap();
        drop(store);
        let database = redb::Database::open(&path).unwrap();
        let transaction = database.begin_read().unwrap();
        let table = transaction.open_table(IMPORTED).unwrap();
        table.get(key.as_slice()).unwrap().unwrap().value().to_vec()
    };
    let raw = pack::unpack(&plain, 16 * 1024 * 1024).unwrap().into_owned();
    let replace = |bytes: &[u8]| {
        let database = redb::Database::open(&path).unwrap();
        let transaction = database.begin_write().unwrap();
        {
            let mut table = transaction.open_table(IMPORTED).unwrap();
            table.insert(key.as_slice(), bytes).unwrap();
        }
        transaction.commit().unwrap();
    };

    replace(&raw);
    let store = NativeStore::open(&path, [6; 32]).unwrap();
    assert_eq!(store.repack_rows().unwrap(), 1);
    assert_eq!(store.repack_rows().unwrap(), 0, "already packed");
    store.compact().unwrap();
    let store = NativeStore::open(&path, [6; 32]).unwrap();
    assert!(stored(&store, &blocks, 4, 4));
    drop(store);

    // A plain row that does not decode must stop the run, not be copied.
    let mut broken = raw.clone();
    broken.truncate(broken.len() - 9);
    replace(&broken);
    let store = NativeStore::open(&path, [6; 32]).unwrap();
    assert!(store.repack_rows().is_err());
}
