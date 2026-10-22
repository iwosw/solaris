//! Far-terrain LOD tiles for native clients, beyond the view distance.
//!
//! A tile is 16x16 point samples spaced `1 << level` blocks, whose first
//! sample is the column at block `(tile_x, tile_z) * (16 << level)`. A stored
//! chunk answers with its top non-air block as of the last stored image
//! (player edits after it are not shown). A column that was never generated
//! answers with the generator's cheap router surface, which is what generation
//! will later produce there. Read-only and off the tick thread: the tile
//! borrows the chunk workers' store and generator for each request and does not
//! keep the native database open after the world owner stops.

use std::sync::Arc;

use mc_data::Identifier;
use mc_world::{BlockStateId, Chunk, ChunkPos};

use crate::CoreError;
use crate::chunk_loader::ChunkSource;

/// Samples per tile edge.
pub const TILE_EDGE: usize = 16;
/// Coarsest level: 64-block sample spacing.
pub const MAX_LEVEL: u8 = 6;

/// Where a sample came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LodOrigin {
    /// Not answered: a store read failed, or the client asked for stored
    /// columns only and this one was never generated.
    Missing,
    /// The column's top non-air block in the stored chunk.
    Stored,
    /// Never generated: the generator's cave-free router surface.
    Router,
}

/// One column of a tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LodSample {
    pub origin: LodOrigin,
    /// Y of the top block (stored) or top solid block (router).
    pub height: i32,
    /// Stored top block; the router does not decide surface blocks.
    pub block: Option<BlockStateId>,
    pub biome: Option<Identifier>,
}

impl LodSample {
    const MISSING: Self = Self {
        origin: LodOrigin::Missing,
        height: 0,
        block: None,
        biome: None,
    };
}

pub(crate) struct LodSource {
    source: std::sync::Weak<ChunkSource>,
    air: Vec<BlockStateId>,
    program: std::sync::OnceLock<Option<Arc<Vec<u8>>>>,
}

impl std::fmt::Debug for LodSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LodSource")
            .field(
                "dimension",
                &self.source.upgrade().map(|source| source.dimension),
            )
            .finish_non_exhaustive()
    }
}

impl LodSource {
    pub(crate) fn new(source: Arc<ChunkSource>) -> Self {
        let air = source
            .registry
            .states()
            .filter(|state| matches!(state.block.id.path(), "air" | "cave_air" | "void_air"))
            .map(|state| state.id)
            .collect();
        Self {
            source: Arc::downgrade(&source),
            air,
            program: std::sync::OnceLock::new(),
        }
    }

    /// The generator's far-terrain program, zlib-compressed and built once.
    /// `None` when the generator has none or the export failed (logged).
    pub(crate) fn program(&self) -> Option<Arc<Vec<u8>>> {
        let source = self.source.upgrade()?;
        self.program
            .get_or_init(|| {
                let program = match source.generator.lod_program() {
                    Ok(program) => program?,
                    Err(error) => {
                        tracing::warn!(%error, "far-terrain program export failed");
                        return None;
                    }
                };
                let mut encoder = flate2::write::ZlibEncoder::new(
                    Vec::with_capacity(program.len() / 4),
                    flate2::Compression::best(),
                );
                std::io::Write::write_all(&mut encoder, &program).ok()?;
                encoder.finish().ok().map(Arc::new)
            })
            .clone()
    }

