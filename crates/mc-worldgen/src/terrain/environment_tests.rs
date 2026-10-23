use super::*;

#[test]
fn badlands_subsurface_is_terracotta_not_red_sand() {
    let blocks = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let generator = TerrainGenerator::new(42, Arc::clone(&blocks));
    for name in [
        "minecraft:badlands",
        "minecraft:eroded_badlands",
        "minecraft:wooded_badlands",
    ] {
        let (surface, fill) = generator.surface_materials(&Identifier::parse(name).unwrap());
        assert_eq!(
            surface,
            blocks
                .block(&Identifier::parse("minecraft:red_sand").unwrap())
                .unwrap()
                .default
        );
        assert_eq!(
            fill,
            blocks
                .block(&Identifier::parse("minecraft:terracotta").unwrap())
                .unwrap()
                .default
        );
    }
    assert!(
        generator
            .terracotta_layers
            .windows(2)
            .any(|pair| pair[0] != pair[1])
    );
}

#[test]
fn native_deep_cavities_are_not_dry_air() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    if !root.exists() {
        eprintln!("SKIP local vanilla assets unavailable");
        return;
    }
    let blocks = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let mut generator = TerrainGenerator::new(42, blocks);
    generator.load_vanilla_worldgen(&root).unwrap();
    let chunk = generator.generate(ChunkPos { x: 0, z: 0 });
    for y in -63..=-54 {
        for x in 0..16 {
            for z in 0..16 {
                assert_ne!(
                    chunk.get_block(x, y, z),
                    Some(generator.air),
                    "dry cavity at {x},{y},{z}"
                );
            }
        }
    }
}

#[cfg(not(debug_assertions))]
#[test]
#[ignore = "approximate release A/B microbenchmark; run only on owner authorization"]
fn overworld_structure_generation_approximate_ab() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let blocks = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let mut generator = TerrainGenerator::new(42, blocks);
    generator.load_vanilla_worldgen(&root).unwrap();
    let positions = [(0, 0), (11, -7), (-19, 25), (41, 33)];
    let mut structures = generator.overworld_structures.take();
    let mut baseline = [0u128; 7];
    let mut enabled = [0u128; 7];
    for round in 0..7 {
        for mode in [round % 2, 1 - round % 2] {
            if mode == 1 {
                generator.overworld_structures = structures.take();
            }
            let start = std::time::Instant::now();
            for &(x, z) in &positions {
                std::hint::black_box(generator.generate(ChunkPos { x, z }));
            }
            let nanos = start.elapsed().as_nanos() / positions.len() as u128;
            if mode == 1 {
                enabled[round] = nanos;
                structures = generator.overworld_structures.take();
            } else {
                baseline[round] = nanos;
            }
        }
    }
    eprintln!(
        "approximate contended-machine ns/chunk baseline={baseline:?} structures={enabled:?}"
    );
    baseline.sort_unstable();
    enabled.sort_unstable();
    eprintln!(
        "approximate median ns/chunk baseline={} structures={} ratio={:.3}",
        baseline[3],
        enabled[3],
        enabled[3] as f64 / baseline[3] as f64
    );
}
