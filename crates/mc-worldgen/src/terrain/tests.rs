use super::biome_routing::BEACH_HEIGHT_ABOVE_SEA;
use super::*;
use mc_data::worldgen_ores::{HeightAnchor, OreFeature, OrePlacementCount, OreTarget};
use mc_world::chunk::{MAX_Y, MIN_Y, OVERWORLD_GEOMETRY};
use std::collections::BTreeMap;

#[test]
fn ore_boundary_culling_preserves_cells_and_exposure() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut rule = g.ores.rules()[0].clone();
    rule.y = YRange { min: -64, max: 319 };
    rule.discard_chance_on_air_exposure = 0.5;
    let mut placed = 0;
    for pos in [ChunkPos { x: -1, z: -1 }, ChunkPos { x: 0, z: 0 }] {
        let columns =
            std::array::from_fn(|i| g.plan_column(pos, (i % 16) as u8, (i / 16) as u8, None));
        let mut actual =
            Chunk::empty_with_geometry(pos, g.air, g.biomes.default.clone(), g.geometry);
        for y in -40..=-24 {
            for z in 0..16 {
                for x in 0..16 {
                    actual.set_block(x, y, z, g.stone).unwrap();
                }
            }
        }
        let mut expected = actual.clone();
        let mut actual_heights = HeightContext::for_chunk(&g, pos, &[], None);
        let mut expected_heights = HeightContext::for_chunk(&g, pos, &[], None);
        let mut actual_cache = g.ore_column_cache(&actual, &columns, &mut actual_heights);
        let mut expected_cache = g.ore_column_cache(&expected, &columns, &mut expected_heights);
        for dx in [-1, 0, 15, 16] {
            for dz in [-1, 0, 15, 16] {
                let anchor = [pos.x * 16 + dx, -32, pos.z * 16 + dz];
                for hash in 0..32 {
                    g.place_ore_vein(&mut actual, &mut actual_cache, &rule, anchor, hash, 17);
                    // Original ordering: query every offset, then reject
                    // out-of-chunk writes in place_ore_cell.
                    let mut offsets = [[0; 3]; MAX_ORE_VEIN_SIZE];
                    let count = ore_veins::ore_offsets(&mut offsets, hash, 17);
                    for (i, &offset) in offsets[..count].iter().enumerate() {
                        if g.ore_cell_can_generate(
                            &mut expected_cache,
                            &rule,
                            anchor,
                            offset,
                            hash ^ i as u64,
                        ) {
                            g.place_ore_cell(&mut expected, &rule, anchor, offset);
                        }
                    }
                }
            }
        }
        for y in -40..=-24 {
            for z in 0..16 {
                for x in 0..16 {
                    let block = actual.get_block(x, y, z);
                    assert_eq!(block, expected.get_block(x, y, z), "{pos:?} ({x},{y},{z})");
                    placed += usize::from(block == Some(rule.normal));
                }
            }
        }
    }
    assert!(
        placed > 0,
        "boundary comparison must exercise actual ore writes"
    );
}

#[test]
fn diagnostic_cache_evicts_one_column_without_clearing_a_full_shard() {
    let cache = DiagnosticCache::default();
    let coordinate = |index: usize| (index as i32 * DIAGNOSTIC_CACHE_SHARDS as i32, 0);
    let sample = |index| TerrainDiagnosticSample {
        surface_y: index,
        biome: Identifier::parse("minecraft:plains").unwrap(),
        vegetation_density: Some(0.25),
    };
    for index in 0..DIAGNOSTIC_CACHE_ENTRIES_PER_SHARD {
        let (x, z) = coordinate(index);
        assert_eq!(DiagnosticCache::shard_index(x, z), 0);
        assert_eq!(
            cache.insert(x, z, sample(index as i32)),
            sample(index as i32)
        );
    }
    assert_eq!(cache.insert(0, 0, sample(-1)), sample(0));
    let (x, z) = coordinate(DIAGNOSTIC_CACHE_ENTRIES_PER_SHARD);
    cache.insert(x, z, sample(-1));
    assert!(
        cache.get(0, 0).is_none(),
        "only the oldest column is evicted"
    );
    for index in 1..DIAGNOSTIC_CACHE_ENTRIES_PER_SHARD {
        let (x, z) = coordinate(index);
        assert_eq!(cache.get(x, z), Some(sample(index as i32)));
    }
    assert_eq!(cache.get(x, z), Some(sample(-1)));
}

impl TerrainGenerator {
    pub(super) fn biome_for(&self, world_x: i32, world_z: i32, height: i32) -> Identifier {
        let sample = self.density_router().sample(world_x, world_z);
        self.vanilla_biome_for(world_x, world_z, height, sample)
    }
}

pub(in crate::terrain) fn tiny_registry() -> Arc<BlockRegistry> {
    use mc_data::blocks::{BlockReport, BlockStateReport};
    let report = vec![
        BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:bedrock").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 1,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 2,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:dirt").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 3,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:grass_block").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 4,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:sand").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 14,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:water").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 5,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lava").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 6,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 7,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:coal_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 8,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:iron_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 9,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:copper_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 10,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_coal_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 11,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_iron_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 12,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_copper_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 13,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:gold_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 15,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:redstone_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 16,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:diamond_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 17,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lapis_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 18,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:emerald_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 19,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_gold_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 20,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_redstone_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 21,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_diamond_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 22,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_lapis_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 23,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_emerald_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 24,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:stone_bricks").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 25,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_log").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 26,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_leaves").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 27,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:short_grass").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 28,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:dandelion").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 29,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:poppy").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 30,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:pumpkin").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 31,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:sugar_cane").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 32,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:cactus").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 33,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:red_sand").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 34,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:gravel").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 35,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:podzol").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 36,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:snow_block").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 37,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:birch_log").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 38,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:birch_leaves").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 39,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:seagrass").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 40,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:kelp").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 41,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:kelp_plant").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 42,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:acacia_log").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 43,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:acacia_leaves").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 44,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
    ];
    Arc::new(BlockRegistry::from_report(&report).unwrap())
}

fn registry_without_block(missing: &str) -> Arc<BlockRegistry> {
    use mc_data::blocks::{BlockReport, BlockStateReport};
    let names = [
        "minecraft:air",
        "minecraft:bedrock",
        "minecraft:stone",
        "minecraft:dirt",
        "minecraft:grass_block",
        "minecraft:iron_ore",
    ];
    let report = names
        .into_iter()
        .filter(|name| *name != missing)
        .enumerate()
        .map(|(id, name)| BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: u32::try_from(id).unwrap(),
                default: true,
                properties: BTreeMap::new(),
            }],
        })
        .collect::<Vec<_>>();
    Arc::new(BlockRegistry::from_report(&report).unwrap())
}

fn required_only_registry() -> Arc<BlockRegistry> {
    registry_without_block("minecraft:not_present")
}

fn land_biome_family(g: &TerrainGenerator, biome: &Identifier) -> Option<&'static str> {
    if g.biomes.is_surface_water(biome)
        || g.biomes.is_beach_or_shore(biome)
        || g.biomes.mountain.contains(biome)
        || g.biomes.cave.contains(biome)
    {
        None
    } else if g.biomes.swamp.contains(biome) {
        Some("swamp")
    } else if g.biomes.cold.contains(biome) {
        Some("cold")
    } else if g.biomes.hot_dry.contains(biome) {
        Some("hot_dry")
    } else if g.biomes.jungle.contains(biome) {
        Some("jungle")
    } else if g.biomes.temperate_forest.contains(biome) {
        Some("temperate_forest")
    } else if g.biomes.grassland.contains(biome) {
        Some("grassland")
    } else {
        None
    }
}

#[test]
fn generated_leaf_distances_follow_nearest_logs_and_preserve_decay() {
    use mc_data::blocks::{BlockReport, BlockStateReport};

    let mut report = [
        "minecraft:air",
        "minecraft:bedrock",
        "minecraft:stone",
        "minecraft:dirt",
        "minecraft:grass_block",
        "minecraft:iron_ore",
        "minecraft:oak_log",
    ]
    .into_iter()
    .enumerate()
    .map(|(id, name)| BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: id as u32,
            default: true,
            properties: BTreeMap::new(),
        }],
    })
    .collect::<Vec<_>>();
    report.push(BlockReport {
        id: Identifier::parse("minecraft:oak_leaves").unwrap(),
        properties: BTreeMap::from([
            (
                "distance".to_string(),
                (1..=7).map(|distance| distance.to_string()).collect(),
            ),
            ("persistent".to_string(), vec!["false".to_string()]),
            ("waterlogged".to_string(), vec!["false".to_string()]),
        ]),
        states: (1..=7)
            .map(|distance| BlockStateReport {
                id: 6 + distance,
                default: distance == 7,
                properties: BTreeMap::from([
                    ("distance".to_string(), distance.to_string()),
                    ("persistent".to_string(), "false".to_string()),
                    ("waterlogged".to_string(), "false".to_string()),
                ]),
            })
            .collect(),
    });
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    let generator = TerrainGenerator::try_with_rules(
        42,
        registry.clone(),
        BiomeRules::vanilla_overworld(),
        OreRules::new(Vec::new()).unwrap(),
    )
    .unwrap();
    let mut chunk = generator.generate(ChunkPos { x: 0, z: 0 });
    let y = generator.geometry.max_y() - 4;
    let log = optional_block(&registry, "minecraft:oak_log").unwrap();
    let initial_leaf = optional_generated_leaves(&registry, "minecraft:oak_leaves").unwrap();
    chunk.set_block(0, y, 0, log);
    chunk.set_block(1, y, 8, log);
    let mut leaves = Vec::new();
    for offset in (1..=8).rev() {
        for (x, z) in [(offset, 0), (0, offset)] {
            chunk.set_block(x, y, z, initial_leaf);
            leaves.push((x, y, z));
        }
    }
    chunk.set_block(15, y, 15, initial_leaf);
    leaves.push((15, y, 15));
    generator.initialize_leaf_distances(&mut chunk, &mut leaves);

    for offset in 1..=8 {
        for (x, z, expected) in [
            (offset, 0, offset.min(7)),
            (0, offset, offset.min(9 - offset)),
        ] {
            let state = chunk.get_block(x, y, z).unwrap();
            assert_eq!(
                mc_world::plant_rules_26_1_2::leaf_distance_from_state(&registry, state),
                expected,
                "leaf at ({x}, {y}, {z}) must use its nearest supporting log"
            );
            assert!(
                registry
                    .by_id(state)
                    .unwrap()
                    .properties
                    .iter()
                    .any(|(key, value)| key == "persistent" && value == "false")
            );
        }
    }
    assert_eq!(
        mc_world::plant_rules_26_1_2::leaf_distance_from_state(
            &registry,
            chunk.get_block(15, y, 15).unwrap()
        ),
        7,
        "an isolated leaf remains eligible for natural decay"
    );
}

