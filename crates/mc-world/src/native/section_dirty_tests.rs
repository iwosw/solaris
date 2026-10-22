//! Section-granular checkpoint writes: counters and reopen equality for
//! metadata-only, one-section, full-chunk, revert-to-baseline and no-op captures.

use super::checkpoint_tests::{globals, registries};
use super::*;
use crate::BlockStateId;
use mc_data::Identifier;

const CHUNKS: i32 = 8;
const SECTIONS: usize = 24;
const EDITED: [usize; 3] = [5, 9, 13];
const MIN_Y: i32 = -64;

fn bit(section: usize) -> u32 {
    1 << section
}

fn mask(sections: &[usize]) -> Option<u32> {
    Some(
        sections
            .iter()
            .fold(0, |mask, section| mask | bit(*section)),
    )
}

fn plains() -> Identifier {
    Identifier::parse("minecraft:plains").unwrap()
}

fn block(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}

fn edit_section(chunk: &mut Chunk, section: usize, state: BlockStateId) {
    chunk
        .set_block(1, MIN_Y + section as i32 * 16 + 1, 1, state)
        .unwrap();
}

struct Work<'a> {
    base: &'a Chunk,
    effective: &'a Chunk,
    dirty: Option<u32>,
}

fn checkpoint(store: &NativeStore, tick: u64, work: &[Work<'_>]) -> NativeCheckpointIo {
    let (blocks, items) = registries();
    let records = globals(tick);
    store
        .commit_checkpoint_stream(
            NativeCheckpoint {
                tick,
                previous_tick: tick - 1,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &records,
            },
            |write| {
                for item in work {
                    write(snapshot(item))?;
                }
                Ok(())
            },
            std::iter::empty::<Result<NativeEntitySnapshot, NativeError>>(),
            &blocks,
            &items,
        )
        .unwrap()
}

fn snapshot<'a>(item: &Work<'a>) -> NativeChunkSnapshot<'a> {
    NativeChunkSnapshot {
        dimension: 0,
        base: item.base,
        effective: item.effective,
        dirty_sections: item.dirty,
    }
}

/// `(rewritten chunks, section overlays evaluated, section records written)`.
fn counts(io: &NativeCheckpointIo) -> (usize, u64, u64) {
    (
        io.rewritten_chunks,
        io.sections_encoded,
        io.section_records_written,
    )
}

/// Require every chunk to compose from the store to the live image.
fn assert_store_equals(store: &NativeStore, bases: &[Chunk], effective: &[Chunk]) {
    let (blocks, items) = registries();
    for (base, expected) in bases.iter().zip(effective) {
        let composed = store
            .compose(
                0,
                base,
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
            )
            .unwrap();
        assert_eq!(composed.inhabited_time(), expected.inhabited_time());
        for (index, (left, right)) in composed.sections.iter().zip(&expected.sections).enumerate() {
            for cell in 0..crate::SECTION_VOLUME {
                let (x, y, z) = (
                    (cell % 16) as u8,
                    (cell / 256) as u8,
                    ((cell / 16) % 16) as u8,
                );
                assert_eq!(
                    left.get(x, y, z),
                    right.get(x, y, z),
                    "chunk {:?} section {index} cell {cell}",
                    base.pos
                );
            }
        }
    }
}

