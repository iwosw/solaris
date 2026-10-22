//! Chunk packet assembly: snapshot chunk -> `LevelChunkWithLight`.
//!
//! Thin lift over [`mc_world::wire`]: chunk-data blob, client heightmaps,
//! block-entity sidecars (resolved through the block-entity-type registry),
//! and authoritative baked light from the core snapshot.

use mc_data::{Identifier, Registry};
use mc_protocol::packets::play::{BlockEntityInfo, ChunkHeightmap, LevelChunkWithLight};
use mc_world::BlockRegistry;
use mc_world::chunk::{Chunk, ChunkPos};

/// Registries needed to encode a resident chunk.
pub struct EncodeInputs<'a> {
    /// `worldgen/biome` registry.
    pub biomes: &'a Registry,
    /// Block table.
    pub blocks: &'a BlockRegistry,
    /// Item table for block-entity encoding.
    pub items: &'a mc_data::items::ItemRegistry,
    /// Block-entity type table.
    pub block_entity_types: &'a mc_data::block_entity_types::BlockEntityTypeRegistry,
}

/// Assemble one full chunk packet from a resident snapshot chunk.
///
/// Records with unknown block-entity type names are skipped (never fail a
/// chunk send on one bad sidecar).
pub fn encode_chunk_packet(
    pos: ChunkPos,
    chunk: &Chunk,
    inputs: EncodeInputs<'_>,
) -> Result<LevelChunkWithLight, mc_world::wire::WireError> {
    encode_chunk_packet_with_sections(pos, chunk, inputs, &[])
}

/// [`encode_chunk_packet`] with some block sections replaced on the wire
/// (anti-xray's rock stand-ins; see [`mc_world::wire::encode_chunk_data_with_sections`]).
pub fn encode_chunk_packet_with_sections(
    pos: ChunkPos,
    chunk: &Chunk,
    inputs: EncodeInputs<'_>,
    sections: &[Option<mc_world::ChunkSection>],
) -> Result<LevelChunkWithLight, mc_world::wire::WireError> {
    let EncodeInputs {
        biomes,
        blocks,
        items,
        block_entity_types,
    } = inputs;
    let data = mc_world::wire::encode_chunk_data_with_sections(chunk, biomes, blocks, sections)?;
    let heightmaps: Vec<ChunkHeightmap> = mc_world::wire::client_heightmaps(chunk)
        .into_iter()
        .map(|h| ChunkHeightmap {
            type_id: h.type_id,
            data: h.data,
        })
        .collect();
    let block_entities: Vec<BlockEntityInfo> =
        mc_world::wire::client_block_entities(chunk, blocks, items)
            .into_iter()
            .filter_map(|record| {
                let type_id = block_entity_types.id_of(&record.type_name)?;
                let packed_xz =
                    ((record.pos.x.rem_euclid(16) as u8) << 4) | record.pos.z.rem_euclid(16) as u8;
                Some(BlockEntityInfo {
                    packed_xz,
                    y: record.pos.y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
                    type_id: i32::try_from(type_id).ok()?,
                    nbt: record.nbt,
                })
            })
            .collect();
    let light = crate::light::chunk_light(chunk);
    Ok(LevelChunkWithLight {
        chunk_x: pos.x,
        chunk_z: pos.z,
        heightmaps,
        data,
        block_entities,
        light,
    })
}

/// Biome registry probe used by tests: a `worldgen/biome` registry over the
/// given names in order.
#[must_use]
pub fn test_biome_registry(names: &[&str]) -> Registry {
    Registry {
        id: Identifier::parse("minecraft:worldgen/biome").expect("static id parses"),
        entries: names
            .iter()
            .map(|n| Identifier::parse(*n).expect("static id parses"))
            .collect(),
    }
}

#[cfg(test)]
#[path = "chunks_tests.rs"]
mod chunks_tests;