#[test]
fn try_with_rules_reports_missing_required_block() {
    let err = match TerrainGenerator::try_with_rules(
        42,
        registry_without_block("minecraft:grass_block"),
        BiomeRules::vanilla_overworld(),
        OreRules::new(Vec::new()).expect("empty ore rules fit the admission budget"),
    ) {
        Ok(_) => panic!("missing required terrain block must fail"),
        Err(err) => err,
    };

    assert_eq!(
        err,
        TerrainGeneratorError::MissingRequiredBlock {
            name: "minecraft:grass_block"
        }
    );
    assert_eq!(
        err.to_string(),
        "block registry missing required terrain block minecraft:grass_block"
    );
}

#[test]
fn standalone_realistic_profile_rejects_missing_normal_or_deepslate_ore() {
    for missing in ["minecraft:coal_ore", "minecraft:deepslate_coal_ore"] {
        let mut report = mc_data::blocks::solaris_required_blocks_report()
            .into_iter()
            .filter(|block| block.id.as_str() != missing)
            .collect::<Vec<_>>();
        let mut next = 0;
        for block in &mut report {
            for state in &mut block.states {
                state.id = next;
                next += 1;
            }
        }
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let generator = TerrainGenerator::new(42, Arc::clone(&registry));
        let Err(TerrainGeneratorError::MissingRequiredBlock { name }) =
            generator.try_with_realistic_deposits(&registry)
        else {
            panic!("standalone deposit profile accepted missing {missing}");
        };
        assert_eq!(name, missing);
    }
}

#[test]
fn try_with_rules_allows_missing_optional_blocks_when_required_resources_exist() {
    let registry = required_only_registry();
    let generator = TerrainGenerator::try_with_rules(
        42,
        registry,
        BiomeRules::vanilla_overworld(),
        OreRules::new(Vec::new()).expect("empty ore rules fit the admission budget"),
    )
    .expect("optional terrain blocks should use fallbacks");

    assert_eq!(generator.sand, generator.stone);
    assert_eq!(generator.red_sand, generator.stone);
    assert_eq!(generator.gravel, generator.stone);
    assert_eq!(generator.podzol, generator.stone);
    assert_eq!(generator.snow_block, generator.stone);
    assert_eq!(generator.deepslate, generator.stone);
    assert_eq!(generator.water, generator.air);
    assert_eq!(generator.decorations.oak_log, None);
}

#[test]
fn generated_column_has_bedrock_and_biome_surface() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let chunk = g.generate(ChunkPos { x: 0, z: 0 });
    let air = BlockStateId(0);
    let bedrock = BlockStateId(1);
    let water = BlockStateId(5);

    // Bedrock at MIN_Y.
    assert_eq!(chunk.get_block(8, MIN_Y, 8), Some(bedrock));
    // Find the terrain surface. Biome selection decides whether it
    // is grassland, forest, cold, dry, mountain, or water/coast material.
    let height = g.surface_height(8, 8);
    assert_ne!(chunk.get_block(8, height, 8), Some(air));
    if height < SEA_LEVEL {
        assert_eq!(chunk.get_block(8, SEA_LEVEL, 8), Some(water));
        assert_eq!(chunk.get_block(8, SEA_LEVEL + 1, 8), Some(air));
    }

    // Decorations may extend the final opaque top above the terrain field.
    let hm = chunk.heightmaps.get("MOTION_BLOCKING").unwrap();
    let highest = chunk.highest_opaque_y(8, 8).unwrap();
    assert!(highest >= height);
    assert_eq!(hm.get(8, 8), (highest + 1 - MIN_Y) as u32);

    // Dirty flag set so M6 flush picks it up.
    assert!(chunk.dirty);
}

#[test]
fn generated_chunk_uses_explicit_geometry() {
    let geometry = ChunkGeometry::new(0, 256).unwrap();
    let generator = TerrainGenerator::new(42, tiny_registry()).with_geometry(geometry);

    let chunk = generator.generate(ChunkPos { x: 0, z: 0 });

    assert_eq!(chunk.geometry(), geometry);
    assert_eq!(chunk.sections.len(), 16);
    assert_eq!(chunk.get_block(8, 0, 8), Some(generator.bedrock));
    let surface = generator.surface_height(8, 8);
    let highest = chunk.highest_opaque_y(8, 8).unwrap();
    assert!(highest >= surface);
    assert_eq!(
        chunk.heightmaps["MOTION_BLOCKING"].get(8, 8),
        (highest + 1) as u32
    );
}

#[test]
fn generated_chunk_handles_geometry_above_vanilla_surface_band() {
    let geometry = ChunkGeometry::new(256, 256).unwrap();
    let generator = TerrainGenerator::new(42, tiny_registry()).with_geometry(geometry);

    let chunk = generator.generate(ChunkPos { x: 0, z: 0 });

    assert_eq!(chunk.geometry(), geometry);
    assert_eq!(
        chunk.get_block(8, geometry.min_y(), 8),
        Some(generator.bedrock)
    );
    assert!(generator.surface_height(8, 8) >= geometry.min_y());
}

#[test]
#[should_panic(expected = "chunk lies outside the supported i32 block-coordinate range")]
fn generation_rejects_chunk_coordinates_outside_the_block_domain() {
    TerrainGenerator::new(0, tiny_registry()).generate(ChunkPos { x: i32::MAX, z: 0 });
}

#[test]
fn chunk_geometry_typed_boundary_rejects_invalid_vertical_ranges() {
    assert_eq!(ChunkGeometry::new(0, 0), None);
    assert_eq!(ChunkGeometry::new(1, 16), None);
    assert_eq!(ChunkGeometry::new(0, 15), None);
    assert_eq!(ChunkGeometry::new(0, 512), None);
    assert_eq!(ChunkGeometry::new(i32::MAX - 15, 16), None);
}

#[test]
fn generated_chunks_respect_short_and_tall_geometry_boundaries() {
    for geometry in [
        ChunkGeometry::new(-16, 16).expect("one section"),
        ChunkGeometry::new(-128, 496).expect("maximum section-aligned height"),
    ] {
        let generator = TerrainGenerator::new(42, tiny_registry()).with_geometry(geometry);
        let chunk = generator.generate(ChunkPos { x: 0, z: 0 });

        assert_eq!(chunk.geometry(), geometry);
        assert_eq!(chunk.sections.len(), geometry.section_count());
        assert_eq!(
            chunk.get_block(8, geometry.min_y(), 8),
            Some(generator.bedrock)
        );
        assert_eq!(chunk.get_block(8, geometry.min_y() - 1, 8), None);
        assert_eq!(chunk.get_block(8, geometry.max_y(), 8), None);
        assert!(
            chunk.heightmaps["WORLD_SURFACE"].get(8, 8)
                <= u32::try_from(geometry.height()).unwrap()
        );
    }
}

#[test]
fn generated_chunks_handle_extreme_valid_geometry_without_y_overflow() {
    for geometry in [
        ChunkGeometry::new(i32::MIN, 16).expect("lowest aligned section"),
        ChunkGeometry::new(i32::MAX - 31, 16).expect("highest aligned section"),
    ] {
        let generator = TerrainGenerator::new(42, tiny_registry()).with_geometry(geometry);

        let chunk = generator.generate(ChunkPos { x: 0, z: 0 });

        assert_eq!(chunk.geometry(), geometry);
        assert_eq!(chunk.sections.len(), 1);
        assert_eq!(
            chunk.get_block(8, geometry.min_y(), 8),
            Some(generator.bedrock)
        );
        if let Some(below) = geometry.min_y().checked_sub(1) {
            assert_eq!(chunk.get_block(8, below, 8), None);
        }
        assert_eq!(chunk.get_block(8, geometry.max_y(), 8), None);
    }
}

