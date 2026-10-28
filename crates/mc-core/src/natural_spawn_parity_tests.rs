use super::*;
use std::sync::Arc;

struct Flat {
    air: u32,
    floor: u32,
    biome: &'static str,
    y: i32,
}

impl mc_world::ChunkGenerator for Flat {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.air),
            mc_data::Identifier::parse(self.biome).unwrap(),
        );
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, self.y - 1, z, mc_world::BlockStateId(self.floor));
            }
        }
        chunk
    }
}

fn flat(biome: &'static str, y: i32) -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = Arc::new(Flat {
        air: registry.states.air,
        floor: registry.states.grass_block,
        biome,
        y,
    });
    let mut world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, y, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let wanted = crate::world::chunks_in_radius(mc_world::ChunkPos { x: 0, z: 0 }, 4)
        .into_iter()
        .map(|pos| (pos, 0))
        .collect();
    world.ensure_blocking(&wanted);
    let active = world.chunks().map(|chunk| chunk.pos).collect();
    world.set_active_chunks(active, 0);
    let mut state = TickState::new(registry, world, 4, 512);
    state.day_time = 18_000;
    state
}

fn accepts(state: &TickState, name: &str, pos: [f64; 3], seed: u64) -> bool {
    let id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap();
    admitted(
        state,
        state.registry.blueprint(id).unwrap(),
        pos,
        &mut Rng(seed),
    )
}

#[test]
fn hostile_gap_spawn_support_rejects_glass_and_bottom_slabs() {
    let mut state = flat("minecraft:plains", 65);
    for (name, slab_type, allowed) in [
        ("stone", None, true),
        ("glass", None, false),
        ("tinted_glass", None, false),
        ("white_stained_glass", None, false),
        ("stone_slab", Some("bottom"), false),
        ("stone_slab", Some("top"), true),
        ("stone_slab", Some("double"), true),
    ] {
        let name = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
        let block = state.registry.blocks.block(&name).unwrap();
        let mut props = state
            .registry
            .blocks
            .by_id(block.default)
            .unwrap()
            .properties
            .clone();
        if let Some(value) = slab_type {
            props.iter_mut().find(|(key, _)| key == "type").unwrap().1 = value.into();
        }
        let floor = state
            .registry
            .blocks
            .by_name_and_props(&name, &props)
            .unwrap()
            .0;
        state.world.set_block(0, 64, 0, floor);
        assert_eq!(
            valid_support(&state, 0, 64, 0, MobCategory::Monster, &[]),
            allowed,
            "{name} {slab_type:?}"
        );
    }
}

#[test]
fn hostile_columns_use_the_outer_128_block_area() {
    let mut outer = 0;
    for seed in 0..1024 {
        let [x, z] = spawn_column([0.5, 65.0, 0.5], MobCategory::Monster, &mut Rng(seed));
        let distance = f64::from(x * x + z * z).sqrt();
        assert!(distance < 130.0);
        outer += usize::from(distance > 80.0);
    }
    assert!(
        outer > 400,
        "hostile candidates must not concentrate inside 64 blocks: {outer}"
    );
}

#[test]
fn hostile_height_sampling_keeps_invalid_air_and_solid_attempts() {
    let state = flat("minecraft:plains", 65);
    let mut floor = 0;
    let mut below = false;
    for seed in 0..512 {
        let y = spawn_height(&state, 0, 0, MobCategory::Monster, &mut Rng(seed)).unwrap();
        assert!((-64..=65).contains(&y));
        floor += usize::from(y == 65);
        below |= y < 64;
    }
    assert!(
        below && floor < 20,
        "do not retry until a valid surface is found: {floor}"
    );
}

#[test]
fn surface_slimes_require_swamp_habitat() {
    let state = flat("minecraft:plains", 65);
    assert!((0..128).all(|seed| !accepts(&state, "minecraft:slime", [0.5, 65.0, 0.5], seed)));
    assert!((0..128).any(|seed| accepts(&state, "minecraft:zombie", [0.5, 65.0, 0.5], seed)));
}

#[test]
fn swamp_slimes_respect_moon_altitude_and_light() {
    let mut state = flat("minecraft:swamp", 65);
    assert!((0..256).any(|seed| accepts(&state, "minecraft:slime", [0.5, 65.0, 0.5], seed)));
    state.day_time += 4 * 24_000;
    assert!((0..256).all(|seed| !accepts(&state, "minecraft:slime", [0.5, 65.0, 0.5], seed)));
    state.day_time = 6_000;
    assert!((0..128).all(|seed| !accepts(&state, "minecraft:slime", [0.5, 65.0, 0.5], seed)));
    let high = flat("minecraft:swamp", 70);
    assert!((0..128).all(|seed| !accepts(&high, "minecraft:slime", [0.5, 70.0, 0.5], seed)));
}

