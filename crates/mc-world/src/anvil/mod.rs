//! Anvil region-file codec: read and write `.mca` files into our
//! [`crate::chunk::Chunk`] type.
//!
//! Submodules:
//!
//! - [`region`] — sector layout, header parsing, compression. Returns
//!   raw uncompressed NBT bytes per chunk slot.
//! - [`chunk_nbt`] — translation between the parsed NBT compound and
//!   our [`Chunk`] struct.
//!
//! The two layers are split because the region binary is independent
//! of the chunk schema (tools or different worldgen targets could
//! reuse it).

pub mod chunk_nbt;
mod copper_chest_nbt;
pub mod entity_regions;
pub mod region;
pub mod upgrade;

pub use chunk_nbt::{
    ChunkNbtError, TARGET_DATA_VERSION, chunk_from_nbt, chunk_from_nbt_with_items,
    chunk_from_nbt_with_items_at_position_at_tick,
    chunk_from_payload_with_items_at_position_at_tick, chunk_to_nbt, chunk_to_payload,
    chunk_to_payload_with_items, chunk_to_payload_with_items_at_tick,
    chunk_to_payload_with_items_at_tick_for_position, repair_chunk_nbt_position,
};
pub use entity_regions::{
    EntityChunk, EntityRegionError, entity_chunk_from_nbt, entity_region_file_name,
    read_entity_regions, validate_shared_fields, write_entity_regions,
};
pub use region::{
    CHUNKS_PER_REGION_AXIS, ChunkPayload, CompressionType, RawChunkRecord, RegionChunkOutput,
    RegionError, read_chunk, read_region, read_region_raw, visit_region, write_region,
    write_region_create_new, write_region_create_new_mixed,
};