fn generated_block_fingerprint(generator: &TerrainGenerator, positions: &[ChunkPos]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &pos in positions {
        let chunk = generator.generate(pos);
        for value in [
            pos.x,
            pos.z,
            chunk.geometry().min_y(),
            chunk.geometry().max_y(),
        ] {
            for byte in value.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
            }
        }
        for y in chunk.geometry().min_y()..chunk.geometry().max_y() {
            for z in 0..16u8 {
                for x in 0..16u8 {
                    let state = chunk.get_block(x, y, z).expect("Y is inside geometry");
                    for byte in state.0.to_le_bytes() {
                        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
                    }
                }
            }
        }
    }
    hash
}

#[test]
fn custom_geometry_generation_is_deterministic_for_the_same_seed() {
    let geometry = ChunkGeometry::new(0, 32).expect("two sections");
    let positions = [ChunkPos { x: -2, z: 3 }, ChunkPos { x: 7, z: -5 }];
    let first = TerrainGenerator::new(99, tiny_registry()).with_geometry(geometry);
    let second = TerrainGenerator::new(99, tiny_registry()).with_geometry(geometry);

    assert_eq!(
        generated_block_fingerprint(&first, &positions),
        generated_block_fingerprint(&second, &positions)
    );
}

#[test]
fn determinism_across_repeated_generate_calls() {
    let g = TerrainGenerator::new(99, tiny_registry());
    let a = g.generate(ChunkPos { x: 5, z: -3 });
    let b = g.generate(ChunkPos { x: 5, z: -3 });
    for y in MIN_Y..=80 {
        for x in 0..16u8 {
            for z in 0..16u8 {
                assert_eq!(a.get_block(x, y, z), b.get_block(x, y, z));
            }
        }
    }
}

#[test]
fn different_seeds_change_generated_chunks() {
    let a = TerrainGenerator::new(0, tiny_registry());
    let b = TerrainGenerator::new(1, tiny_registry());
    let positions = [
        ChunkPos { x: 0, z: 0 },
        ChunkPos { x: 5, z: -3 },
        ChunkPos { x: -12, z: 8 },
    ];

    for pos in positions {
        let chunk_a = a.generate(pos);
        let chunk_b = b.generate(pos);
        for y in MIN_Y..=96 {
            for x in 0..16u8 {
                for z in 0..16u8 {
                    if chunk_a.get_block(x, y, z) != chunk_b.get_block(x, y, z) {
                        return;
                    }
                }
            }
        }
    }

    panic!("different world seeds should alter at least one sampled generated chunk");
}

#[test]
fn far_chunks_still_have_terrain() {
    let g = TerrainGenerator::new(1234, tiny_registry());
    let chunk = g.generate(ChunkPos {
        x: 1_000,
        z: -1_000,
    });
    let height = g.surface_height(1_000 * 16 + 8, -1_000 * 16 + 8);
    let biome = g.biome_for(1_000 * 16 + 8, -1_000 * 16 + 8, height);
    let (surface, _) = g.surface_materials(&biome);
    assert!(matches!(chunk.get_block(8, height, 8), Some(state) if state == surface));
    assert_eq!(chunk.status, "minecraft:full");
}

#[test]
fn default_seed_origin_remains_land_spawn() {
    let g = TerrainGenerator::new(0, tiny_registry());
    let height = g.surface_height(0, 0);
    let biome = g.biome_for(0, 0, height);

    assert!(height > SEA_LEVEL, "spawn origin should not be underwater");
    assert!(!g.biomes.is_ocean(&biome));
}

#[test]
fn continental_mask_produces_water_coasts_and_land_biomes() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut saw_ocean = false;
    let mut saw_beach = false;
    let mut saw_forest_or_jungle = false;
    let mut saw_grass_or_dry = false;
    let mut saw_mountain = false;
    let mut ocean_column = None;

    for wx in (-4096..=4096).step_by(64) {
        for wz in (-4096..=4096).step_by(64) {
            let height = g.surface_height(wx, wz);
            let biome = g.biome_for(wx, wz, height);
            if g.biomes.is_ocean(&biome) {
                saw_ocean = true;
                ocean_column.get_or_insert((wx, wz, height));
            } else if g.biomes.is_beach_or_shore(&biome) {
                saw_beach = true;
            } else if g.biomes.temperate_forest.contains(&biome) || g.biomes.jungle.contains(&biome)
            {
                saw_forest_or_jungle = true;
            } else if g.biomes.grassland.contains(&biome) || g.biomes.hot_dry.contains(&biome) {
                saw_grass_or_dry = true;
            } else if g.biomes.mountain.contains(&biome) {
                saw_mountain = true;
            }
        }
    }

    assert!(saw_ocean, "expected ocean cells in the sampled area");
    assert!(saw_beach, "expected beach cells around coastlines");
    assert!(
        saw_forest_or_jungle,
        "expected forest/jungle cells in the sampled area"
    );
    assert!(
        saw_grass_or_dry,
        "expected grassland/dry cells in the sampled area"
    );
    assert!(saw_mountain, "expected mountain cells in the sampled area");

    let (wx, wz, height) = ocean_column.unwrap();
    let chunk = g.generate(ChunkPos {
        x: wx.div_euclid(16),
        z: wz.div_euclid(16),
    });
    let lx = wx.rem_euclid(16) as u8;
    let lz = wz.rem_euclid(16) as u8;
    assert!(height < SEA_LEVEL);
    assert_eq!(chunk.get_block(lx, SEA_LEVEL, lz), Some(BlockStateId(5)));
    assert_eq!(
        chunk.get_block(lx, SEA_LEVEL + 1, lz),
        Some(BlockStateId(0))
    );
}

#[test]
fn default_seed_coastline_has_bounded_steps() {
    let g = TerrainGenerator::new(0, tiny_registry());
    for wx in (-512..=512).step_by(8) {
        for wz in (-512..=512).step_by(8) {
            let h = g.surface_height(wx, wz);
            let hx = g.surface_height(wx + 1, wz);
            let hz = g.surface_height(wx, wz + 1);
            let near_coast = (h - SEA_LEVEL).abs() <= 12
                || (hx - SEA_LEVEL).abs() <= 12
                || (hz - SEA_LEVEL).abs() <= 12;
            if near_coast {
                assert!(
                    (h - hx).abs() <= 6,
                    "sharp x coastline step at ({wx}, {wz}): {h} -> {hx}"
                );
                assert!(
                    (h - hz).abs() <= 6,
                    "sharp z coastline step at ({wx}, {wz}): {h} -> {hz}"
                );
            }
        }
    }
}

#[test]
fn solaris_owned_land_biomes_form_chunk_scale_regions() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut stable_windows = 0usize;

    for wx in (-1_536..=1_536).step_by(64) {
        for wz in (-1_536..=1_536).step_by(64) {
            let mut expected = None;
            let mut stable = true;
            for dx in [0, 16, 32, 48] {
                for dz in [0, 16, 32, 48] {
                    let x = wx + dx;
                    let z = wz + dz;
                    let height = g.surface_height(x, z);
                    if height <= SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA + 2 {
                        stable = false;
                        continue;
                    }
                    let biome = g.biome_for(x, z, height);
                    if land_biome_family(&g, &biome).is_none() {
                        stable = false;
                        continue;
                    }
                    match &expected {
                        Some(expected) if expected != &biome => stable = false,
                        None => expected = Some(biome),
                        _ => {}
                    }
                }
            }
            if stable && expected.is_some() {
                stable_windows += 1;
            }
        }
    }

    assert!(
        stable_windows >= 8,
        "expected multiple 4x4-chunk land windows to keep one exact biome, saw {stable_windows}"
    );
}

#[test]
fn solaris_owned_biome_choices_do_not_track_192_block_grid_lines() {
    const OLD_GRID_SIZE: i32 = 192;

    let g = TerrainGenerator::new(42, tiny_registry());
    let mut aligned_pairs = 0usize;
    let mut aligned_flips = 0usize;
    let mut control_pairs = 0usize;
    let mut control_flips = 0usize;

    for grid_x in (-1_536..=1_536).step_by(OLD_GRID_SIZE as usize) {
        for wz in (-2_048..=2_048).step_by(16) {
            for (left, right, pairs, flips) in [
                (
                    grid_x - 4,
                    grid_x + 4,
                    &mut aligned_pairs,
                    &mut aligned_flips,
                ),
                (
                    grid_x + 72,
                    grid_x + 80,
                    &mut control_pairs,
                    &mut control_flips,
                ),
            ] {
                let left_height = g.surface_height(left, wz);
                let right_height = g.surface_height(right, wz);
                let left_biome = g.biome_for(left, wz, left_height);
                let right_biome = g.biome_for(right, wz, right_height);
                let Some(left_family) = land_biome_family(&g, &left_biome) else {
                    continue;
                };
                let Some(right_family) = land_biome_family(&g, &right_biome) else {
                    continue;
                };
                if left_family != right_family {
                    continue;
                }
                *pairs += 1;
                if left_biome != right_biome {
                    *flips += 1;
                }
            }
        }
    }

    assert!(
        aligned_pairs > 150,
        "sample should include enough grid-line land pairs, saw {aligned_pairs} (control {control_pairs})"
    );
    assert!(
        control_pairs > 150,
        "sample should include enough control land pairs"
    );
    assert!(
        aligned_flips * control_pairs
            <= (control_flips * aligned_pairs) + (aligned_pairs * control_pairs / 20),
        "biome flips should not concentrate on 192-block grid lines: aligned {aligned_flips}/{aligned_pairs}, control {control_flips}/{control_pairs}"
    );
}

