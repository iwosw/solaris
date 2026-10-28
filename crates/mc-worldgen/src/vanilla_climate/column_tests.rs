use super::*;

#[test]
#[ignore = "requires the imported vanilla content cache"]
fn column_sampler_matches_stateless_climate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    for seed in [0, 81, 497625] {
        let climate = VanillaClimate::new(seed, &root).unwrap();
        let mut sampler = climate.column_sampler();
        for (x, z) in [(-17, -1), (-16, -32), (0, 0), (31, 48), (-17, -1)] {
            for y in (-64..320).step_by(4).chain([-1, 0, 1, -64]) {
                let expected = climate.sample(x, y, z);
                let actual = sampler.sample(x, y, z);
                assert_eq!(
                    actual.climate.map(f32::to_bits),
                    expected.climate.map(f32::to_bits),
                    "seed={seed}, position=({x},{y},{z})"
                );
                assert_eq!(actual.quantized, expected.quantized);
                assert_eq!(actual.biome, expected.biome);
            }
        }
    }
}
