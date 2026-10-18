use super::{SEA_LEVEL, TerrainGenerator, tests::tiny_registry};
use mc_data::Identifier;

#[test]
fn coastal_variants_follow_temperature() {
    let g = TerrainGenerator::new(712_816, tiny_registry());
    let mut sample = g.density_router().sample(0, 0);
    sample.continentalness = -0.01;
    sample.river = 1.0;
    sample.wetland = 0.0;
    sample.ridges = 0.0;
    sample.erosion = 0.5;
    sample.climate_domain = 0.0;
    for (temperature, ocean, deep, shore) in [
        (-0.4, "frozen_ocean", "deep_frozen_ocean", "snowy_beach"),
        (-0.2, "cold_ocean", "deep_cold_ocean", "beach"),
        (0.0, "ocean", "deep_ocean", "beach"),
        (0.2, "lukewarm_ocean", "deep_lukewarm_ocean", "beach"),
        (0.4, "warm_ocean", "deep_lukewarm_ocean", "beach"),
    ] {
        sample.temperature = temperature;
        for (height, expected) in [
            (SEA_LEVEL - 30, deep),
            (SEA_LEVEL - 3, ocean),
            (SEA_LEVEL + 1, shore),
        ] {
            let (x, z) = (-448, -32);
            let biome = g.vanilla_biome_for(x, z, height, sample);
            assert_eq!(
                biome.path(),
                expected,
                "temperature {temperature}, height {height}"
            );
        }
    }
}

#[test]
fn rocky_shore_has_rocky_ground_but_raised_bank_is_not_beach() {
    let g = TerrainGenerator::new(712_816, tiny_registry());
    let mut sample = g.density_router().sample(-448, -32);
    sample.continentalness = 0.0;
    sample.temperature = 0.3;
    sample.erosion = 0.2;
    sample.river = 1.0;
    sample.wetland = 0.0;
    let shore = g.vanilla_biome_for(-448, -32, SEA_LEVEL + 1, sample);
    assert_eq!(shore.path(), "stony_shore");
    assert_eq!(g.surface_materials(&shore), (g.gravel, g.stone));
    let bank = g.vanilla_biome_for(-448, -32, SEA_LEVEL + 3, sample);
    assert!(!g.biomes.is_beach_or_shore(&bank));
    let beach = Identifier::parse("minecraft:beach").unwrap();
    assert_eq!(g.surface_materials(&beach), (g.sand, g.sand));
}
