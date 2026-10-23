//! Deterministic Nether caverns, biome landscapes and imported content.
//! Generation is pure in seed/coordinates; native ticking owns all inhabitants.

use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkGenerator, ChunkGeometry, ChunkPos};

mod landscape;
mod ores;
mod structures;
use std::sync::Arc;

pub const NETHER_MIN_Y: i32 = 0;
pub const NETHER_HEIGHT: i32 = 256;
/// Lava sea level (vanilla Nether dimension constant).
pub const NETHER_LAVA_LEVEL: i32 = 32;
/// Top bedrock cap (vanilla Nether dimension constant).
pub const NETHER_CEILING_Y: i32 = 127;
/// Rough underside band under the cap (mixed bedrock/netherrack).
pub const NETHER_CEILING_BAND: i32 = 4;

#[must_use]
pub fn nether_geometry() -> ChunkGeometry {
    ChunkGeometry::new(NETHER_MIN_Y, NETHER_HEIGHT).expect("static nether geometry")
}

/// Resolved content and bounded structure caches, built before generation.
pub struct NetherGenerator {
    seed: i64,
    geometry: ChunkGeometry,
    air: BlockStateId,
    bedrock: BlockStateId,
    netherrack: BlockStateId,
    lava: BlockStateId,
    biomes: [Identifier; 5],
    registry: Arc<BlockRegistry>,
    palette: landscape::Palette,
    ores: Vec<ores::OrePass>,
    structures: Option<structures::NetherStructures>,
}

#[derive(Debug, thiserror::Error)]
pub enum NetherGeneratorError {
    #[error("block registry is missing {0}")]
    MissingRequiredBlock(&'static str),
}

impl NetherGenerator {
    /// Build a generator, failing on missing required vanilla blocks instead
    /// of panicking, so startup can report configuration errors.
    ///
    /// # Errors
    ///
    /// Returns [`NetherGeneratorError::MissingRequiredBlock`] when the registry
    /// lacks a required Nether landscape block.
    pub fn try_new(seed: i64, registry: Arc<BlockRegistry>) -> Result<Self, NetherGeneratorError> {
        let air = try_resolve(registry.as_ref(), "minecraft:air")?;
        let bedrock = try_resolve(registry.as_ref(), "minecraft:bedrock")?;
        let netherrack = try_resolve(registry.as_ref(), "minecraft:netherrack")?;
        let lava_id = Identifier::parse("minecraft:lava").expect("static identifier");
        let lava = registry
            .by_name_and_props(&lava_id, &[("level".to_string(), "0".to_string())])
            .ok_or(NetherGeneratorError::MissingRequiredBlock("minecraft:lava"))?;
        Ok(Self {
            seed,
            geometry: nether_geometry(),
            air,
            bedrock,
            netherrack,
            lava,
            biomes: landscape::BIOMES.map(|name| Identifier::parse(name).expect("static biome")),
            palette: landscape::Palette::load(&registry)?,
            registry,
            ores: Vec::new(),
            structures: None,
        })
    }

    /// Build a generator, panicking on missing required blocks.
    ///
    /// # Panics
    ///
    /// Panics if the registry is missing required vanilla Nether blocks. Use
    /// [`NetherGenerator::try_new`] for fallible startup validation.
    #[must_use]
    pub fn new(seed: i64, registry: Arc<BlockRegistry>) -> Self {
        Self::try_new(seed, registry).unwrap_or_else(|err| panic!("{err}"))
    }

    /// Load the licensed vanilla ore/structure graph through the existing
    /// worldgen data paths. Missing or unsupported content fails startup.
    pub fn load_vanilla_worldgen(
        &mut self,
        root: &std::path::Path,
        tags: Arc<crate::vanilla_features::CacheTags>,
    ) -> Result<(), String> {
        self.load_ores(root)?;
        self.structures = Some(structures::NetherStructures::load(root, self, tags)?);
        Ok(())
    }

    fn ceiling_is_bedrock(&self, wx: i32, y: i32, wz: i32) -> bool {
        if y == NETHER_CEILING_Y {
            return true;
        }
        // Rough underside: deterministic hash picks bedrock teeth.
        let hash = splitmix64(
            (self.seed as u64)
                .wrapping_add(wx as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(wz as u64)
                .wrapping_mul(0xBF58_476D_1CE4_E5B9)
                .wrapping_add(y as u64),
        );
        hash & 0b11 == 0
    }
}

fn try_resolve(
    registry: &BlockRegistry,
    name: &'static str,
) -> Result<BlockStateId, NetherGeneratorError> {
    let id = Identifier::parse(name).expect("static identifier");
    registry
        .block(&id)
        .map(|block| block.default)
        .ok_or(NetherGeneratorError::MissingRequiredBlock(name))
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

impl ChunkGenerator for NetherGenerator {
    fn noise_biome(&self, world_x: i32, _y: i32, world_z: i32) -> Option<Identifier> {
        Some(self.biomes[self.biome_index(world_x, world_z)].clone())
    }

    fn structure_sites_in_region(
        &self,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Vec<mc_world::chunk::GeneratedStructureSite> {
        self.structures
            .as_ref()
            .map_or_else(Vec::new, |structures| structures.sites(self, min, max))
    }

    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk =
            Chunk::empty_with_geometry(pos, self.air, self.biomes[0].clone(), self.geometry);
        self.fill_landscape(&mut chunk);
        self.apply_ores(&mut chunk);
        self.decorate(&mut chunk);
        if let Some(structures) = &self.structures {
            structures.write(self, &mut chunk);
        }
        chunk.status = "minecraft:full".into();
        chunk.mark_dirty();
        chunk
    }
}

#[cfg(test)]
#[path = "nether/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "nether/landscape_tests.rs"]
mod landscape_tests;

#[cfg(test)]
#[path = "nether/content_tests.rs"]
mod content_tests;
