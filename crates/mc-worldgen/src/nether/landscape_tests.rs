use super::*;
use std::collections::BTreeSet;

fn registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    )
}

#[test]
fn all_five_nether_biomes_are_reachable() {
    let generator = NetherGenerator::new(1234, registry());
    let mut biomes = BTreeSet::new();
    for x in -12..=12 {
        for z in -12..=12 {
            biomes.insert(
                generator
                    .noise_biome(x * 128, 64, z * 128)
                    .unwrap()
                    .to_string(),
            );
        }
    }
    assert_eq!(
        biomes.len(),
        5,
        "Nether must not be a single wastes biome: {biomes:?}"
    );
}

#[test]
fn terrain_contains_stacked_caverns_not_one_height_field() {
    let generator = NetherGenerator::new(1234, registry());
    let mut stacked = false;
    for cx in -2..=2 {
        let chunk = generator.generate(ChunkPos {
            x: cx * 9,
            z: cx * 7,
        });
        for x in 0..16 {
            for z in 0..16 {
                let openings = (10..120)
                    .filter(|&y| {
                        chunk.get_block(x, y, z) == Some(generator.air)
                            && chunk.get_block(x, y - 1, z) != Some(generator.air)
                    })
                    .count();
                stacked |= openings >= 2;
            }
        }
    }
    assert!(stacked, "no column contains multiple cavern floors");
}
