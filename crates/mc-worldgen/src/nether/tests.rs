use super::*;
use mc_data::blocks::{BlockReport, BlockStateReport};
use std::collections::BTreeMap;

fn nether_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    )
}

fn get(chunk: &Chunk, x: u8, y: i32, z: u8) -> BlockStateId {
    chunk
        .get_block(x, y, z)
        .expect("generated nether column is in-bounds")
}

#[test]
fn floor_and_ceiling_cap_every_column() {
    let generator = NetherGenerator::new(1234, nether_registry());
    let chunk = generator.generate(ChunkPos { x: 0, z: 0 });
    for x in 0..16u8 {
        for z in 0..16u8 {
            assert_eq!(get(&chunk, x, NETHER_MIN_Y, z), generator.bedrock);
            assert_eq!(get(&chunk, x, NETHER_CEILING_Y, z), generator.bedrock);
            for y in NETHER_CEILING_Y - NETHER_CEILING_BAND..NETHER_CEILING_Y {
                let state = get(&chunk, x, y, z);
                assert!(
                    state == generator.bedrock || state == generator.netherrack,
                    "ceiling band holds {state:?} at ({x}, {y}, {z})"
                );
            }
        }
    }
}

#[test]
fn body_keeps_lava_below_sea_and_roof_air_empty() {
    let generator = NetherGenerator::new(1234, nether_registry());
    let mut saw_lava = false;
    let mut saw_netherrack = false;
    for cx in -4..4 {
        let chunk = generator.generate(ChunkPos {
            x: cx * 7,
            z: cx * 11,
        });
        for x in 0..16u8 {
            for z in 0..16u8 {
                for y in 1..NETHER_CEILING_Y - NETHER_CEILING_BAND {
                    let state = get(&chunk, x, y, z);
                    if state == generator.lava {
                        assert!(y < NETHER_LAVA_LEVEL, "lava above sea level at {y}");
                        saw_lava = true;
                    }
                    saw_netherrack |= state == generator.netherrack;
                }
                for y in NETHER_CEILING_Y + 1..NETHER_MIN_Y + NETHER_HEIGHT {
                    assert_eq!(get(&chunk, x, y, z), generator.air);
                }
            }
        }
    }
    assert!(saw_lava, "lava never surfaces");
    assert!(saw_netherrack, "netherrack body never surfaces");
}

#[test]
fn generation_is_deterministic_per_chunk() {
    let generator = NetherGenerator::new(99, nether_registry());
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
    let twin = NetherGenerator::new(99, nether_registry());
    twin.generate(ChunkPos { x: -40, z: 12 });
    let second = generator.generate(pos);
    for x in 0..16u8 {
        for z in 0..16u8 {
            for y in NETHER_MIN_Y..NETHER_MIN_Y + NETHER_HEIGHT {
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
fn missing_netherrack_is_a_startup_error() {
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
        BlockReport {
            id: Identifier::parse("minecraft:lava").unwrap(),
            properties: BTreeMap::from([("level".to_string(), vec!["0".to_string()])]),
            states: vec![BlockStateReport {
                id: 2,
                default: true,
                properties: BTreeMap::from([("level".to_string(), "0".to_string())]),
            }],
        },
    ];
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    assert!(matches!(
        NetherGenerator::try_new(7, registry),
        Err(NetherGeneratorError::MissingRequiredBlock(
            "minecraft:netherrack"
        ))
    ));
}