#[test]
fn solaris_owned_nearby_land_samples_avoid_excessive_family_flips() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut comparable = 0usize;
    let mut flips = 0usize;

    for wx in (-1_024..=1_024).step_by(16) {
        for wz in (-1_024..=1_024).step_by(16) {
            let height = g.surface_height(wx, wz);
            let biome = g.biome_for(wx, wz, height);
            let Some(family) = land_biome_family(&g, &biome) else {
                continue;
            };
            for (nx, nz) in [(wx + 16, wz), (wx, wz + 16)] {
                let nheight = g.surface_height(nx, nz);
                let nbiome = g.biome_for(nx, nz, nheight);
                let Some(nfamily) = land_biome_family(&g, &nbiome) else {
                    continue;
                };
                comparable += 1;
                if family != nfamily {
                    flips += 1;
                }
            }
        }
    }

    assert!(comparable > 200, "sample should include enough land pairs");
    assert!(
        flips * 5 <= comparable,
        "too many 16-block land biome family flips: {flips}/{comparable}"
    );
}

#[test]
fn solaris_owned_river_masks_are_broad_when_reachable() {
    let g = TerrainGenerator::new(42, tiny_registry());

    for wx in (-2_048..=2_048).step_by(4) {
        for wz in (-2_048..=2_048).step_by(4) {
            let height = g.surface_height(wx, wz);
            let biome = g.biome_for(wx, wz, height);
            if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                continue;
            }

            let mut width_x = 1usize;
            for dx in 1..=32 {
                let height = g.surface_height(wx + dx, wz);
                let biome = g.biome_for(wx + dx, wz, height);
                if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                    break;
                }
                width_x += 1;
            }
            for dx in 1..=32 {
                let height = g.surface_height(wx - dx, wz);
                let biome = g.biome_for(wx - dx, wz, height);
                if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                    break;
                }
                width_x += 1;
            }
            let mut width_z = 1usize;
            for dz in 1..=32 {
                let height = g.surface_height(wx, wz + dz);
                let biome = g.biome_for(wx, wz + dz, height);
                if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                    break;
                }
                width_z += 1;
            }
            for dz in 1..=32 {
                let height = g.surface_height(wx, wz - dz);
                let biome = g.biome_for(wx, wz - dz, height);
                if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                    break;
                }
                width_z += 1;
            }
            if width_x < 8 || width_z < 8 {
                continue;
            }

            let mut nearby_river_water = 0usize;
            for dx in -4..=4 {
                for dz in -4..=4 {
                    let height = g.surface_height(wx + dx, wz + dz);
                    let biome = g.biome_for(wx + dx, wz + dz, height);
                    if g.biomes.is_river(&biome) && height <= SEA_LEVEL {
                        nearby_river_water += 1;
                    }
                }
            }
            assert!(
                nearby_river_water >= 24,
                "river should occupy a broad local cross-section near ({wx}, {wz}), saw {nearby_river_water}/81 water-level river columns"
            );

            let chunk = g.generate(ChunkPos {
                x: wx.div_euclid(16),
                z: wz.div_euclid(16),
            });
            assert_eq!(
                chunk.get_block(wx.rem_euclid(16) as u8, SEA_LEVEL, wz.rem_euclid(16) as u8),
                Some(BlockStateId(5)),
                "river centre at ({wx}, {wz}) with surface {}",
                g.surface_height(wx, wz)
            );
            return;
        }
    }

    panic!("sampled area should contain a reachable broad river cross-section");
}

#[test]
fn river_biomes_only_label_carved_water_floors() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut rivers = 0usize;

    for wx in (-4_096..=4_096).step_by(32) {
        for wz in (-4_096..=4_096).step_by(32) {
            let height = g.surface_height(wx, wz);
            let biome = g.biome_for(wx, wz, height);
            if g.biomes.is_river(&biome) {
                rivers += 1;
                assert!(
                    height <= SEA_LEVEL,
                    "river biome at ({wx},{wz}) was not carved: {height}"
                );
            }
        }
    }

    assert!(rivers > 0, "sample should contain routed river floors");
}

#[test]
fn solaris_owned_river_valleys_keep_continuous_water_floors() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut checked = 0usize;

    for wx in (-2_048..=2_048).step_by(4) {
        for wz in (-2_048..=2_048).step_by(4) {
            let height = g.surface_height(wx, wz);
            let biome = g.biome_for(wx, wz, height);
            if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                continue;
            }

            for (axis_x, axis_z) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
                let mut wet_floor = true;
                let mut center_height = i32::MAX;
                for offset in [-12, -8, -4, 0, 4, 8, 12] {
                    let x = wx + axis_x * offset;
                    let z = wz + axis_z * offset;
                    let sample_height = g.surface_height(x, z);
                    let sample_biome = g.biome_for(x, z, sample_height);
                    center_height = center_height.min(sample_height);
                    if !g.biomes.is_river(&sample_biome) || sample_height > SEA_LEVEL {
                        wet_floor = false;
                        break;
                    }
                }
                if !wet_floor {
                    continue;
                }

                let mut previous_height = None;
                for offset in (-28..=28).step_by(4) {
                    let x = wx + axis_x * offset;
                    let z = wz + axis_z * offset;
                    let sample_height = g.surface_height(x, z);
                    if let Some(previous_height) = previous_height {
                        let step: i32 = sample_height - previous_height;
                        assert!(
                            step.abs() <= 10,
                            "river cross-section should rise gradually near ({wx}, {wz}) on axis ({axis_x}, {axis_z}): offset {offset}, step {step}"
                        );
                    }
                    previous_height = Some(sample_height);
                }

                let ordinary_banks = [-28, 28].into_iter().all(|offset| {
                    let x = wx + axis_x * offset;
                    let z = wz + axis_z * offset;
                    g.surface_height(x, z) >= center_height
                });
                if !ordinary_banks {
                    // A lower side sample is another drainage branch or a coast
                    // intersection, not an ordinary single-channel bank profile.
                    continue;
                }
                checked += 1;
                if checked >= 6 {
                    return;
                }
            }
        }
    }

    assert!(
        checked >= 3,
        "sampled area should contain several continuous river water-floor cross-sections, saw {checked}"
    );
}

#[test]
fn solaris_owned_local_terrain_steps_stay_bounded() {
    let g = TerrainGenerator::new(42, tiny_registry());
    for wx in (-512..=512).step_by(4) {
        for wz in (-512..=512).step_by(4) {
            let h = g.surface_height(wx, wz);
            let hx = g.surface_height(wx + 4, wz);
            let hz = g.surface_height(wx, wz + 4);
            assert!(
                (h - hx).abs() <= 10,
                "sharp x terrain step at ({wx}, {wz}): {h} -> {hx}"
            );
            assert!(
                (h - hz).abs() <= 10,
                "sharp z terrain step at ({wx}, {wz}): {h} -> {hz}"
            );
        }
    }
}

#[test]
fn generated_chunk_biomes_are_not_all_default() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut saw_non_default = false;
    for cx in -8..=8 {
        for cz in -8..=8 {
            let chunk = g.generate(ChunkPos { x: cx, z: cz });
            for section in &chunk.biomes {
                match section {
                    BiomeSection::Single(id) => {
                        saw_non_default |= id.as_str() != "minecraft:plains";
                    }
                    BiomeSection::Indirect { palette, .. } => {
                        saw_non_default |=
                            palette.iter().any(|id| id.as_str() != "minecraft:plains");
                    }
                }
            }
        }
    }
    assert!(saw_non_default, "generated chunks should carry biome data");
}

#[test]
fn ocean_chunks_can_contain_underwater_vegetation_columns() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut saw_seagrass = false;
    for cx in -32..=32 {
        for cz in -32..=32 {
            let chunk = g.generate(ChunkPos { x: cx, z: cz });
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let wx = cx * 16 + lx as i32;
                    let wz = cz * 16 + lz as i32;
                    let height = g.surface_height(wx, wz);
                    let biome = g.biome_for(wx, wz, height);
                    if g.biomes.is_ocean(&biome)
                        && matches!(
                            chunk.get_block(lx, height + 1, lz),
                            Some(BlockStateId(40 | 41))
                        )
                    {
                        assert_eq!(chunk.get_block(lx, SEA_LEVEL, lz), Some(BlockStateId(5)));
                        saw_seagrass = true;
                    }
                    if g.biomes.is_ocean(&biome)
                        && chunk.get_block(lx, height + 1, lz) == Some(BlockStateId(42))
                    {
                        let mut y = height + 1;
                        while chunk.get_block(lx, y, lz) == Some(BlockStateId(42)) {
                            y += 1;
                        }
                        assert_eq!(chunk.get_block(lx, y, lz), Some(BlockStateId(41)));
                        assert!(y > height + 1, "kelp should be a visible column");
                        return;
                    }
                }
            }
        }
    }
    assert!(saw_seagrass, "sampled ocean chunks should contain seagrass");
    panic!("sampled ocean chunks should contain kelp columns");
}

