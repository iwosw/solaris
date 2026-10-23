use super::*;
use mc_data::blocks::{BlockReport, BlockStateReport};
use std::collections::BTreeMap;

fn end_registry() -> Arc<BlockRegistry> {
    let state = |id: u32, properties: &[(&str, &str)]| BlockStateReport {
        id,
        default: true,
        properties: properties
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    let simple = |id: u32, name: &str| BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(id, &[])],
    };
    let report = vec![
        simple(0, "minecraft:air"),
        simple(1, "minecraft:bedrock"),
        simple(2, "minecraft:end_stone"),
        simple(3, "minecraft:obsidian"),
        BlockReport {
            id: Identifier::parse("minecraft:chorus_plant").unwrap(),
            properties: ["down", "up", "east", "west", "north", "south"]
                .into_iter()
                .map(|key| (key.into(), vec!["false".into(), "true".into()]))
                .collect(),
            states: vec![state(
                4,
                &[
                    ("down", "true"),
                    ("up", "true"),
                    ("east", "false"),
                    ("west", "false"),
                    ("north", "false"),
                    ("south", "false"),
                ],
            )],
        },
        simple(5, "minecraft:chorus_flower"),
        simple(6, "minecraft:purpur_block"),
        simple(7, "minecraft:end_stone_bricks"),
        simple(8, "minecraft:end_rod"),
    ];
    Arc::new(BlockRegistry::from_report(&report).unwrap())
}

fn get(chunk: &Chunk, x: u8, y: i32, z: u8) -> BlockStateId {
    chunk
        .get_block(x, y, z)
        .expect("generated end column is in-bounds")
}

#[test]
fn island_body_is_end_stone_with_void_at_edges() {
    let generator = EndGenerator::new(1234, end_registry());
    // Origin chunk sits on the island: every column is solid end stone from
    // the floor up to a surface, with only air or pillar blocks above.
    // Pillar shafts start above the island surface, so rock and pillar
    // blocks never mix in one row.
    let chunk = generator.generate(ChunkPos { x: 0, z: 0 });
    let mut saw_rock = false;
    for x in 0..16u8 {
        for z in 0..16u8 {
            let mut top_rock: Option<i32> = None;
            for y in END_MIN_Y..END_MIN_Y + END_HEIGHT {
                match get(&chunk, x, y, z) {
                    BlockStateId(2) => {
                        top_rock = Some(y);
                        saw_rock = true;
                    }
                    BlockStateId(0) | BlockStateId(1) | BlockStateId(3) => {}
                    other => panic!("unexpected end body block {other:?} at {y}"),
                }
            }
            if let Some(top) = top_rock {
                for y in END_MIN_Y..=top {
                    assert_eq!(
                        get(&chunk, x, y, z),
                        BlockStateId(2),
                        "island column ({x}, {z}) has a hole at {y}"
                    );
                }
                for y in top + 1..END_MIN_Y + END_HEIGHT {
                    assert!(
                        get(&chunk, x, y, z) != BlockStateId(2),
                        "end stone floats above surface at ({x}, {y}, {z})"
                    );
                }
            }
        }
    }
    assert!(saw_rock, "origin island has no end stone");
    // The gap between the central island and outer islands stays void.
    let far = generator.generate(ChunkPos { x: 24, z: -24 });
    for x in 0..16u8 {
        for z in 0..16u8 {
            for y in END_MIN_Y..END_MIN_Y + END_HEIGHT {
                assert_eq!(get(&far, x, y, z), BlockStateId(0));
            }
        }
    }
}