#[test]
fn underground_slimes_require_seeded_chunks_and_ignore_light() {
    let mut state = flat("minecraft:plains", 39);
    state.day_time = 6_000;
    // Java Random slime-chunk vectors for world seed 42: (2,0) yes,
    // (0,0) no. Open daytime sky intentionally rejects ordinary monsters.
    assert!((0..256).any(|seed| accepts(&state, "minecraft:slime", [32.5, 39.0, 0.5], seed)));
    assert!((0..256).all(|seed| !accepts(&state, "minecraft:slime", [0.5, 39.0, 0.5], seed)));
    assert!((0..256).all(|seed| !accepts(&state, "minecraft:zombie", [32.5, 39.0, 0.5], seed)));
    let high = flat("minecraft:plains", 40);
    assert!((0..256).all(|seed| !accepts(&high, "minecraft:slime", [32.5, 40.0, 0.5], seed)));
}

#[test]
fn distant_hostiles_despawn_on_non_spawn_cycle_ticks() {
    let mut state = flat("minecraft:plains", 65);
    let _ = crate::tick::run_tick(
        &mut state,
        &[crate::boundary::PlayerInput::Join {
            connection: crate::boundary::ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "observer".into(),
            permission_level: 0,
        }],
    );
    let zombie = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let id = state.spawn_mob(zombie, [0.5, 250.0, 0.5], 20.0);
    state.tick = 21;
    let mut changes = ChangeSet::default();
    run(&mut state, &mut changes);
    assert!(changes.entities_removed.contains(&id));
}

#[test]
fn hostile_attempts_run_between_twenty_tick_passes() {
    let mut state = flat("minecraft:plains", 65);
    let _ = crate::tick::run_tick(
        &mut state,
        &[crate::boundary::PlayerInput::Join {
            connection: crate::boundary::ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "observer".into(),
            permission_level: 0,
        }],
    );
    let mut spawned = 0;
    for tick in 21..200 {
        if tick % 20 == 0 {
            continue;
        }
        state.tick = tick;
        let mut changes = ChangeSet::default();
        run(&mut state, &mut changes);
        for id in changes.entities_spawned {
            let slot = state.entities.slot_of(id).unwrap();
            if category_of(&state, slot) == MobCategory::Monster {
                spawned += 1;
            }
        }
    }
    assert!(
        spawned > 0,
        "hostiles attempt spawning every tick, not once per second"
    );
}

#[test]
fn local_counts_use_128_block_circle_from_chunk_centers() {
    let mut state = flat("minecraft:plains", 65);
    let zombie = state.registry.zombie_type_id;
    state.world.ensure_blocking(
        &[(8, 8), (7, 0), (-8, 0)]
            .map(|(x, z)| (mc_world::ChunkPos { x, z }, 0))
            .into_iter()
            .collect(),
    );
    // Chunk (8, 8) sits inside the 17x17 square around the observer's chunk,
    // but its center (136, 136) is ~192 blocks away: outside vanilla's
    // 128-block horizontal circle.
    state.spawn_mob(zombie, [128.5, 65.0, 128.5], 20.0);
    // Chunk (7, 0) center (120, 8) is ~120 blocks away: inside the circle.
    state.spawn_mob(zombie, [112.5, 65.0, 0.5], 20.0);
    let global = census(&mut state, &[[0.5, 65.0, 0.5]]);
    assert_eq!(global[0], 2, "global cap still counts every loaded mob");
    assert_eq!(
        state.spawn_census.local_count(0, 0),
        1,
        "only the mob whose chunk center is within 128 blocks counts locally"
    );
    // Players in the same chunk can lie on opposite sides of a local-cap
    // boundary: use actual positions, not the observer's chunk center.
    state.spawn_mob(zombie, [-127.5, 65.0, 0.5], 20.0);
    census(&mut state, &[[15.9, 65.0, 8.0], [0.1, 65.0, 8.0]]);
    assert_eq!(state.spawn_census.local_count(0, 0), 1);
    assert_eq!(state.spawn_census.local_count(1, 0), 2);
}