#[test]
fn biome_surface_rules_are_visibly_distinct() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let cases = [
        ("minecraft:plains", BlockStateId(4), BlockStateId(3)),
        ("minecraft:birch_forest", BlockStateId(4), BlockStateId(3)),
        ("minecraft:badlands", BlockStateId(34), BlockStateId(34)),
        ("minecraft:desert", BlockStateId(14), BlockStateId(14)),
        ("minecraft:jagged_peaks", BlockStateId(35), BlockStateId(2)),
        ("minecraft:snowy_plains", BlockStateId(37), BlockStateId(3)),
        ("minecraft:beach", BlockStateId(14), BlockStateId(14)),
    ];

    for (biome, surface, fill) in cases {
        let biome = Identifier::parse(biome).unwrap();
        assert_eq!(g.surface_materials(&biome), (surface, fill));
    }
}

#[test]
fn surface_decorations_are_visible_and_refresh_heightmaps() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let decorations = [
        BlockStateId(26),
        BlockStateId(27),
        BlockStateId(28),
        BlockStateId(29),
        BlockStateId(30),
        BlockStateId(31),
        BlockStateId(32),
        BlockStateId(33),
    ];
    let mut saw_decoration = false;

    for cx in -2..=2 {
        for cz in -2..=2 {
            let chunk = g.generate(ChunkPos { x: cx, z: cz });
            let again = g.generate(ChunkPos { x: cx, z: cz });
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let wx = cx * 16 + lx as i32;
                    let wz = cz * 16 + lz as i32;
                    let height = g.surface_height(wx, wz);
                    for y in (height + 1)..=(height + 8).min(MAX_Y - 1) {
                        assert_eq!(chunk.get_block(lx, y, lz), again.get_block(lx, y, lz));
                        if chunk
                            .get_block(lx, y, lz)
                            .is_some_and(|state| decorations.contains(&state))
                        {
                            saw_decoration = true;
                            assert!(chunk.highest_opaque_y(lx, lz).is_some_and(|top| top >= y));
                            assert!(
                                chunk.heightmaps["WORLD_SURFACE"].get(lx, lz)
                                    >= (y + 1 - MIN_Y) as u32
                            );
                        }
                    }
                }
            }
        }
    }

    assert!(
        saw_decoration,
        "sampled chunks should contain surface decorations"
    );
}

#[test]
fn pumpkins_remain_a_rare_surface_decoration() {
    let registry = tiny_registry();
    let mut eligible_columns = 0usize;
    let mut pumpkins = 0usize;
    for seed in -2..2 {
        let generator = TerrainGenerator::new(seed, Arc::clone(&registry));
        let pumpkin = generator.decorations.pumpkin.expect("pumpkin state");
        for chunk_x in -2..=2 {
            for chunk_z in -2..=2 {
                let pos = ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                };
                let chunk = generator.generate(pos);
                for lx in 0..16u8 {
                    for lz in 0..16u8 {
                        let plan = generator.plan_column(pos, lx, lz, None);
                        if (generator.biomes.grassland.contains(&plan.biome)
                            || generator.biomes.temperate_forest.contains(&plan.biome)
                            || generator.biomes.jungle.contains(&plan.biome))
                            && (plan.surface == generator.grass_block
                                || plan.surface == generator.podzol)
                        {
                            eligible_columns += 1;
                            pumpkins += usize::from(
                                chunk.get_block(lx, plan.height + 1, lz) == Some(pumpkin),
                            );
                        }
                    }
                }
            }
        }
    }

    assert!(pumpkins > 0, "sample should contain a pumpkin");
    assert!(
        pumpkins * 256 <= eligible_columns,
        "pumpkins are too dense: {pumpkins}/{eligible_columns} eligible columns"
    );
}

#[test]
fn surface_vegetation_density_is_moderate_and_biome_specific() {
    let generator = TerrainGenerator::new(42, tiny_registry());
    let plants = [
        generator.decorations.short_grass,
        generator.decorations.dandelion,
        generator.decorations.poppy,
        generator.decorations.pumpkin,
    ];
    let logs = [
        generator.decorations.oak_log,
        generator.decorations.forest_log,
        generator.decorations.cold_log,
        generator.decorations.jungle_log,
        generator.decorations.acacia_log,
    ];
    let mut eligible = [0usize; 3];
    let mut decorated = [0usize; 3];

    for chunk_x in (-256..=256).step_by(32) {
        for chunk_z in (-256..=256).step_by(32) {
            let pos = ChunkPos {
                x: chunk_x,
                z: chunk_z,
            };
            let chunk = generator.generate(pos);
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let plan = generator.plan_column(pos, lx, lz, None);
                    let category = if generator.biomes.jungle.contains(&plan.biome) {
                        2
                    } else if generator.biomes.temperate_forest.contains(&plan.biome) {
                        1
                    } else if generator.biomes.grassland.contains(&plan.biome) {
                        0
                    } else {
                        continue;
                    };
                    if plan.surface != generator.grass_block && plan.surface != generator.podzol {
                        continue;
                    }
                    eligible[category] += 1;
                    let decoration = chunk.get_block(lx, plan.height + 1, lz);
                    decorated[category] +=
                        usize::from(plants.contains(&decoration) || logs.contains(&decoration));
                }
            }
        }
    }

    for (index, label) in ["grassland", "forest", "jungle"].into_iter().enumerate() {
        let eligible_count = eligible[index];
        let decorated_count = decorated[index];
        assert!(
            eligible_count > 256,
            "sampled only {eligible_count} {label} columns"
        );
        assert!(decorated_count > 20, "{label} vegetation became too sparse");
        // Jungle is the densest overworld forest; holding it to the same
        // 1/8 cover cap as plains forces the sparse-jungle defect back.
        let cover_limit = if index == 2 { 4 } else { 8 };
        assert!(
            decorated_count * cover_limit <= eligible_count,
            "{label} vegetation is too dense: {decorated_count}/{eligible_count} eligible columns"
        );
    }
    assert!(
        decorated[2] * eligible[1] > decorated[1] * eligible[2],
        "jungle should be denser than forest: {}/{} versus {}/{}",
        decorated[2],
        eligible[2],
        decorated[1],
        eligible[1]
    );
}

#[test]
fn jungle_admits_trees_more_often_than_forest_and_plains() {
    let generator = TerrainGenerator::new(42, tiny_registry());
    let mut eligible = [0usize; 3];
    let mut candidates = [0usize; 3];
    for chunk_x in (-256..=256).step_by(32) {
        for chunk_z in (-256..=256).step_by(32) {
            let pos = ChunkPos {
                x: chunk_x,
                z: chunk_z,
            };
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let plan = generator.plan_column(pos, lx, lz, None);
                    let category = if generator.biomes.jungle.contains(&plan.biome) {
                        2
                    } else if generator.biomes.temperate_forest.contains(&plan.biome) {
                        1
                    } else if generator.biomes.grassland.contains(&plan.biome) {
                        0
                    } else {
                        continue;
                    };
                    eligible[category] += 1;
                    candidates[category] += usize::from(
                        generator
                            .tree_spacing_for_biome(&plan.biome)
                            .is_some_and(|spacing| plan.hash.is_multiple_of(spacing))
                            && generator.tree_density_allows(&plan),
                    );
                }
            }
        }
    }
    for (index, label) in ["grassland", "forest", "jungle"].into_iter().enumerate() {
        assert!(
            eligible[index] > 256,
            "sampled only {eligible_count} {label} columns",
            eligible_count = eligible[index]
        );
    }
    assert!(
        candidates[2] * eligible[1] > candidates[1] * eligible[2],
        "jungle must admit trees more often than forest: {}/{} versus {}/{}",
        candidates[2],
        eligible[2],
        candidates[1],
        eligible[1]
    );
    assert!(
        candidates[2] * eligible[0] > candidates[0] * eligible[2],
        "jungle must admit trees more often than plains: {}/{} versus {}/{}",
        candidates[2],
        eligible[2],
        candidates[0],
        eligible[0]
    );
}

#[test]
fn vegetation_density_changes_over_regions_instead_of_columns() {
    let registry = tiny_registry();
    let mut near_change = 0.0;
    let mut regional_change = 0.0;
    let mut comparisons = 0usize;

    for seed in [-11, 0, 712_816] {
        let generator = TerrainGenerator::new(seed, Arc::clone(&registry));
        for z in (-2_048_i32..=2_048_i32).step_by(137) {
            for x in (-2_048_i32..=2_048_i32).step_by(131) {
                let centre = generator.plan_column(
                    ChunkPos {
                        x: x.div_euclid(16),
                        z: z.div_euclid(16),
                    },
                    x.rem_euclid(16) as u8,
                    z.rem_euclid(16) as u8,
                    None,
                );
                let near = generator.plan_column(
                    ChunkPos {
                        x: (x + 8).div_euclid(16),
                        z: z.div_euclid(16),
                    },
                    (x + 8).rem_euclid(16) as u8,
                    z.rem_euclid(16) as u8,
                    None,
                );
                let regional = generator.plan_column(
                    ChunkPos {
                        x: (x + 192).div_euclid(16),
                        z: z.div_euclid(16),
                    },
                    (x + 192).rem_euclid(16) as u8,
                    z.rem_euclid(16) as u8,
                    None,
                );
                near_change += (centre.vegetation_density - near.vegetation_density).abs();
                regional_change += (centre.vegetation_density - regional.vegetation_density).abs();
                comparisons += 1;
            }
        }
    }

    assert!(comparisons > 2_000);
    assert!(
        near_change * 4.0 < regional_change,
        "vegetation density is too noisy per column: near={near_change}, regional={regional_change}"
    );
}