#[test]
fn section_granular_checkpoint_counters_and_reopen_equality() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let store = NativeStore::create(&path, [1; 32]).unwrap();
    let (blocks, items) = registries();
    let air = block(&blocks, "minecraft:air");
    let stone = block(&blocks, "minecraft:stone");
    let dirt = block(&blocks, "minecraft:dirt");
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 0,
                previous_tick: 0,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &globals(0),
            },
            &blocks,
            &items,
        )
        .unwrap();
    let bases: Vec<Chunk> = (0..CHUNKS)
        .map(|x| Chunk::empty(ChunkPos { x, z: 0 }, air, plains()))
        .collect();
    let mut live = bases.clone();
    for chunk in &mut live {
        for section in EDITED {
            edit_section(chunk, section, stone);
        }
    }
    assert_eq!(bases[0].sections.len(), SECTIONS);

    // Creation: every chunk carries three edited sections.
    let work: Vec<_> = bases
        .iter()
        .zip(&live)
        .map(|(base, effective)| Work {
            base,
            effective,
            dirty: mask(&EDITED),
        })
        .collect();
    let create = checkpoint(&store, 1, &work);
    assert_store_equals(&store, &bases, &live);
    eprintln!(
        "SECTION-COUNTERS create: chunks={} encoded={} records={} bytes={}",
        create.rewritten_chunks,
        create.sections_encoded,
        create.section_records_written,
        create.chunk_payload_bytes
    );
    assert_eq!(
        counts(&create),
        (CHUNKS as usize, (CHUNKS as u64) * EDITED.len() as u64, 24)
    );

    // A: metadata only (inhabited time) on every chunk.
    for chunk in &mut live {
        chunk.increment_inhabited_time(7);
    }
    let work: Vec<_> = bases
        .iter()
        .zip(&live)
        .map(|(base, effective)| Work {
            base,
            effective,
            dirty: mask(&[]),
        })
        .collect();
    let a = checkpoint(&store, 2, &work);
    assert_store_equals(&store, &bases, &live);
    eprintln!(
        "SECTION-COUNTERS A metadata-only x{CHUNKS}: chunks={} encoded={} records={} bytes={}",
        a.rewritten_chunks, a.sections_encoded, a.section_records_written, a.chunk_payload_bytes
    );
    assert_eq!(
        counts(&a),
        (CHUNKS as usize, 0, 0),
        "metadata-only change evaluates no section"
    );

    // B: one block edit in one section of one chunk.
    edit_section(&mut live[3], EDITED[0], dirt);
    let work = [Work {
        base: &bases[3],
        effective: &live[3],
        dirty: mask(&[EDITED[0]]),
    }];
    let b = checkpoint(&store, 3, &work);
    assert_store_equals(&store, &bases, &live);
    eprintln!(
        "SECTION-COUNTERS B one-section x1: chunks={} encoded={} records={} bytes={}",
        b.rewritten_chunks, b.sections_encoded, b.section_records_written, b.chunk_payload_bytes
    );
    assert_eq!(counts(&b), (1, 1, 1), "one edited section of 24");

    // Revert: that section returns to its baseline; the overlay record must go.
    let overlays_before = store.record_counts().unwrap().overlay_sections;
    live[3]
        .set_block(1, MIN_Y + EDITED[0] as i32 * 16 + 1, 1, air)
        .unwrap();
    let work = [Work {
        base: &bases[3],
        effective: &live[3],
        dirty: mask(&[EDITED[0]]),
    }];
    let revert = checkpoint(&store, 4, &work);
    assert_store_equals(&store, &bases, &live);
    assert_eq!(
        store.record_counts().unwrap().overlay_sections,
        overlays_before - 1,
        "section back at baseline loses its overlay record"
    );
    eprintln!(
        "SECTION-COUNTERS revert x1: chunks={} encoded={} records={} bytes={}",
        revert.rewritten_chunks,
        revert.sections_encoded,
        revert.section_records_written,
        revert.chunk_payload_bytes
    );
    assert_eq!(counts(&revert), (1, 1, 0));

    // C: every section of one chunk changes.
    for section in &mut live[5].sections {
        *section = crate::ChunkSection::filled(dirt, air);
    }
    let all: Vec<usize> = (0..SECTIONS).collect();
    let work = [Work {
        base: &bases[5],
        effective: &live[5],
        dirty: mask(&all),
    }];
    let c = checkpoint(&store, 5, &work);
    assert_store_equals(&store, &bases, &live);
    eprintln!(
        "SECTION-COUNTERS C full-chunk x1: chunks={} encoded={} records={} bytes={}",
        c.rewritten_chunks, c.sections_encoded, c.section_records_written, c.chunk_payload_bytes
    );
    assert_eq!(counts(&c), (1, SECTIONS as u64, SECTIONS as u64));

    // No-op: a dirty chunk whose content did not actually change.
    let work = [Work {
        base: &bases[5],
        effective: &live[5],
        dirty: mask(&[]),
    }];
    let noop = checkpoint(&store, 6, &work);
    assert_store_equals(&store, &bases, &live);
    eprintln!(
        "SECTION-COUNTERS noop x1: chunks={} encoded={} records={} bytes={}",
        noop.rewritten_chunks,
        noop.sections_encoded,
        noop.section_records_written,
        noop.chunk_payload_bytes
    );
    assert_eq!(
        counts(&noop),
        (0, 0, 0),
        "a dirty chunk without a real change is not rewritten"
    );
    drop(store);
    let reopened = NativeStore::open(&path, [1; 32]).unwrap();
    assert_store_equals(&reopened, &bases, &live);
}

#[test]
fn dirty_section_mask_outside_geometry_is_rejected_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let (blocks, items) = registries();
    let air = block(&blocks, "minecraft:air");
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 0,
                previous_tick: 0,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &globals(0),
            },
            &blocks,
            &items,
        )
        .unwrap();
    let chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, plains());
    let records = globals(1);
    let error = store
        .commit_checkpoint_stream(
            NativeCheckpoint {
                tick: 1,
                previous_tick: 0,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &records,
            },
            |write| {
                write(NativeChunkSnapshot {
                    dimension: 0,
                    base: &chunk,
                    effective: &chunk,
                    dirty_sections: Some(bit(SECTIONS)),
                })
            },
            std::iter::empty::<Result<NativeEntitySnapshot, NativeError>>(),
            &blocks,
            &items,
        )
        .unwrap_err();
    assert!(error.to_string().contains("dirty section outside"));
    assert_eq!(store.read_checkpoint().unwrap().tick, 0);
}