#[test]
fn spawn_admission_allows_chunk_with_any_uncapped_observer_in_range() {
    let mut state = flat("minecraft:plains", 65);
    let zombie = state.registry.zombie_type_id;
    state.world.ensure_blocking(
        &[(mc_world::ChunkPos { x: -7, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    // Fractional observer posts straddle chunk centers: vanilla measures
    // from actual x/z, not observer chunk centers.
    let capped = [15.9, 65.0, 0.5];
    let free = [150.5, 65.0, 0.5];
    // Fill the capped observer's circle without entering the other's: chunk
    // (-7, 0) center (-104, 8) is ~120 blocks from `capped`, ~255 from `free`.
    for i in 0..70 {
        state.spawn_mob(zombie, [-111.5 + f64::from(i) * 0.2, 65.0, 0.5], 20.0);
    }
    census(&mut state, &[capped, free]);
    assert_eq!(state.spawn_census.local_count(0, 0), 70);
    assert_eq!(state.spawn_census.local_count(1, 0), 0);
    // Chunk (4, 0) center (72, 8) is within 128 blocks of both observers, so
    // vanilla allows the spawn: at least one in-range observer is uncapped.
    assert!(
        !state
            .spawn_census
            .local_cap_reached([72.5, 65.0, 0.5], 0, 70),
        "a capped observer must not veto a chunk that an uncapped observer also covers"
    );
    // Chunk (-7, 0) is covered only by the capped observer: spawns stay
    // blocked there even though an uncapped observer exists elsewhere.
    assert!(
        state
            .spawn_census
            .local_cap_reached([-111.5, 65.0, 0.5], 0, 70),
        "a fully capped circle still blocks its own chunks"
    );
    // A chunk center beyond 128 blocks of every observer also stays blocked.
    assert!(
        state
            .spawn_census
            .local_cap_reached([600.5, 65.0, 600.5], 0, 70),
        "chunks outside every observer circle admit no spawns"
    );
}

#[test]
fn distant_players_spawn_hostiles_despite_frozen_old_population() {
    use crate::boundary::{ConnectionId, PlayerInput};

    for frozen_count in [0, 70] {
        let mut state = flat("minecraft:plains", 65);
        state.config_sim_distance = 6;
        state.config_view_distance = 6;
        crate::tick::run_tick(
            &mut state,
            &[1, 2].map(|id| PlayerInput::Join {
                connection: ConnectionId(id),
                uuid: uuid::Uuid::new_v4(),
                name: format!("observer{id}"),
                permission_level: 0,
            }),
        );
        let observers = [[0.5, 65.0, 0.5], [1312.5, 65.0, 0.5]];
        for (id, position) in [1, 2].into_iter().zip(observers) {
            let player = state.connections.entity_of(ConnectionId(id)).unwrap();
            state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
            let slot = state.entities.slot_of(player).unwrap();
            state.entities.set_pos(slot, position);
        }
        state.world.ensure_blocking(&state.desired_chunks());
        state.refresh_active_chunks(state.tick + 1);
        let zombie = state.registry.zombie_type_id;
        for _ in 0..64 {
            state.spawn_mob(zombie, [8.5, 65.0, 0.5], 20.0);
        }
        // Native rows survive chunk deactivation. They must not consume the
        // cap scaled to the two current simulation areas.
        let frozen: Vec<_> = (0..frozen_count)
            .map(|_| state.spawn_mob(zombie, [656.5, 65.0, 0.5], 20.0))
            .collect();
        assert!(!state.world.is_active(41, 0));
        census(&mut state, &observers);
        assert!(state.spawn_census.eligible.len() >= 2 * 13 * 13);
        assert_eq!(state.spawn_census.local_count(0, 0), 64);
        assert_eq!(state.spawn_census.local_count(1, 0), 0);
        let started = std::time::Instant::now();
        let mut far_births = 0;
        for tick in 21..277 {
            state.tick = tick;
            let mut changes = ChangeSet::default();
            run(&mut state, &mut changes);
            for id in changes.entities_spawned {
                let slot = state.entities.slot_of(id).unwrap();
                if category_of(&state, slot) == MobCategory::Monster
                    && state.entities.pos(slot)[0] > 1200.0
                {
                    far_births += 1;
                }
            }
        }
        eprintln!(
            "natural-spawn debug loaded-host: frozen={frozen_count}, 256 passes={:?}, far births={far_births}",
            started.elapsed()
        );
        assert!(
            far_births > 0,
            "64 hostiles beside one observer must not starve another 1312 blocks away; frozen={frozen_count}"
        );
        assert!(
            frozen
                .iter()
                .all(|id| state.entities.slot_of(*id).is_some())
        );
    }
}

#[test]
fn census_excludes_unresident_rows_but_keeps_loaded_inactive_mobs() {
    let mut state = flat("minecraft:plains", 65);
    let zombie = state.registry.zombie_type_id;
    state.spawn_mob(zombie, [64.5, 65.0, 0.5], 20.0);
    state.spawn_mob(zombie, [656.5, 65.0, 0.5], 20.0);
    state
        .world
        .set_active_chunks([mc_world::ChunkPos { x: 0, z: 0 }].into_iter().collect(), 0);
    assert!(state.world.has_chunk(4, 0));
    assert!(!state.world.is_active(4, 0));
    assert!(!state.world.has_chunk(41, 0));
    assert_eq!(census(&mut state, &[[0.5, 65.0, 0.5]])[0], 1);
    assert_eq!(state.spawn_census.local_count(0, 0), 1);
    assert!(
        state
            .spawn_census
            .overlaps(&state.entities, [656.5, 65.0, 0.5], 0.3, 1.8,)
    );
}