    pub(crate) fn tile(
        &self,
        tile_x: i32,
        tile_z: i32,
        level: u8,
        stored_only: bool,
    ) -> Result<Vec<LodSample>, CoreError> {
        let source = self
            .source
            .upgrade()
            .ok_or_else(|| CoreError::Storage("LOD source stopped".into()))?;
        if level > MAX_LEVEL {
            return Err(CoreError::Storage(format!(
                "lod level {level} > {MAX_LEVEL}"
            )));
        }
        let spacing = 1i64 << level;
        let edge = TILE_EDGE as i64;
        let origin = [tile_x, tile_z].map(|t| i64::from(t) * edge * spacing);
        let column = |index: usize, axis: usize| -> Result<i32, CoreError> {
            let offset = if axis == 0 {
                index % TILE_EDGE
            } else {
                index / TILE_EDGE
            };
            i32::try_from(origin[axis] + offset as i64 * spacing)
                .map_err(|_| CoreError::Storage("lod tile outside the world".into()))
        };
        // Visit each chunk once, holding at most one decoded chunk.
        let mut order = Vec::with_capacity(TILE_EDGE * TILE_EDGE);
        for index in 0..TILE_EDGE * TILE_EDGE {
            let (x, z) = (column(index, 0)?, column(index, 1)?);
            let pos = ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            };
            order.push((pos, index, x, z));
        }
        order.sort_unstable_by_key(|(pos, index, ..)| (pos.x, pos.z, *index));
        let mut samples = vec![LodSample::MISSING; TILE_EDGE * TILE_EDGE];
        let mut group = 0;
        while group < order.len() {
            let pos = order[group].0;
            let end = group
                + order[group..]
                    .iter()
                    .take_while(|entry| entry.0 == pos)
                    .count();
            match source.load_native(pos) {
                Ok(Some(chunk)) => {
                    for &(_, index, x, z) in &order[group..end] {
                        samples[index] = self.stored(&chunk, x, z);
                    }
                }
                Ok(None) if !stored_only => {
                    for &(_, index, x, z) in &order[group..end] {
                        if let Some((height, biome)) = source.generator.lod_column(x, z) {
                            samples[index] = LodSample {
                                origin: LodOrigin::Router,
                                height,
                                block: None,
                                biome: Some(biome),
                            };
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "lod tile store read failed"),
            }
            group = end;
        }
        Ok(samples)
    }

    fn stored(&self, chunk: &Chunk, x: i32, z: i32) -> LodSample {
        let geometry = chunk.geometry();
        let (lx, lz) = (x.rem_euclid(16) as u8, z.rem_euclid(16) as u8);
        let top = (geometry.min_y()..geometry.max_y()).rev().find_map(|y| {
            chunk
                .get_block(lx, y, lz)
                .filter(|state| !self.air.contains(state))
                .map(|state| (y, state))
        });
        let Some((height, block)) = top else {
            return LodSample {
                origin: LodOrigin::Stored,
                height: geometry.min_y() - 1,
                block: None,
                biome: None,
            };
        };
        let biome = geometry
            .world_y_to_section(height)
            .and_then(|(section, y)| {
                chunk
                    .biomes
                    .get(section)
                    .map(|biomes| biomes.get(lx / 4, y / 4, lz / 4).clone())
            });
        LodSample {
            origin: LodOrigin::Stored,
            height,
            block: Some(block),
            biome,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_world::ChunkGenerator;

    /// A stored chunk answers with its current top block, edits included; the
    /// never-generated rest of the tile with the router, or nothing when the
    /// client asked for stored columns only.
    #[test]
    fn stored_chunks_answer_with_top_blocks_and_the_rest_with_the_router() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
        let store = crate::native_checkpoint::tests::fixture_store(&config).expect("store");
        let registry = crate::registry::CoreRegistry::load(None).expect("registry");
        let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
            42,
            Arc::clone(&registry.blocks),
        ));
        let mut chunk = generator.generate(ChunkPos { x: 0, z: 0 });
        let stone = registry
            .blocks
            .block(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap()
            .default;
        // A player's pillar at column (2, 4): 240 is far above any terrain.
        chunk.set_block(2, 240, 4, stone);
        store
            .fill_generated(0, &[chunk], &registry.blocks, &registry.items)
            .expect("stored chunk");
        let chunks = Arc::new(ChunkSource {
            native: Some(store),
            dimension: 0,
            generator: generator.clone(),
            registry: Arc::clone(&registry.blocks),
            items: Arc::clone(&registry.items),
            chest_loot: Arc::clone(&registry.chest_loot),
            light: Arc::clone(&registry.light),
            geometry: crate::dimensions::Dimension::Overworld.geometry(),
        });
        let source = LodSource::new(Arc::clone(&chunks));
        // Level 1 tile (0, 0): 2-block spacing over chunks (0..2, 0..2).
        let tile = source.tile(0, 0, 1, false).expect("tile");
        let pillar = &tile[2 * TILE_EDGE + 1];
        assert_eq!(pillar.origin, LodOrigin::Stored);
        assert_eq!((pillar.height, pillar.block), (240, Some(stone)));
        assert!(pillar.biome.is_some());
        for (index, sample) in tile.iter().enumerate() {
            let (x, z) = (index % TILE_EDGE * 2, index / TILE_EDGE * 2);
            if x < 16 && z < 16 {
                assert_eq!(sample.origin, LodOrigin::Stored, "column ({x}, {z})");
            } else {
                let (height, biome) = generator.lod_column(x as i32, z as i32);
                assert_eq!(sample.origin, LodOrigin::Router, "column ({x}, {z})");
                assert_eq!(
                    (sample.height, sample.biome.as_ref()),
                    (height, Some(&biome))
                );
                assert_eq!(sample.block, None);
            }
        }
        let stored_only = source.tile(0, 0, 1, true).expect("tile");
        assert_eq!(stored_only[0].origin, LodOrigin::Stored);
        assert_eq!(stored_only[TILE_EDGE - 1].origin, LodOrigin::Missing);
        // Coarse levels sample one column per chunk at most, far from the origin too.
        let far = source.tile(-3, 7, MAX_LEVEL, false).expect("tile");
        assert!(far.iter().all(|sample| sample.origin == LodOrigin::Router));
        assert!(source.tile(0, 0, MAX_LEVEL + 1, false).is_err());
        drop(chunks);
        assert!(
            source
                .tile(0, 0, 0, false)
                .unwrap_err()
                .to_string()
                .contains("LOD source stopped")
        );
        assert!(source.program().is_none());
    }
}
