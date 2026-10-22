//! Checkpoint capture carries the exact edited block sections: metadata-only
//! changes encode none, one edit encodes one section, and the written store
//! reopens to the identical world.

use rustc_hash::FxHashMap as HashMap;

use super::world_tests::{test_world, write_capture_io};
use crate::world::{ChunkWorld, chunks_in_radius};
use mc_world::{ChunkPos, OVERWORLD_GEOMETRY};

const SECTIONS: usize = 24;

fn wants(positions: &[ChunkPos]) -> HashMap<ChunkPos, i32> {
    positions.iter().map(|pos| (*pos, 0)).collect()
}

/// One checkpoint: capture, commit, ACK. Returns (sections evaluated, records written).
fn checkpoint(world: &mut ChunkWorld) -> (u64, u64) {
    let capture = world.capture_dirty(&Default::default());
    let (written, io) = write_capture_io(&capture).unwrap();
    world.mark_flushed(&written);
    (io.sections_encoded, io.section_records_written)
}

fn every_block(world: &ChunkWorld, positions: &[ChunkPos]) -> Vec<u32> {
    let mut states = Vec::new();
    for pos in positions {
        for y in OVERWORLD_GEOMETRY.min_y()..OVERWORLD_GEOMETRY.max_y() {
            for z in 0..16 {
                for x in 0..16 {
                    states.push(world.block_at(pos.x * 16 + x, y, pos.z * 16 + z).unwrap());
                }
            }
        }
    }
    states
}

#[test]
fn checkpoint_encodes_only_edited_sections_and_reopens_identically() {
    let directory = tempfile::tempdir().unwrap();
    let mut world = test_world(Some(directory.path().to_path_buf()));
    let center = ChunkPos { x: 0, z: 0 };
    let positions = chunks_in_radius(center, 1);
    world.ensure_blocking(&wants(&positions));
    let glass = world
        .source
        .registry
        .block(&mc_data::Identifier::parse("minecraft:glass").unwrap())
        .unwrap()
        .default
        .0;
    let min_y = OVERWORLD_GEOMETRY.min_y();
    // Settle: freshly admitted chunks carry light and generated bases.
    checkpoint(&mut world);
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );

    // A: metadata-only change (a block-entity-style dirty mark): no section work.
    world.mark_block_entity_dirty(mc_world::BlockPos { x: 1, y: 70, z: 1 });
    assert_eq!(checkpoint(&mut world), (0, 0), "metadata-only");

    // B: one block edit in one section: exactly that section is examined.
    world.set_block(4, 100, 4, glass);
    assert_eq!(checkpoint(&mut world), (1, 1), "one edited section");

    // D: an edit lands between capture and ACK. The stale ACK keeps the chunk
    // dirty and the next capture still carries the section the first one wrote.
    world.set_block(5, 100, 5, glass);
    let first = world.capture_dirty(&Default::default());
    world.set_block(6, min_y + 16 * 7 + 2, 6, glass);
    world.fold_overlay();
    let (written, io) = write_capture_io(&first).unwrap();
    assert_eq!(io.sections_encoded, 1);
    world.mark_flushed(&written);
    drop(first);
    let (encoded, _) = checkpoint(&mut world);
    assert_eq!(
        encoded, 2,
        "mask keeps unacknowledged bits plus the new edit"
    );
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );

    // C: every section of one chunk edited.
    for section in 0..SECTIONS {
        world.set_block(3, min_y + section as i32 * 16 + 3, 3, glass);
    }
    assert_eq!(
        checkpoint(&mut world),
        (SECTIONS as u64, SECTIONS as u64),
        "full chunk"
    );

    let expected = every_block(&world, &positions);
    drop(world);
    let mut reopened = test_world(Some(directory.path().to_path_buf()));
    reopened.ensure_blocking(&wants(&positions));
    assert!(
        expected == every_block(&reopened, &positions),
        "reopened world differs from the checkpointed one"
    );
}
