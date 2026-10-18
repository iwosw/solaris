use mc_data::Identifier;
use mc_nbt::Tag;

use crate::{BlockStateId, Chunk, ChunkPos};

fn chunk(position: ChunkPos) -> Chunk {
    Chunk::empty(
        position,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    )
}

#[test]
fn chunk_inhabited_time_accumulates_without_dirtying() {
    let position = ChunkPos { x: 2, z: -3 };
    let mut chunk = chunk(position);
    chunk.extras.push(("InhabitedTime".into(), Tag::Long(41)));
    chunk.dirty = false;

    chunk.increment_inhabited_time(7);

    assert_eq!(chunk.inhabited_time(), 48);
    assert!(!chunk.dirty);
}

#[test]
fn zero_inhabited_time_delta_is_a_clean_noop() {
    let mut chunk = chunk(ChunkPos { x: 0, z: 0 });
    chunk.dirty = false;

    chunk.increment_inhabited_time(0);

    assert_eq!(chunk.inhabited_time(), 0);
    assert!(!chunk.dirty);
}
