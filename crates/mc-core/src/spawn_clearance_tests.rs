//! Fresh-world natural spawn selection checks the chunk the generator produces.
//!
//! Seed 7's first routing-level candidate (-640, 68, 512) is a dry grass block
//! under a `short_grass` decoration that the terrain router cannot see, so the
//! search must skip it instead of refusing to start the world.
use crate::{Core, CoreConfig, registry::CoreRegistry};
use mc_world::{BlockStateId, ChunkGenerator};

fn seed_7_config(root: &std::path::Path) -> CoreConfig {
    let mut config = CoreConfig::new(root.to_owned(), 7);
    config.view_distance = 1;
    config.simulation_distance = 1;
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config
}

#[test]
fn seed_7_spawn_search_skips_candidates_blocked_by_generated_decoration() {
    let root = tempfile::tempdir().unwrap();
    let config = seed_7_config(root.path());
    let registry = CoreRegistry::load(config.vanilla_data_dir.as_deref()).unwrap();
    let terrain = crate::core::overworld_terrain(&config, &registry).unwrap();
    let air = BlockStateId(registry.states.air);

    let spawn = terrain.locate_safe_spawn().unwrap();
    assert_ne!(
        (spawn.block_x, spawn.block_z),
        (-640, 512),
        "the decorated routing-level candidate must be skipped"
    );
    let chunk = terrain.generate(spawn.chunk());
    let lx = spawn.block_x.rem_euclid(16) as u8;
    let lz = spawn.block_z.rem_euclid(16) as u8;
    assert_ne!(chunk.get_block(lx, spawn.surface_y, lz), Some(air));
    for dy in 1..=2 {
        assert_eq!(chunk.get_block(lx, spawn.surface_y + dy, lz), Some(air));
    }
}

#[test]
fn fresh_seed_7_world_opens_with_a_cleared_natural_spawn() {
    let root = tempfile::tempdir().unwrap();
    let core = Core::open(seed_7_config(root.path()))
        .expect("seed 7 must find a block-level clear natural spawn");
    core.shutdown();
}