#[test]
fn pillar_ring_stands_near_origin_with_bedrock_caps() {
    let generator = EndGenerator::new(1234, end_registry());
    let mut saw_obsidian = false;
    let mut column_tops = Vec::new();
    // Ring radius 42 plus the 3-wide footprint fits inside ±3 chunks.
    for cx in -3..3 {
        for cz in -3..3 {
            let chunk = generator.generate(ChunkPos { x: cx, z: cz });
            for x in 0..16u8 {
                for z in 0..16u8 {
                    let mut top_obsidian: Option<i32> = None;
                    for y in END_MIN_Y..END_MIN_Y + END_HEIGHT {
                        match get(&chunk, x, y, z) {
                            BlockStateId(3) => {
                                top_obsidian = Some(y);
                                saw_obsidian = true;
                            }
                            BlockStateId(1) => {
                                // Every bedrock block is a pillar cap, so the
                                // block directly below it is shaft obsidian.
                                assert_eq!(
                                    get(&chunk, x, y - 1, z),
                                    BlockStateId(3),
                                    "bedrock without shaft below at ({x}, {y}, {z})"
                                );
                            }
                            BlockStateId(0) | BlockStateId(2) => {}
                            other => panic!("unexpected end pillar block {other:?} at {y}"),
                        }
                    }
                    if let Some(top) = top_obsidian {
                        column_tops.push(top);
                        // Each shaft is capped: bedrock directly above the top
                        // obsidian row.
                        assert_eq!(
                            get(&chunk, x, top + 1, z),
                            BlockStateId(1),
                            "pillar shaft uncapped at ({x}, {top}, {z})"
                        );
                    }
                }
            }
        }
    }
    assert!(saw_obsidian, "no obsidian pillars near the origin");
    assert!(
        column_tops.len() >= END_PILLAR_COUNT,
        "pillar ring under-populated: {} shaft columns",
        column_tops.len()
    );
}

#[test]
fn generation_is_deterministic_per_chunk() {
    let generator = EndGenerator::new(99, end_registry());
    let pos = ChunkPos { x: 3, z: -7 };
    let first = generator.generate(pos);
    // Interleave surrounding chunks and a second same-seed generator before
    // regenerating the target: shared-state contamination would show here.
    for dx in -1..=1 {
        for dz in -1..=1 {
            if dx != 0 || dz != 0 {
                generator.generate(ChunkPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                });
            }
        }
    }
    let twin = EndGenerator::new(99, end_registry());
    twin.generate(ChunkPos { x: -40, z: 12 });
    let second = generator.generate(pos);
    for x in 0..16u8 {
        for z in 0..16u8 {
            for y in END_MIN_Y..END_MIN_Y + END_HEIGHT {
                assert_eq!(
                    get(&first, x, y, z),
                    get(&second, x, y, z),
                    "chunk order changed column ({x}, {z}) at {y}"
                );
            }
        }
    }
}

#[test]
fn missing_end_stone_is_a_startup_error() {
    let state = |id: u32| BlockStateReport {
        id,
        default: true,
        properties: BTreeMap::new(),
    };
    let simple = |id: u32, name: &str| BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(id)],
    };
    let report = vec![
        simple(0, "minecraft:air"),
        simple(1, "minecraft:bedrock"),
        simple(2, "minecraft:obsidian"),
    ];
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    assert!(matches!(
        EndGenerator::try_new(7, registry),
        Err(EndGeneratorError::MissingRequiredBlock(
            "minecraft:end_stone"
        ))
    ));
}

#[test]
fn outer_end_has_land_and_void_without_filling_the_void_floor() {
    let generator = EndGenerator::new(1234, end_registry());
    let mut land = 0;
    let mut void = 0;
    for x in (1024..2048).step_by(16) {
        for z in (1024..1536).step_by(16) {
            let y = generator.surface_height(x, z);
            if y >= 0 {
                land += 1;
            } else {
                void += 1;
            }
        }
    }
    assert!(land > 100, "outer End is still entirely void");
    assert!(void > 100, "outer islands must be separated by void");
}

fn template_generator(seed: i64) -> EndGenerator {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    EndGenerator::new(seed, Arc::clone(&registry))
        .with_vanilla_data(&root, &registry)
        .unwrap()
}