#[test]
fn open_cold_biomes_are_treeless_while_taiga_and_grove_keep_spruce() {
    let generator = TerrainGenerator::new(42, tiny_registry());
    for name in ["minecraft:snowy_plains", "minecraft:ice_spikes"] {
        let biome = Identifier::parse(name).unwrap();
        assert_eq!(generator.tree_spacing_for_biome(&biome), None, "{name}");
        assert!(!TerrainGenerator::is_cold_forest(&biome), "{name}");
    }
    for name in [
        "minecraft:taiga",
        "minecraft:snowy_taiga",
        "minecraft:old_growth_pine_taiga",
        "minecraft:old_growth_spruce_taiga",
        "minecraft:grove",
    ] {
        let biome = Identifier::parse(name).unwrap();
        assert!(generator.tree_spacing_for_biome(&biome).is_some(), "{name}");
        assert_eq!(
            generator
                .tree_blocks_for_biome(&biome)
                .map(|blocks| blocks.kind),
            Some(TreeKind::Spruce),
            "{name}"
        );
    }
}

#[test]
fn generated_tree_trunks_start_on_the_planned_surface() {
    let registry = tiny_registry();
    let mut trees = 0usize;
    for seed in -4..4 {
        let generator = TerrainGenerator::new(seed, Arc::clone(&registry));
        let log_states = [
            generator.decorations.oak_log,
            generator.decorations.forest_log,
            generator.decorations.cold_log,
            generator.decorations.jungle_log,
            generator.decorations.acacia_log,
        ];
        for chunk_x in -3..=3 {
            for chunk_z in -3..=3 {
                let pos = ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                };
                let chunk = generator.generate(pos);
                for lx in 0..16u8 {
                    for lz in 0..16u8 {
                        let plan = generator.plan_column(pos, lx, lz, None);
                        let first_above = chunk.get_block(lx, plan.height + 1, lz);
                        let has_trunk = (plan.height + 1..=plan.height + 6).any(|y| {
                            chunk
                                .get_block(lx, y, lz)
                                .is_some_and(|state| log_states.contains(&Some(state)))
                        });
                        if !has_trunk {
                            continue;
                        }
                        trees += 1;
                        assert_eq!(
                            chunk.get_block(lx, plan.height, lz),
                            Some(plan.surface),
                            "tree support changed at {},{}",
                            plan.wx,
                            plan.wz,
                        );
                        assert!(
                            first_above.is_some_and(|state| log_states.contains(&Some(state))),
                            "tree trunk floats above {},{}",
                            plan.wx,
                            plan.wz,
                        );
                    }
                }
            }
        }
    }
    assert!(trees >= 8, "sample should contain generated trees");
}

#[test]
fn tree_species_have_distinct_tapered_canopy_profiles() {
    let profile = |kind| {
        (-4..=1)
            .filter_map(|relative_y| tree_canopy_radius(kind, relative_y))
            .collect::<Vec<_>>()
    };

    let oak = profile(TreeKind::Oak);
    let birch = profile(TreeKind::Birch);
    let spruce = profile(TreeKind::Spruce);
    let jungle = profile(TreeKind::Jungle);
    let acacia = profile(TreeKind::Acacia);
    assert_eq!(oak.last(), Some(&1));
    assert_eq!(birch.last(), Some(&0));
    assert_eq!(spruce.last(), Some(&0));
    assert_eq!(jungle.last(), Some(&1));
    assert_eq!(acacia.last(), Some(&1));
    assert!(oak.iter().any(|radius| *radius > *oak.last().unwrap()));
    assert!(birch.iter().any(|radius| *radius > *birch.last().unwrap()));
    assert!(
        spruce
            .iter()
            .any(|radius| *radius > *spruce.last().unwrap())
    );
    assert!(
        jungle
            .iter()
            .any(|radius| *radius > *jungle.last().unwrap())
    );
    assert!(
        acacia
            .iter()
            .any(|radius| *radius > *acacia.last().unwrap())
    );
    assert_ne!(oak, birch);
    assert_ne!(oak, spruce);
    assert_ne!(oak, jungle);
    assert_ne!(oak, acacia);
    assert_ne!(birch, spruce);
    assert_ne!(birch, jungle);
    assert_ne!(birch, acacia);
    assert_ne!(spruce, jungle);
    assert_ne!(spruce, acacia);
    assert_ne!(jungle, acacia);
}

#[test]
fn generated_tree_canopies_narrow_above_the_main_crown() {
    let registry = tiny_registry();
    let mut inspected = 0usize;
    for seed in -4..4 {
        let generator = TerrainGenerator::new(seed, Arc::clone(&registry));
        let logs = [
            generator.decorations.oak_log,
            generator.decorations.forest_log,
            generator.decorations.cold_log,
            generator.decorations.jungle_log,
            generator.decorations.acacia_log,
        ];
        let leaves = [
            generator.decorations.oak_leaves,
            generator.decorations.forest_leaves,
            generator.decorations.cold_leaves,
            generator.decorations.jungle_leaves,
            generator.decorations.acacia_leaves,
        ];
        for chunk_x in -3..=3 {
            for chunk_z in -3..=3 {
                let pos = ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                };
                let chunk = generator.generate(pos);
                for lx in 2..=13u8 {
                    for lz in 2..=13u8 {
                        let plan = generator.plan_column(pos, lx, lz, None);
                        let base_y = plan.height + 1;
                        if !chunk
                            .get_block(lx, base_y, lz)
                            .is_some_and(|state| logs.contains(&Some(state)))
                        {
                            continue;
                        }
                        let trunk_top = (base_y..=base_y + 7)
                            .take_while(|y| {
                                chunk
                                    .get_block(lx, *y, lz)
                                    .is_some_and(|state| logs.contains(&Some(state)))
                            })
                            .last()
                            .expect("tree has a trunk base");
                        let mut layers = Vec::new();
                        for y in base_y..=base_y + 8 {
                            let count = (-2..=2)
                                .flat_map(|dx| (-2..=2).map(move |dz| (dx, dz)))
                                .filter(|(dx, dz)| {
                                    let x = i32::from(lx) + dx;
                                    let z = i32::from(lz) + dz;
                                    (0..16).contains(&x)
                                        && (0..16).contains(&z)
                                        && chunk
                                            .get_block(x as u8, y, z as u8)
                                            .is_some_and(|state| leaves.contains(&Some(state)))
                                })
                                .count();
                            if count > 0 {
                                layers.push((y, count));
                            }
                        }
                        let (top_y, top_count) = layers.last().copied().expect("leaf canopy");
                        let widest = layers.iter().map(|(_, count)| *count).max().unwrap();
                        assert!(top_y > trunk_top, "tree crown must cap the trunk");
                        assert!(
                            top_count < widest,
                            "tree crown must taper above its main canopy"
                        );
                        inspected += 1;
                    }
                }
            }
        }
    }
    assert!(inspected >= 8, "sample should contain generated trees");
}

