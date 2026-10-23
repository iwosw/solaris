//! # mc-world
//!
//! Block states, chunk format, world storage.
//!
//! Part of the Solaris engine.

pub mod anvil;
pub mod atomic_file;
pub mod block;
pub mod chunk;
pub mod light;
pub mod native;
pub mod plant_rules_26_1_2;
pub mod section;
pub mod storage;
pub mod structure_starts;
pub mod wire;

pub use block::{Block, BlockRegistry, BlockState, BlockStateId, RegistryError};
pub use chunk::{
    BIOME_DIM, BIOME_VOLUME, BiomeSection, BlockPos, ChestBlockEntity, Chunk, ChunkGenerator,
    ChunkGeometry, ChunkPos, FurnaceBlockEntity, FurnaceSlot, GeneratedEntityMarker,
    GeneratedVillagePiece, GeneratedVillageSite, HEIGHTMAP_BITS, HEIGHTMAP_LEN, Heightmap,
    HopperBlockEntity, LIGHT_LAYER_BYTES, LightSection, MAX_Y, MIN_SECTION_Y, MIN_Y,
    OVERWORLD_GEOMETRY, SECTION_COUNT, ScheduledBlockTick, ScheduledFluidTick, SectionLight,
    chunk_rectangle_for_world_bounds,
};
pub use section::{ChunkSection, PackedBitArray, SECTION_DIM, SECTION_VOLUME};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod inhabited_time_tests;
#[cfg(test)]
mod plant_rules_26_1_2_tests;

#[cfg(test)]
mod structure_starts_tests;
