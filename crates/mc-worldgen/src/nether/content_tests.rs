use super::*;
use mc_world::BlockPos;
use std::sync::LazyLock;

fn generator() -> &'static NetherGenerator {
    static GENERATOR: LazyLock<NetherGenerator> = LazyLock::new(|| {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
        let blocks = Arc::new(
            BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
        );
        let data = Arc::new(
            mc_data::load(&root)
                .expect("local vanilla cache required for Nether content acceptance"),
        );
        let tags = Arc::new(mc_data::tags::load(&root, &data).unwrap());
        let tags = Arc::new(crate::vanilla_features::CacheTags::new(
            blocks.clone(),
            data,
            tags,
        ));
        let mut generator = NetherGenerator::new(1234, blocks);
        generator.load_vanilla_worldgen(&root, tags).unwrap();
        generator
    });
    &GENERATOR
}

fn state(generator: &NetherGenerator, name: &'static str) -> BlockStateId {
    try_resolve(&generator.registry, name).unwrap()
}

#[test]
#[ignore = "requires the imported vanilla content cache"]
fn nether_ores_generate_and_debris_is_buried() {
    let generator = generator();
    let states = [
        "minecraft:nether_quartz_ore",
        "minecraft:nether_gold_ore",
        "minecraft:ancient_debris",
    ]
    .map(|name| state(generator, name));
    let mut counts = [0; 3];
    for cx in -3..=3 {
        for cz in -3..=3 {
            let pos = ChunkPos { x: cx, z: cz };
            let mut chunk = Chunk::empty_with_geometry(
                pos,
                generator.air,
                generator.biomes[0].clone(),
                generator.geometry,
            );
            generator.fill_landscape(&mut chunk);
            generator.apply_ores(&mut chunk);
            for x in 0..16u8 {
                for z in 0..16u8 {
                    for y in 1..123 {
                        let block = chunk.get_block(x, y, z).unwrap();
                        if let Some(i) = states.iter().position(|s| *s == block) {
                            counts[i] += 1;
                            if i == 2 {
                                assert!((1..=122).contains(&y));
                                let wx = cx * 16 + i32::from(x);
                                let wz = cz * 16 + i32::from(z);
                                for (dx, dy, dz) in [
                                    (1, 0, 0),
                                    (-1, 0, 0),
                                    (0, 1, 0),
                                    (0, -1, 0),
                                    (0, 0, 1),
                                    (0, 0, -1),
                                ] {
                                    assert!(
                                        NetherGenerator::solid(
                                            &generator.density_column(wx + dx, wz + dz),
                                            y + dy
                                        ),
                                        "exposed debris at {wx},{y},{wz}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(counts.iter().all(|n| *n > 0), "ore counts {counts:?}");
}

#[test]
#[ignore = "requires the imported vanilla content cache"]
fn all_biome_landscapes_have_their_resources() {
    let generator = generator();
    let groups = [
        vec![
            state(generator, "minecraft:crimson_nylium"),
            state(generator, "minecraft:crimson_stem"),
        ],
        vec![
            state(generator, "minecraft:warped_nylium"),
            state(generator, "minecraft:warped_stem"),
        ],
        vec![
            state(generator, "minecraft:soul_sand"),
            state(generator, "minecraft:soul_soil"),
        ],
        vec![
            state(generator, "minecraft:basalt"),
            state(generator, "minecraft:blackstone"),
        ],
    ];
    for (i, group) in groups.iter().enumerate() {
        let mut seen = vec![false; group.len()];
        let mut samples = 0;
        'search: for cx in -15..=15 {
            for cz in -15..=15 {
                let pos = ChunkPos {
                    x: cx * 7,
                    z: cz * 7,
                };
                if generator.biome_index(pos.x * 16 + 8, pos.z * 16 + 8) != i + 1 {
                    continue;
                }
                let mut chunk = Chunk::empty_with_geometry(
                    pos,
                    generator.air,
                    generator.biomes[0].clone(),
                    generator.geometry,
                );
                generator.fill_landscape(&mut chunk);
                generator.decorate(&mut chunk);
                for x in 0..16 {
                    for z in 0..16 {
                        for y in 32..123 {
                            if let Some(j) = group
                                .iter()
                                .position(|s| Some(*s) == chunk.get_block(x, y, z))
                            {
                                seen[j] = true;
                            }
                        }
                    }
                }
                samples += 1;
                if seen.iter().all(|b| *b) || samples >= 20 {
                    break 'search;
                }
            }
        }
        assert!(
            seen.iter().all(|b| *b),
            "biome {} missing resources {seen:?}",
            i + 1
        );
    }
}

#[test]
#[ignore = "requires the imported vanilla content cache"]
fn fortress_starts_place_blaze_spawners_wart_and_loot_across_chunks() {
    let generator = generator();
    let sites = generator.structure_sites_in_region((-70, -70), (70, 70));
    let site = sites
        .iter()
        .find(|s| s.structure.path() == "fortress")
        .expect("generated fortress");
    let origin = site.position;
    let spawner = BlockPos {
        x: origin.x + 27,
        y: 66,
        z: origin.z,
    };
    let chunk = generator.generate(ChunkPos {
        x: spawner.x.div_euclid(16),
        z: spawner.z.div_euclid(16),
    });
    assert_eq!(
        chunk.get_block(
            spawner.x.rem_euclid(16) as u8,
            spawner.y,
            spawner.z.rem_euclid(16) as u8
        ),
        Some(state(generator, "minecraft:spawner"))
    );
    let bytes = chunk
        .block_entities
        .get(&spawner)
        .expect("native spawner NBT");
    let nbt = mc_nbt::read_network(&mut bytes.as_slice()).unwrap();
    assert!(format!("{nbt:?}").contains("minecraft:blaze"));
    let garden = BlockPos {
        x: origin.x - 27,
        y: 65,
        z: origin.z + 1,
    };
    let garden_chunk = generator.generate(ChunkPos {
        x: garden.x.div_euclid(16),
        z: garden.z.div_euclid(16),
    });
    assert_eq!(
        garden_chunk.get_block(
            garden.x.rem_euclid(16) as u8,
            garden.y,
            garden.z.rem_euclid(16) as u8
        ),
        Some(state(generator, "minecraft:nether_wart"))
    );
    let chest = BlockPos {
        x: origin.x,
        y: 65,
        z: origin.z + 27,
    };
    let chest_chunk = generator.generate(ChunkPos {
        x: chest.x.div_euclid(16),
        z: chest.z.div_euclid(16),
    });
    let contents = chest_chunk.chests.get(&chest).unwrap();
    assert!(contents.slots.iter().all(|slot| slot.count == 0));
    assert_eq!(
        contents.loot_table.as_ref().unwrap().as_str(),
        "minecraft:chests/nether_bridge"
    );
    let again = generator.generate(chunk.pos);
    assert_eq!(chunk.block_entities, again.block_entities);
    for x in 0..16 {
        for z in 0..16 {
            for y in 0..128 {
                assert_eq!(chunk.get_block(x, y, z), again.get_block(x, y, z));
            }
        }
    }
}

#[test]
#[ignore = "requires the imported vanilla content cache"]
fn bastion_jigsaw_fossils_and_ruined_portals_are_natural_starts() {
    let generator = generator();
    let sites = generator.structure_sites_in_region((-100, -100), (100, 100));
    for (name, block) in [
        ("bastion_remnant", "minecraft:polished_blackstone_bricks"),
        ("nether_fossil", "minecraft:bone_block"),
        ("ruined_portal_nether", "minecraft:obsidian"),
    ] {
        let site = sites
            .iter()
            .find(|s| s.structure.path() == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        let expected = state(generator, block);
        let mut found = false;
        'chunks: for dx in -2..=2 {
            for dz in -2..=2 {
                let chunk = generator.generate(ChunkPos {
                    x: site.position.x.div_euclid(16) + dx,
                    z: site.position.z.div_euclid(16) + dz,
                });
                for x in 0..16 {
                    for z in 0..16 {
                        for y in 1..123 {
                            if chunk.get_block(x, y, z) == Some(expected) {
                                found = true;
                                break 'chunks;
                            }
                        }
                    }
                }
            }
        }
        assert!(found, "located {name} had no {block}");
    }
    let structures = generator.structures.as_ref().unwrap();
    let site = sites
        .iter()
        .find(|s| s.structure.path() == "bastion_remnant")
        .unwrap();
    let plan = structures
        .bastions
        .plan_for_start_chunk(
            site.start_chunk.0,
            site.start_chunk.1,
            &|_, _| 33,
            &|x, _, z| generator.biomes[generator.biome_index(x, z)].clone(),
        )
        .unwrap()
        .pieces()
        .len();
    assert!(
        plan > 1,
        "bastion must grow connected jigsaw pieces rather than stamp one start room"
    );
}

#[test]
fn coarse_density_matches_scalar_at_chunk_edges() {
    let blocks = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let generator = NetherGenerator::new(3456, blocks);
    for cx in [-7, -1, 0, 8] {
        let pos = ChunkPos { x: cx, z: -cx };
        let mut chunk = Chunk::empty_with_geometry(
            pos,
            generator.air,
            generator.biomes[0].clone(),
            generator.geometry,
        );
        generator.fill_landscape(&mut chunk);
        for x in [0, 1, 14, 15] {
            for z in [0, 1, 14, 15] {
                for y in 1..123 {
                    let block = chunk.get_block(x, y, z).unwrap();
                    assert_eq!(
                        block != generator.air && block != generator.lava,
                        generator.solid_at(cx * 16 + i32::from(x), y, -cx * 16 + i32::from(z))
                    );
                }
            }
        }
    }
}

#[test]
fn hanging_glowstone_is_generated() {
    let blocks = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let generator = NetherGenerator::new(3456, blocks);
    let mut found = false;
    for cx in -4..=4 {
        let chunk = generator.generate(ChunkPos {
            x: cx * 9,
            z: cx * 5,
        });
        for x in 0..16 {
            for z in 0..16 {
                for y in 32..123 {
                    found |= chunk.get_block(x, y, z) == Some(generator.palette.glowstone);
                }
            }
        }
    }
    assert!(found, "no hanging glowstone clusters generated");
}

#[test]
#[ignore = "requires the imported vanilla content cache"]
fn all_four_bastion_types_expand_and_emit_inhabitants() {
    let generator = generator();
    let source = &generator.structures.as_ref().unwrap().bastions;
    let placement = source.closure().placement;
    let mut families = std::collections::BTreeSet::new();
    let mut inhabitants = std::collections::BTreeSet::new();
    for gx in -5..=5 {
        for gz in -5..=5 {
            let start = placement.potential_chunk(
                generator.seed,
                gx * placement.spacing,
                gz * placement.spacing,
            );
            let Some(plan) =
                source.plan_for_start_chunk(start.0, start.1, &|_, _| 33, &|x, _, z| {
                    generator.biomes[generator.biome_index(x, z)].clone()
                })
            else {
                continue;
            };
            let crate::village::plan_source::PlanElement::Single { template, .. } =
                &plan.pieces()[0].element
            else {
                panic!("bastion start must be a template")
            };
            let family = template.path().split('/').nth(1).unwrap().to_string();
            if !families.insert(family) {
                continue;
            }
            assert!(plan.pieces().len() > 1);
            let plans =
                crate::village::plan_source::VillagePlanSet::from_plans(vec![Arc::new(plan)])
                    .unwrap();
            let (min, max) = plans.affected_box();
            let radius = source.neighbourhood_chunk_radius();
            for x in min.x.div_euclid(16)..=max.x.div_euclid(16) {
                for z in min.z.div_euclid(16)..=max.z.div_euclid(16) {
                    assert!((x - start.0).abs() <= radius && (z - start.1).abs() <= radius);
                    let pos = ChunkPos { x, z };
                    if !plans.affects_chunk(pos.x, pos.z) {
                        continue;
                    }
                    let mut chunk = Chunk::empty_with_geometry(
                        pos,
                        generator.air,
                        generator.biomes[0].clone(),
                        generator.geometry,
                    );
                    source
                        .write_plans(
                            &mut chunk,
                            &plans,
                            &mut [false; 256],
                            &|_, _| 33,
                            generator.air,
                        )
                        .unwrap();
                    for marker in chunk.generated_entities() {
                        inhabitants.insert(marker.entity_type);
                    }
                }
            }
            if families.len() == 4 {
                break;
            }
        }
        if families.len() == 4 {
            break;
        }
    }
    assert_eq!(families.len(), 4, "bastion variants {families:?}");
    assert!(inhabitants.contains("minecraft:piglin"), "{inhabitants:?}");
    assert!(
        inhabitants.contains("minecraft:piglin_brute"),
        "{inhabitants:?}"
    );
}

#[test]
#[ignore = "requires the imported vanilla content cache"]
fn generated_start_metadata_preserves_locate_anchor_and_bounds() {
    use mc_nbt::Tag;
    let generator = generator();
    let sites = generator.structure_sites_in_region((-100, -100), (100, 100));
    for name in [
        "fortress",
        "bastion_remnant",
        "nether_fossil",
        "ruined_portal_nether",
    ] {
        let site = sites
            .iter()
            .find(|site| site.structure.path() == name)
            .unwrap();
        let chunk = generator.generate(ChunkPos {
            x: site.start_chunk.0,
            z: site.start_chunk.1,
        });
        let Tag::Compound(structures) = &chunk
            .extras
            .iter()
            .find(|(key, _)| key == "structures")
            .unwrap()
            .1
        else {
            panic!("missing structures");
        };
        let Tag::Compound(starts) = &structures
            .iter()
            .find(|(key, _)| key == "starts")
            .unwrap()
            .1
        else {
            panic!("missing starts");
        };
        let Tag::Compound(start) = &starts
            .iter()
            .find(|(key, _)| key == site.structure.as_str())
            .unwrap()
            .1
        else {
            panic!("missing start");
        };
        assert_eq!(
            start
                .iter()
                .find(|(key, _)| key == "SolarisLocatePos")
                .unwrap()
                .1,
            Tag::IntArray(vec![site.position.x, site.position.y, site.position.z])
        );
        let Tag::IntArray(bounds) = &start.iter().find(|(key, _)| key == "BB").unwrap().1 else {
            panic!("missing bounds");
        };
        assert!(bounds[0] < bounds[3] && bounds[1] < bounds[4] && bounds[2] < bounds[5]);
        assert!(bounds[1] >= 0 && bounds[4] < 128);
        let expected = Tag::Compound(chunk.extras);
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &expected).unwrap();
        assert_eq!(
            mc_nbt::read_network(&mut bytes.as_slice()).unwrap(),
            expected
        );
    }
}
