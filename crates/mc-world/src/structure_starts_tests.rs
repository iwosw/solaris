use super::structure_starts::{insert, read};
use crate::{
    BlockPos, BlockRegistry, BlockStateId, Chunk, ChunkPos,
    chunk::GeneratedStructureSite,
    native::{NativeChunkSnapshot, NativeStore},
};
use mc_data::{Identifier, blocks::solaris_required_blocks_report, items::solaris_required_items};
use mc_nbt::{ListTag, Tag};

fn site() -> GeneratedStructureSite {
    GeneratedStructureSite {
        structure: Identifier::parse("minecraft:stronghold").unwrap(),
        start_chunk: (7, 8),
        position: BlockPos {
            x: 119,
            y: -20,
            z: 133,
        },
    }
}
fn chunk() -> Chunk {
    Chunk::empty(
        ChunkPos { x: 7, z: 8 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    )
}

#[test]
fn structure_starts_preserve_anchor_separately_from_bounds() {
    let mut chunk = chunk();
    let site = site();
    insert(
        &mut chunk,
        &site,
        BlockPos {
            x: 100,
            y: -30,
            z: 120,
        },
        BlockPos {
            x: 150,
            y: 0,
            z: 160,
        },
    );
    assert_eq!(read(&chunk.extras, chunk.pos, false).unwrap(), vec![site]);
    assert!(read(&chunk.extras, ChunkPos { x: 8, z: 8 }, false).is_err());
    assert!(read(&[("structures".into(), Tag::Int(1))], chunk.pos, false).is_err());
}

#[test]
fn imported_structure_starts_use_child_bounds_without_native_anchor() {
    let site = site();
    let fields = vec![(
        "structures".into(),
        Tag::Compound(vec![(
            "starts".into(),
            Tag::Compound(vec![(
                site.structure.to_string(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String(site.structure.to_string())),
                    ("ChunkX".into(), Tag::Int(7)),
                    ("ChunkZ".into(), Tag::Int(8)),
                    (
                        "Children".into(),
                        Tag::List(ListTag {
                            element_type: 10,
                            elements: vec![Tag::Compound(vec![(
                                "BB".into(),
                                Tag::IntArray(vec![119, -20, 133, 130, -10, 140]),
                            )])],
                        }),
                    ),
                ]),
            )]),
        )]),
    )];
    assert_eq!(
        read(&fields, ChunkPos { x: 7, z: 8 }, false).unwrap(),
        vec![site]
    );
}

#[test]
fn structure_starts_native_query_keeps_imported_empty_coverage_and_metadata_override() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("world.redb");
    let store = NativeStore::create(&path, [9; 32]).unwrap();
    let mut base = chunk();
    let site = site();
    insert(&mut base, &site, site.position, site.position);
    store.import_chunk(0, &base, &blocks, &items).unwrap();
    let mut empty = chunk();
    empty.pos = ChunkPos { x: 8, z: 8 };
    store.import_chunk(0, &empty, &blocks, &items).unwrap();
    let rows = store
        .checkpoint_reader()
        .unwrap()
        .structure_starts_in_region(0, (7, 8), (8, 8), false)
        .unwrap();
    assert_eq!(rows, vec![(base.pos, vec![site]), (empty.pos, vec![])]);
    let mut changed = base.clone();
    changed.extras.clear();
    changed.set_world_journal_lsn(1);
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &base,
                effective: &changed,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    drop(store);
    let reopened = NativeStore::open(&path, [9; 32]).unwrap();
    let rows = reopened
        .checkpoint_reader()
        .unwrap()
        .structure_starts_in_region(0, (7, 8), (8, 8), false)
        .unwrap();
    assert_eq!(rows, vec![(base.pos, vec![]), (empty.pos, vec![])]);
}

#[test]
fn structure_starts_procedural_baseline_metadata_is_not_discarded() {
    let blocks = BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap();
    let items = solaris_required_items();
    let dir = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&dir.path().join("world.redb"), [8; 32]).unwrap();
    let mut base = chunk();
    let site = site();
    insert(&mut base, &site, site.position, site.position);
    base.set_world_journal_lsn(1);
    let mut changed = base.clone();
    changed
        .set_block(
            0,
            64,
            0,
            blocks
                .block(&Identifier::parse("minecraft:stone").unwrap())
                .unwrap()
                .default,
        )
        .unwrap();
    store
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &base,
                effective: &changed,
                dirty_sections: None,
            }],
            &blocks,
            &items,
        )
        .unwrap();
    let rows = store
        .checkpoint_reader()
        .unwrap()
        .structure_starts_in_region(0, (7, 8), (7, 8), false)
        .unwrap();
    assert_eq!(rows, vec![(base.pos, vec![site])]);
}

#[test]
fn structure_starts_skip_referenced_not_merely_generated_and_reject_corruption() {
    let mut chunk = chunk();
    let site = site();
    insert(&mut chunk, &site, site.position, site.position);
    assert_eq!(
        read(&chunk.extras, chunk.pos, true).unwrap(),
        vec![site.clone()]
    );
    let set_references = |chunk: &mut Chunk, value| {
        let Tag::Compound(structures) = &mut chunk.extras[0].1 else {
            panic!()
        };
        let Tag::Compound(starts) = &mut structures[0].1 else {
            panic!()
        };
        let Tag::Compound(fields) = &mut starts[0].1 else {
            panic!()
        };
        fields
            .iter_mut()
            .find(|(key, _)| key == "references")
            .unwrap()
            .1 = value;
    };
    set_references(&mut chunk, Tag::Int(1));
    assert!(read(&chunk.extras, chunk.pos, true).unwrap().is_empty());
    assert_eq!(read(&chunk.extras, chunk.pos, false).unwrap(), vec![site]);
    set_references(&mut chunk, Tag::Int(-1));
    assert!(read(&chunk.extras, chunk.pos, true).is_err());
    set_references(&mut chunk, Tag::String("corrupt".into()));
    assert!(read(&chunk.extras, chunk.pos, false).is_err());
}