#[test]
fn radius_one_tree_crowns_are_raised_and_irregular() {
    let registry = tiny_registry();
    let generator = TerrainGenerator::new(42, registry);
    let plan = generator.plan_column(ChunkPos { x: 0, z: 0 }, 8, 8, None);
    let trunk_top_y = plan.height + 5;

    let layer = |kind, relative_y| {
        (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
            .filter(|(dx, dz)| {
                generator.tree_leaf_is_present(
                    &plan,
                    kind,
                    trunk_top_y,
                    TreeLeafOffset {
                        relative_y,
                        dx: *dx,
                        dz: *dz,
                        radius: 1,
                    },
                )
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(tree_canopy_radius(TreeKind::Oak, 0), Some(1));
    let main = layer(TreeKind::Oak, 0);
    assert_eq!(main.len(), 8, "main oak crown keeps three corners");

    for kind in [TreeKind::Oak, TreeKind::Jungle] {
        assert_eq!(tree_canopy_radius(kind, 1), Some(1));
        let raised = layer(kind, 1);
        assert_eq!(raised.len(), 6, "raised crown keeps one corner");
        assert!(
            raised.contains(&(0, 0))
                && raised.contains(&(-1, 0))
                && raised.contains(&(1, 0))
                && raised.contains(&(0, -1))
                && raised.contains(&(0, 1)),
            "raised crown keeps a connected cross"
        );
    }
}

#[test]
fn generated_columns_keep_a_solid_surface_shell_across_seeds() {
    let registry = tiny_registry();
    for seed in [i64::MIN, -1_000_003, -4, -1, 0, 1, 3, 999_983, i64::MAX] {
        let generator = TerrainGenerator::new(seed, Arc::clone(&registry));
        for pos in [
            ChunkPos { x: -2, z: -2 },
            ChunkPos { x: 0, z: 0 },
            ChunkPos { x: 2, z: -1 },
            ChunkPos { x: 1, z: 2 },
        ] {
            let chunk = generator.generate(pos);
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let wx = pos.x * 16 + i32::from(lx);
                    let wz = pos.z * 16 + i32::from(lz);
                    let surface = generator.surface_height(wx, wz);
                    // The cave cutoff itself may be carved. The 32 cells above it
                    // are the protected shell, including the surface block.
                    let cave_cutoff = surface - CAVE_SURFACE_CLEARANCE;
                    let shell_bottom = (cave_cutoff + 1).max(generator.geometry.min_y() + 1);
                    assert_eq!(surface - shell_bottom + 1, CAVE_SURFACE_CLEARANCE);
                    for y in shell_bottom..=surface {
                        assert_ne!(
                            chunk.get_block(lx, y, lz),
                            Some(generator.air),
                            "seed {seed} opened the protected surface shell at {wx},{y},{wz}"
                        );
                    }
                }
            }
        }
    }
}

fn find_ore_cell(
    g: &TerrainGenerator,
    y: i32,
    biome: &Identifier,
    expected: BlockStateId,
) -> (i32, i32) {
    for x in -256..=256 {
        for z in -256..=256 {
            if g.ore_for(x, y, z, g.stone, biome) == expected {
                return (x, z);
            }
        }
    }
    panic!("could not find ore {expected:?} at y={y} for biome {biome}");
}

pub(in crate::terrain) fn ore_feature(
    feature: &str,
    normal: &str,
    deepslate: &str,
    min_y: i32,
    max_y: i32,
    count: u32,
) -> OreFeature {
    OreFeature {
        placed_feature: Identifier::parse(feature).unwrap(),
        configured_feature: Identifier::parse(feature).unwrap(),
        placement: mc_data::worldgen_ores::OrePlacement {
            count: Some(OrePlacementCount::Constant(count)),
            rarity_chance: None,
            height: Some(mc_data::worldgen_ores::HeightRange {
                kind: Identifier::parse("minecraft:uniform").unwrap(),
                min: HeightAnchor::Absolute(min_y),
                max: HeightAnchor::Absolute(max_y),
            }),
        },
        size: 4,
        discard_chance_on_air_exposure: 0.0,
        targets: vec![
            OreTarget {
                state: Identifier::parse(normal).unwrap(),
                replaceable_tag: Some(
                    Identifier::parse("minecraft:stone_ore_replaceables").unwrap(),
                ),
            },
            OreTarget {
                state: Identifier::parse(deepslate).unwrap(),
                replaceable_tag: Some(
                    Identifier::parse("minecraft:deepslate_ore_replaceables").unwrap(),
                ),
            },
        ],
    }
}

#[test]
fn expanded_ore_families_are_reachable_and_biome_scoped() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let mountain = Identifier::parse("minecraft:jagged_peaks").unwrap();
    let badlands = Identifier::parse("minecraft:badlands").unwrap();

    for (y, biome, stone_ore, deepslate_ore) in [
        (-16, &plains, BlockStateId(15), BlockStateId(20)),
        (15, &plains, BlockStateId(16), BlockStateId(21)),
        (-56, &plains, BlockStateId(17), BlockStateId(22)),
        (64, &plains, BlockStateId(18), BlockStateId(23)),
        (224, &mountain, BlockStateId(19), BlockStateId(24)),
        (80, &badlands, BlockStateId(15), BlockStateId(20)),
    ] {
        let (x, z) = find_ore_cell(&g, y, biome, stone_ore);
        assert_eq!(g.ore_for(x, y, z, g.deepslate, biome), deepslate_ore);
    }

    let (emerald_x, emerald_z) = find_ore_cell(&g, 224, &mountain, BlockStateId(19));
    for biome in [
        "minecraft:cherry_grove",
        "minecraft:frozen_peaks",
        "minecraft:grove",
        "minecraft:jagged_peaks",
        "minecraft:meadow",
        "minecraft:snowy_slopes",
        "minecraft:stony_peaks",
        "minecraft:windswept_forest",
        "minecraft:windswept_gravelly_hills",
        "minecraft:windswept_hills",
    ] {
        let biome = Identifier::parse(biome).unwrap();
        assert_eq!(
            g.ore_for(emerald_x, 224, emerald_z, g.stone, &biome),
            BlockStateId(19)
        );
    }
    for biome in ["minecraft:plains", "minecraft:desert"] {
        let biome = Identifier::parse(biome).unwrap();
        assert_eq!(
            g.ore_for(emerald_x, 224, emerald_z, g.stone, &biome),
            g.stone
        );
    }

    let (gold_x, gold_z) = find_ore_cell(&g, 80, &badlands, BlockStateId(15));
    for biome in [
        "minecraft:badlands",
        "minecraft:eroded_badlands",
        "minecraft:wooded_badlands",
    ] {
        let biome = Identifier::parse(biome).unwrap();
        assert_eq!(
            g.ore_for(gold_x, 80, gold_z, g.stone, &biome),
            BlockStateId(15)
        );
    }
    for biome in ["minecraft:plains", "minecraft:desert", "minecraft:savanna"] {
        let biome = Identifier::parse(biome).unwrap();
        assert_ne!(
            g.ore_for(gold_x, 80, gold_z, g.stone, &biome),
            BlockStateId(15)
        );
    }
}

#[test]
fn data_fed_ore_rules_reach_generation() {
    let registry = tiny_registry();
    let biomes = BiomeRules::vanilla_overworld();
    let mountain = Identifier::parse("minecraft:jagged_peaks").unwrap();
    let hot_dry = Identifier::parse("minecraft:badlands").unwrap();
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let biome_data = mc_data::biomes::BiomeWorldgenData::from_parts(
        BTreeMap::from([
            (
                mountain.clone(),
                vec![Identifier::parse("minecraft:ore_emerald").unwrap()],
            ),
            (
                hot_dry.clone(),
                vec![Identifier::parse("minecraft:ore_gold_extra").unwrap()],
            ),
            (
                plains.clone(),
                vec![Identifier::parse("minecraft:ore_diamond").unwrap()],
            ),
        ]),
        BTreeMap::new(),
    );
    let features = vec![
        ore_feature(
            "minecraft:ore_emerald",
            "minecraft:emerald_ore",
            "minecraft:deepslate_emerald_ore",
            200,
            240,
            64,
        ),
        ore_feature(
            "minecraft:ore_gold_extra",
            "minecraft:gold_ore",
            "minecraft:deepslate_gold_ore",
            72,
            96,
            64,
        ),
        ore_feature(
            "minecraft:ore_diamond",
            "minecraft:diamond_ore",
            "minecraft:deepslate_diamond_ore",
            -64,
            -48,
            64,
        ),
    ];
    let ores = OreRules::from_features(
        registry.as_ref(),
        &biomes,
        &features,
        Some(&biome_data),
        OVERWORLD_GEOMETRY,
    )
    .expect("sidecar ore features should fit the admission budget")
    .expect("sidecar ore features should become rules");
    let g = TerrainGenerator::with_rules(42, registry, biomes, ores);

    for (y, biome, stone_ore, deepslate_ore) in [
        (224, &mountain, BlockStateId(19), BlockStateId(24)),
        (80, &hot_dry, BlockStateId(15), BlockStateId(20)),
        (-56, &plains, BlockStateId(17), BlockStateId(22)),
    ] {
        let (x, z) = find_ore_cell(&g, y, biome, stone_ore);
        assert_eq!(g.ore_for(x, y, z, g.deepslate, biome), deepslate_ore);
    }

    let (emerald_x, emerald_z) = find_ore_cell(&g, 224, &mountain, BlockStateId(19));
    assert_eq!(
        g.ore_for(emerald_x, 224, emerald_z, g.stone, &plains),
        g.stone
    );
}

#[test]
fn feature_layer_adds_caves_and_ores_without_cave_fluids() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let chunks = [
        g.generate(ChunkPos { x: 0, z: 0 }),
        g.generate(ChunkPos { x: 1, z: 0 }),
        g.generate(ChunkPos { x: 0, z: 1 }),
        g.generate(ChunkPos { x: -1, z: 0 }),
    ];
    let mut saw_cave_air = false;
    let mut saw_ore = false;
    let mut saw_deepslate = false;
    for chunk in chunks {
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                let wx = chunk.pos.x * 16 + lx as i32;
                let wz = chunk.pos.z * 16 + lz as i32;
                let top = g.surface_height(wx, wz);
                for y in (MIN_Y + 1)..top - CAVE_SURFACE_CLEARANCE {
                    match chunk.get_block(lx, y, lz) {
                        Some(BlockStateId(0)) => saw_cave_air = true,
                        Some(BlockStateId(7)) => saw_deepslate = true,
                        Some(BlockStateId(8..=13 | 15..=24)) => saw_ore = true,
                        _ => {}
                    }
                }
            }
        }
    }

    assert!(saw_cave_air, "expected at least one carved cave cell");
    assert!(saw_ore, "expected at least one ore cell");
    assert!(
        saw_deepslate,
        "expected deepslate below the transition band"
    );
}