#[test]
fn outer_end_city_ship_and_chorus_are_chunk_order_independent() {
    let generator = template_generator(1234);
    let (x, _, z, _) = (8..24)
        .flat_map(|gx| (8..24).map(move |gz| (gx, gz)))
        .filter_map(|(gx, gz)| generator.city(gx, gz))
        .find(|(_, _, _, h)| {
            let branches: [u64; 4] =
                std::array::from_fn(|side| splitmix64(*h ^ (side as u64).wrapping_add(0xB41D_6E)));
            branches.iter().filter(|b| **b & 1 != 0).count() >= 2
                && branches.iter().any(|b| *b & 1 != 0 && (b >> 1) % 8 == 0)
        })
        .expect("ship city");
    let mut entities = Vec::new();
    let mut chorus = 0;
    let mut treasures = 0;
    for cx in (x - 96).div_euclid(16)..=(x + 96).div_euclid(16) {
        for cz in (z - 96).div_euclid(16)..=(z + 96).div_euclid(16) {
            let pos = ChunkPos { x: cx, z: cz };
            let chunk = generator.generate(pos);
            let again = generator.generate(pos);
            assert_eq!(chunk.generated_entities(), again.generated_entities());
            entities.extend(chunk.generated_entities());
            for chest in chunk.chests.values() {
                assert!(chest.slots.iter().all(|slot| slot.count == 0));
                if chest
                    .loot_table
                    .as_ref()
                    .is_some_and(|id| id.as_str() == "minecraft:chests/end_city_treasure")
                {
                    treasures += 1;
                }
            }
            for qx in 0..4 {
                for qz in 0..4 {
                    assert_eq!(
                        chunk.biomes[0].get(qx, 0, qz),
                        &generator
                            .noise_biome(
                                cx * 16 + i32::from(qx) * 4,
                                0,
                                cz * 16 + i32::from(qz) * 4
                            )
                            .unwrap(),
                    );
                }
            }
            for lx in 0..16 {
                for lz in 0..16 {
                    assert_eq!(
                        chunk.get_block(lx, 0, lz),
                        Some(generator.air),
                        "outer islands float above void"
                    );
                    for by in 48..100 {
                        chorus += usize::from(
                            chunk.get_block(lx, by, lz) == Some(generator.outer.flower),
                        );
                    }
                }
            }
        }
    }
    assert!(chorus > 0);
    assert!(
        treasures >= 4,
        "treasure tower and ship chests retain their lazy loot tables"
    );
    assert!(
        entities
            .iter()
            .filter(|m| m.entity_type == "minecraft:shulker")
            .count()
            >= 4
    );
    let frame = entities
        .iter()
        .find(|m| m.entity_type == "minecraft:item_frame")
        .expect("ship elytra");
    let frame_chunk = generator.generate(ChunkPos {
        x: (frame.position[0].floor() as i32).div_euclid(16),
        z: (frame.position[2].floor() as i32).div_euclid(16),
    });
    let fx = (frame.position[0].floor() as i32).rem_euclid(16) as u8;
    let fz = (frame.position[2].floor() as i32).rem_euclid(16) as u8;
    let fy = frame.position[1] as i32;
    assert_eq!(frame_chunk.get_block(fx, fy, fz), Some(generator.air));
    let (dx, dz) = [(0, -1), (1, 0), (0, 1), (-1, 0)]
        [((frame.yaw / 90.0).round() as i32).rem_euclid(4) as usize];
    let sx = i32::from(fx) + dx;
    let sz = i32::from(fz) + dz;
    assert!(
        (0..16).contains(&sx) && (0..16).contains(&sz),
        "support must be in frame chunk"
    );
    assert_ne!(
        frame_chunk.get_block(sx as u8, fy, sz as u8),
        Some(generator.air)
    );
    let mut claims = std::collections::BTreeSet::new();
    assert!(
        entities.iter().all(|m| claims.insert(&m.claim)),
        "no duplicated border inhabitants"
    );
}

