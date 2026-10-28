use super::*;

#[test]
fn deepslate_gradient_uses_vanilla_zero_to_eight_bounds() {
    let generator = TerrainGenerator::new(81, tests::tiny_registry());
    for pos in [ChunkPos { x: -1, z: 1 }, ChunkPos { x: 0, z: 0 }] {
        for x in 0..16 {
            for z in 0..16 {
                for y in [-64, -8, -1, 0] {
                    assert_eq!(
                        generator.base_stone_for_y(x, y, z, pos),
                        generator.deepslate
                    );
                }
                for y in [8, 9, 64] {
                    assert_eq!(generator.base_stone_for_y(x, y, z, pos), generator.stone);
                }
            }
        }
    }
}

#[test]
fn deepslate_gradient_probability_is_linear_not_reciprocal() {
    let generator = TerrainGenerator::new(81, tests::tiny_registry());
    let pos = ChunkPos { x: -2, z: 3 };
    for x in 0..16 {
        for z in 0..16 {
            for y in 1..8 {
                let wx = world_block_coordinate(pos.x, x);
                let wz = world_block_coordinate(pos.z, z);
                let expected = if feature_hash(81, wx, y, wz, 0xD33F) % 8 < (8 - y) as u64 {
                    generator.deepslate
                } else {
                    generator.stone
                };
                assert_eq!(generator.base_stone_for_y(x, y, z, pos), expected);
            }
        }
    }
}