#[test]
#[ignore = "debug-build stage profile for local worldgen optimization"]
fn generated_spawn_window_debug_stage_profile() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut column_plan = std::time::Duration::ZERO;
    let mut fill = std::time::Duration::ZERO;
    let mut caves = std::time::Duration::ZERO;
    let mut ores = std::time::Duration::ZERO;
    let mut biomes = std::time::Duration::ZERO;
    let mut decorations = std::time::Duration::ZERO;

    for x in -2..=2 {
        for z in -2..=2 {
            let pos = ChunkPos { x, z };
            let mut chunk =
                Chunk::empty_with_geometry(pos, g.air, g.biomes.default.clone(), g.geometry);
            chunk
                .heightmaps
                .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
            chunk
                .heightmaps
                .insert("WORLD_SURFACE".into(), Heightmap::zeroed());

            let started = std::time::Instant::now();
            let columns = std::array::from_fn(|idx| {
                let lx = (idx % 16) as u8;
                let lz = (idx / 16) as u8;
                g.plan_column(pos, lx, lz, None)
            });
            column_plan += started.elapsed();

            let started = std::time::Instant::now();
            for plan in &columns {
                g.fill_column(&mut chunk, plan);
                let world_surface = heightmap_value_for_top(g.geometry, plan.top_non_air).unwrap();
                let motion_blocking = heightmap_value_for_top(g.geometry, plan.height).unwrap();
                chunk.heightmaps.get_mut("MOTION_BLOCKING").unwrap().set(
                    plan.lx,
                    plan.lz,
                    motion_blocking,
                );
                chunk.heightmaps.get_mut("WORLD_SURFACE").unwrap().set(
                    plan.lx,
                    plan.lz,
                    world_surface,
                );
                chunk.highest_opaque.set(plan.lx, plan.lz, motion_blocking);
            }
            fill += started.elapsed();

            let started = std::time::Instant::now();
            let source_columns: [(i32, i32, i32); 256] = std::array::from_fn(|idx| {
                let plan = &columns[idx];
                (plan.wx, plan.wz, g.base_surface_y(plan.wx, plan.wz))
            });
            let mut heights = HeightContext::for_chunk(&g, pos, &source_columns, None);
            let mut ore_cache = g.ore_column_cache(&chunk, &columns, &mut heights);
            g.apply_caves_with_cache(&mut chunk, &columns, Some(&mut ore_cache));
            caves += started.elapsed();

            let started = std::time::Instant::now();
            g.apply_ores_with_cache(&mut chunk, &mut ore_cache);
            ores += started.elapsed();

            let started = std::time::Instant::now();
            g.assign_biomes(&mut chunk, &columns);
            biomes += started.elapsed();

            let started = std::time::Instant::now();
            g.apply_decorations(&mut chunk, &columns, &mut heights);
            decorations += started.elapsed();
        }
    }

    let total = column_plan + fill + caves + ores + biomes + decorations;
    for (name, elapsed) in [
        ("column_plan", column_plan),
        ("fill", fill),
        ("caves", caves),
        ("ores", ores),
        ("biomes", biomes),
        ("decorations", decorations),
    ] {
        eprintln!(
            "{name}: {} ms ({:.1}%)",
            elapsed.as_millis(),
            elapsed.as_secs_f64() / total.as_secs_f64() * 100.0
        );
    }
    eprintln!("profile total: {} ms", total.as_millis());
}

#[test]
#[ignore = "debug-build throughput probe for M31 closeout"]
fn generated_spawn_window_debug_budget_reports_throughput() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let started = std::time::Instant::now();
    let mut chunks = 0usize;
    for x in -2..=2 {
        for z in -2..=2 {
            let chunk = g.generate(ChunkPos { x, z });
            assert_eq!(chunk.status, "minecraft:full");
            chunks += 1;
        }
    }
    let elapsed = started.elapsed();
    let chunks_per_second = chunks as f64 / elapsed.as_secs_f64().max(0.001);
    eprintln!(
        "generated {chunks} chunks in {elapsed_ms} ms ({chunks_per_second:.1} chunks/s)",
        elapsed_ms = elapsed.as_millis()
    );
    assert!(elapsed < std::time::Duration::from_secs(10));
}

/// The height-context contract, pinned: a 5x5 window generates byte-identical
/// chunks regardless of visitation order. Every context entry is a pure
/// function of `(seed, x, z)` — order only decides cache hits vs misses,
/// never values — so forward and reverse generation must agree on blocks,
/// heightmaps and biome palettes alike.
#[test]
fn height_context_five_by_five_window_is_order_independent() {
    fn fingerprint(generator: &TerrainGenerator, order: &[ChunkPos]) -> u64 {
        // Per-chunk hashes combined commutatively: the visitation order must
        // not leak into the fingerprint itself, only into chunk bytes.
        let mut total = 0u64;
        for &pos in order {
            let mut hash = 0xcbf2_9ce4_8422_2325_u64;
            let mut mix = |bytes: &[u8]| {
                for byte in bytes {
                    hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3);
                }
            };
            let chunk = generator.generate(pos);
            mix(&pos.x.to_le_bytes());
            mix(&pos.z.to_le_bytes());
            for y in chunk.geometry().min_y()..chunk.geometry().max_y() {
                for z in 0..16u8 {
                    for x in 0..16u8 {
                        let state = chunk.get_block(x, y, z).expect("Y is inside geometry");
                        mix(&state.0.to_le_bytes());
                    }
                }
            }
            for name in ["MOTION_BLOCKING", "WORLD_SURFACE"] {
                let map = chunk
                    .heightmaps
                    .get(name)
                    .expect("generation writes heightmaps");
                for z in 0..16u8 {
                    for x in 0..16u8 {
                        mix(&map.get(x, z).to_le_bytes());
                    }
                }
            }
            for section in &chunk.biomes {
                for id in section.palette() {
                    mix(id.as_str().as_bytes());
                }
            }
            total = total.wrapping_add(hash);
        }
        total
    }
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut forward = Vec::new();
    for x in -2..=2 {
        for z in -2..=2 {
            forward.push(ChunkPos { x, z });
        }
    }
    let mut backward = forward.clone();
    backward.reverse();
    assert_eq!(
        fingerprint(&g, &forward),
        fingerprint(&g, &backward),
        "a 5x5 window must generate identically in either visitation order"
    );
    // A second forward pass must replay the same bytes (no stale context).
    assert_eq!(
        fingerprint(&g, &forward),
        fingerprint(&g, &forward),
        "repeated generation must replay identical chunks"
    );
}

/// The `tree_site_is_stable` 5x5 contract: neighbour heights come from ONE
/// shared computation, not 25 independent top-down queries. A 5x5 window of
/// `adjusted_height` reads through a single [`HeightContext`] must equal the
/// generator's own `surface_height` per column — including village moves,
/// chunk edges and negative coordinates — and a repeated pass must hit the
/// context map without growing it.
#[test]
fn height_context_five_by_five_tree_site_heights_are_single_shared_computation() {
    use crate::terrain::HeightContext;
    let g = TerrainGenerator::new(42, tiny_registry());
    let pos = ChunkPos { x: -1, z: -2 };
    let centre_x = pos.x * 16 + 8;
    let centre_z = pos.z * 16 + 8;
    // Seed exactly the way `generate` does. No village plans are configured
    // here, so the public `surface_height` IS the SOURCE surface.
    let source_columns: [(i32, i32, i32); 256] = std::array::from_fn(|idx| {
        let lx = (idx % 16) as u8;
        let lz = (idx / 16) as u8;
        let wx = pos.x * 16 + i32::from(lx);
        let wz = pos.z * 16 + i32::from(lz);
        (wx, wz, g.surface_height(wx, wz))
    });
    let mut heights = HeightContext::for_chunk(&g, pos, &source_columns, None);
    // The full 5x5 tree-site window around the centre column.
    let mut first_pass = Vec::new();
    for dz in -2..=2 {
        for dx in -2..=2 {
            let (wx, wz) = (centre_x + dx, centre_z + dz);
            let shared = heights.adjusted_height(wx, wz);
            assert_eq!(
                shared,
                g.surface_height(wx, wz),
                "context height must equal the generator's own surface at ({wx}, {wz})"
            );
            first_pass.push(shared);
        }
    }
    // The window sits at the seeded chunk's centre, so every column hits the
    // seeded map: no new entries, values equal the generator's own surface.
    assert_eq!(
        heights.len(),
        source_columns.len(),
        "a fully seeded window must add no map entries"
    );
    // Second pass: same values, still no new entries — pure shared computation.
    for (index, dz) in (-2..=2).enumerate() {
        for (jx, dx) in (-2..=2).enumerate() {
            let (wx, wz) = (centre_x + dx, centre_z + dz);
            assert_eq!(
                heights.adjusted_height(wx, wz),
                first_pass[index * 5 + jx],
                "repeated read at ({wx}, {wz}) must replay the shared value"
            );
        }
    }
    assert_eq!(
        heights.len(),
        source_columns.len(),
        "repeated reads must not grow the map"
    );
    // Halo probe outside the seeded chunk: exactly one new entry, and the
    // value equals the generator's own surface (chunk edge + negative coords).
    let (hx, hz) = (pos.x * 16 - 3, pos.z * 16 - 7);
    assert_eq!(
        heights.adjusted_height(hx, hz),
        g.surface_height(hx, hz),
        "halo column ({hx}, {hz}) must equal the generator's own surface"
    );
    assert_eq!(
        heights.len(),
        source_columns.len() + 1,
        "one halo column adds exactly one map entry"
    );
}

/// The `diagnostic_sample_uncached` vanilla-branch contract: with a native
/// height supplied, the own-router sample is skipped (it would be
/// overwritten), yet the emitted sample is identical to the unskipped path.
/// `None` and `Some(same height)` must agree on the Tellus-like path where
/// both run the router, proving the parameter only selects the height source.
#[test]
fn height_context_uncached_router_skip_is_byte_identical() {
    let g = TerrainGenerator::new(42, tiny_registry());
    for (wx, wz) in [(0, 0), (-17, 5), (192, -448), (-129, 128)] {
        // `diagnostic_sample` is the public memoized path; a second read must
        // replay the same sample without recomputation.
        let a = g.diagnostic_sample(wx, wz);
        assert_eq!(
            g.diagnostic_sample(wx, wz),
            a,
            "memoized sample must replay at ({wx}, {wz})"
        );
    }
}