#[test]
fn end_city_site_query_matches_placement_and_respects_bounds() {
    let generator = template_generator(42);
    let sites = generator.structure_sites_in_region((64, 64), (256, 256));
    assert!(!sites.is_empty());
    for site in sites {
        assert_eq!(site.structure.as_str(), "minecraft:end_city");
        let narrowed = generator.structure_sites_in_region(site.start_chunk, site.start_chunk);
        assert_eq!(narrowed, [site.clone()]);
        let expected =
            generator
                .outer
                .placement
                .potential_chunk(42, site.start_chunk.0, site.start_chunk.1);
        assert_eq!(expected, site.start_chunk);
        let chunk = generator.generate(ChunkPos {
            x: site.start_chunk.0,
            z: site.start_chunk.1,
        });
        assert_eq!(
            mc_world::structure_starts::read(&chunk.extras, chunk.pos, false).unwrap(),
            [site],
            "saved starts retain the same locator anchor as generation"
        );
        assert!(
            !chunk.generated_entities().is_empty(),
            "template sentries at advertised city"
        );
    }
    assert!(
        generator
            .structure_sites_in_region((20, 20), (10, 10))
            .is_empty()
    );
}

#[test]
fn natural_return_gateway_has_imported_destination_and_border_frame() {
    use mc_nbt::Tag;
    let generator = template_generator(42);
    let pos = (64_i32..512)
        .flat_map(|x| (64_i32..512).map(move |z| ChunkPos { x, z }))
        .find_map(|pos| {
            let h = splitmix64(
                42_u64
                    ^ (pos.x as u64).wrapping_mul(0xF13C_7AF1)
                    ^ (pos.z as u64).wrapping_mul(0x5B79_2A91),
            );
            if h % 700 != 0 {
                return None;
            }
            let chunk = generator.generate(pos);
            chunk.block_entities.iter().find_map(|(at, bytes)| {
                let Tag::Compound(fields) = mc_nbt::read_network(&mut bytes.as_slice()).unwrap()
                else {
                    return None;
                };
                fields
                    .iter()
                    .any(|(key, value)| {
                        key == "id"
                            && matches!(value,Tag::String(id) if id=="minecraft:end_gateway")
                    })
                    .then_some((*at, fields))
            })
        })
        .expect("natural outer return gateway");
    let (at, fields) = pos;
    assert!(fields.contains(&("exit_portal".into(), Tag::IntArray(vec![100, 50, 0]))));
    assert!(fields.contains(&("ExactTeleport".into(), Tag::Byte(1))));
    for dy in [-2, 2] {
        let chunk = generator.generate(ChunkPos {
            x: at.x.div_euclid(16),
            z: at.z.div_euclid(16),
        });
        assert_eq!(
            chunk.get_block(
                at.x.rem_euclid(16) as u8,
                at.y + dy,
                at.z.rem_euclid(16) as u8
            ),
            Some(generator.bedrock)
        );
    }
}

#[test]
fn city_resource_load_fails_closed_on_missing_import() {
    let generator = EndGenerator::new(42, end_registry());
    let root = tempfile::tempdir().unwrap();
    assert!(
        generator
            .with_vanilla_data(root.path(), &end_registry())
            .is_err()
    );
}

#[test]
#[ignore = "release-only approximate microbenchmark; ordinary memory-gated lane"]
fn outer_end_generation_microbenchmark() {
    let generator = template_generator(1234);
    let (x, _, z, _) = (8..24)
        .flat_map(|gx| (8..24).map(move |gz| (gx, gz)))
        .find_map(|(gx, gz)| generator.city(gx, gz))
        .expect("benchmark city");
    let mut samples = [0_u128; 5];
    for sample in &mut samples {
        let start = std::time::Instant::now();
        for dx in -3..=3 {
            for dz in -3..=3 {
                std::hint::black_box(generator.generate(ChunkPos {
                    x: x.div_euclid(16) + dx,
                    z: z.div_euclid(16) + dz,
                }));
            }
        }
        *sample = start.elapsed().as_nanos() / 49;
    }
    println!("approximate, contended machine: End generation ns/chunk samples={samples:?}");
    samples.sort_unstable();
    println!(
        "approximate, contended machine: End generation median_ns_per_chunk={}",
        samples[2]
    );
}
