//! Seed-100 production-content village regressions: the generated village must
//! be playable (no spring above the streets, no house sealed by hillside soil).
use super::*;
use crate::vanilla_features::{BlockSemantics, CacheTags};
use crate::village::{VillageDecor, load_village_closure};

fn seed100_generator() -> Option<(TerrainGenerator, Arc<BlockRegistry>, Arc<CacheTags>)> {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    if !root.join("data/minecraft/worldgen").is_dir() {
        return None;
    }
    let seed = 100;
    let blocks = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let vanilla = mc_data::load(&root).unwrap();
    let tags = mc_data::tags::load(&root, &vanilla).unwrap();
    let tags = Arc::new(CacheTags::new(
        Arc::clone(&blocks),
        Arc::new(vanilla),
        Arc::new(tags),
    ));
    let closure = Arc::new(
        load_village_closure(
            &root,
            &Identifier::parse("minecraft:villages").unwrap(),
            &blocks,
        )
        .unwrap(),
    );
    let decor = Arc::new(
        VillageDecor::compile(
            &root,
            &closure,
            &BlockSemantics::new(&blocks, tags.as_ref()),
        )
        .unwrap(),
    );
    let source = Arc::new(
        VillagePlanSource::new(
            closure,
            decor,
            seed,
            Arc::clone(&blocks),
            tags.clone(),
            tags.clone(),
        )
        .unwrap(),
    );
    let mut generator = TerrainGenerator::new(seed, Arc::clone(&blocks));
    generator.load_vanilla_worldgen(&root).unwrap();
    Some((generator.with_village_plans(source), blocks, tags))
}

/// Every chunk the seed-100 village (start chunk (7,0)) reaches.
const VILLAGE_CHUNKS_X: std::ops::RangeInclusive<i32> = 2..=13;
const VILLAGE_CHUNKS_Z: std::ops::RangeInclusive<i32> = -5..=5;

struct VillageWorld {
    chunks: std::collections::BTreeMap<(i32, i32), Chunk>,
    blocks: Arc<BlockRegistry>,
    tags: Arc<CacheTags>,
}

impl VillageWorld {
    fn generate(
        generator: &TerrainGenerator,
        blocks: Arc<BlockRegistry>,
        tags: Arc<CacheTags>,
    ) -> Self {
        let mut chunks = std::collections::BTreeMap::new();
        for cz in VILLAGE_CHUNKS_Z {
            for cx in VILLAGE_CHUNKS_X {
                chunks.insert((cx, cz), generator.generate(ChunkPos { x: cx, z: cz }));
            }
        }
        Self {
            chunks,
            blocks,
            tags,
        }
    }

    /// `None` outside the generated window.
    fn state(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        self.chunks
            .get(&(x.div_euclid(16), z.div_euclid(16)))?
            .get_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8)
    }

    fn name(&self, id: BlockStateId) -> &str {
        self.blocks.by_id(id).unwrap().block.id.path()
    }

    fn is_solid(&self, id: BlockStateId) -> bool {
        BlockSemantics::new(&self.blocks, self.tags.as_ref()).is_solid(id)
    }
}

#[test]
fn seed100_village_has_no_exposed_water_source() {
    let Some((generator, blocks, tags)) = seed100_generator() else {
        println!("SKIP seed100_village_has_no_exposed_water_source: no content cache");
        return;
    };
    let world = VillageWorld::generate(&generator, blocks, tags);
    let mut exposed = Vec::new();
    let mut sources = 0;
    for (&(cx, cz), chunk) in &world.chunks {
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                // Above the sea: a source there is a spring over dry land.
                for y in SEA_LEVEL..SEA_LEVEL + 60 {
                    let Some(id) = chunk.get_block(lx, y, lz) else {
                        continue;
                    };
                    let state = world.blocks.by_id(id).unwrap();
                    if state.block.id.path() != "water" {
                        continue;
                    }
                    sources += 1;
                    let (x, z) = (cx * 16 + i32::from(lx), cz * 16 + i32::from(lz));
                    let open = [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1), (0, -1, 0)]
                        .into_iter()
                        .any(|(dx, dy, dz)| {
                            // The window edge is not a verdict: skip unknown cells.
                            world
                                .state(x + dx, y + dy, z + dz)
                                .is_some_and(|n| !world.is_solid(n) && world.name(n) != "water")
                        });
                    if open {
                        exposed.push((x, y, z));
                    }
                }
            }
        }
    }
    println!("water sources above the sea in the village window: {sources}");
    assert!(
        sources > 0,
        "the fixture village keeps its wells and farm plots"
    );
    assert!(
        exposed.is_empty(),
        "water sources open to the side or below would flow over the village: {exposed:?}"
    );
}

#[test]
fn seed100_village_doors_open_onto_walkable_interiors() {
    let Some((generator, blocks, tags)) = seed100_generator() else {
        println!("SKIP seed100_village_doors_open_onto_walkable_interiors: no content cache");
        return;
    };
    let world = VillageWorld::generate(&generator, blocks, tags);
    let mut doors = 0;
    let mut blocked = Vec::new();
    for (&(cx, cz), chunk) in &world.chunks {
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                for y in SEA_LEVEL - 4..SEA_LEVEL + 70 {
                    let Some(id) = chunk.get_block(lx, y, lz) else {
                        continue;
                    };
                    let state = world.blocks.by_id(id).unwrap();
                    if !state.block.id.path().ends_with("_door")
                        || !state
                            .properties
                            .iter()
                            .any(|(name, value)| name == "half" && value == "lower")
                    {
                        continue;
                    }
                    doors += 1;
                    let (x, z) = (cx * 16 + i32::from(lx), cz * 16 + i32::from(lz));
                    let property = |key: &str| {
                        state
                            .properties
                            .iter()
                            .find(|(name, _)| name == key)
                            .map(|(_, value)| value.as_str())
                            .unwrap()
                    };
                    let facing = property("facing");
                    // A closed door's panel spans the wall, so people walk along
                    // its facing axis; an open one has swung 90 degrees onto the
                    // wall, so the way through is the other axis.
                    let along_x = matches!(facing, "east" | "west") != (property("open") == "true");
                    let (dx, dz) = if along_x { (1, 0) } else { (0, 1) };
                    // Both sides of the door: the way in and the way out. Each
                    // needs a standing cell and the cell above it free of solids.
                    for side in [1, -1] {
                        for dy in 0..2 {
                            let probe = (x + dx * side, y + dy, z + dz * side);
                            if let Some(n) = world.state(probe.0, probe.1, probe.2)
                                && world.is_solid(n)
                            {
                                blocked.push((x, y, z, facing, probe, world.name(n).to_owned()));
                            }
                        }
                    }
                }
            }
        }
    }
    println!("doors checked: {doors}");
    assert!(doors > 0, "the fixture village has houses with doors");
    assert!(
        blocked.is_empty(),
        "a door with a solid block beside it traps whoever stands inside: {blocked:?}"
    );
}
