use super::*;
use crate::boundary::ChangeSet;
use crate::natural_spawn::{CATEGORIES, Rng, spawn_group};

struct Habitat {
    air: u32,
    grass: u32,
    stone: u32,
    water: u32,
    wet: bool,
}
impl mc_world::ChunkGenerator for Habitat {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        );
        for x in 0..16 {
            for z in 0..16 {
                for y in [40, 44] {
                    chunk.set_block(x, y, z, mc_world::BlockStateId(self.stone));
                }
                chunk.set_block(x, 64, z, mc_world::BlockStateId(self.grass));
                if self.wet {
                    for y in 41..44 {
                        chunk.set_block(x, y, z, mc_world::BlockStateId(self.water));
                    }
                }
            }
        }
        chunk
    }
}
fn name(category: MobCategory) -> &'static str {
    match category {
        MobCategory::Monster => "minecraft:skeleton",
        MobCategory::Creature => "minecraft:cow",
        MobCategory::Ambient => "minecraft:bat",
        MobCategory::Axolotls => "minecraft:axolotl",
        MobCategory::UndergroundWaterCreature => "minecraft:glow_squid",
        MobCategory::WaterCreature => "minecraft:squid",
        MobCategory::WaterAmbient => "minecraft:cod",
        MobCategory::Misc => panic!("not a natural category"),
    }
}
fn key(category: MobCategory) -> &'static str {
    match category {
        MobCategory::Monster => "monster",
        MobCategory::Creature => "creature",
        MobCategory::Ambient => "ambient",
        MobCategory::Axolotls => "axolotls",
        MobCategory::UndergroundWaterCreature => "underground_water_creature",
        MobCategory::WaterCreature => "water_creature",
        MobCategory::WaterAmbient => "water_ambient",
        MobCategory::Misc => "misc",
    }
}
fn habitat(category: MobCategory) -> TickState {
    let mut registry = CoreRegistry::load(None).unwrap();
    registry.biome_spawns =
        mc_data::biomes::BiomeSpawnRules::from_entries(std::collections::BTreeMap::from([(
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
            std::collections::BTreeMap::from([(
                key(category).into(),
                vec![mc_data::biomes::BiomeSpawnEntry {
                    entity_type: mc_data::Identifier::parse(name(category)).unwrap(),
                    weight: 1,
                    min_count: 4,
                    max_count: 4,
                }],
            )]),
        )]));
    let registry = Arc::new(registry);
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(Habitat {
        air: registry.states.air,
        grass: registry.states.grass_block,
        stone: registry
            .blocks
            .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
            .unwrap()
            .default
            .0,
        water: registry.states.water,
        wet: matches!(
            category,
            MobCategory::Axolotls
                | MobCategory::UndergroundWaterCreature
                | MobCategory::WaterCreature
                | MobCategory::WaterAmbient
        ),
    });
    let mut world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 65, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let wanted = crate::world::chunks_in_radius(mc_world::ChunkPos { x: 0, z: 0 }, 4)
        .into_iter()
        .map(|pos| (pos, 0))
        .collect();
    world.ensure_blocking(&wanted);
    let mut state = TickState::new(registry, world, 4, 512);
    state.day_time = 6_000;
    state
}

#[test]
fn all_natural_categories_spawn_native_bodies_and_enforce_local_caps() {
    for wanted in CATEGORIES {
        let mut state = habitat(wanted);
        let observer = [0.5, 65.0, 0.5];
        let players = [observer, observer]; // overlapping observers must not double the local cap
        let cap = wanted.max_instances_per_chunk() as usize;
        let mut spawned = 0;
        // Hostile Y samples now include unsupported air and solid blocks;
        // do not rely on the former guaranteed-floor sampler to fill the cap.
        let attempts = if wanted == MobCategory::Monster {
            32_768
        } else {
            256
        };
        for seed in 0..attempts {
            if spawned == cap {
                break;
            }
            let mut changes = ChangeSet::default();
            crate::natural_spawn::census(&mut state, &players);
            spawned += spawn_group(
                &mut state,
                &players,
                observer,
                wanted,
                cap + 20,
                &mut Rng(seed),
                &mut changes,
            );
            for id in changes.entities_spawned {
                let slot = state.entities.slot_of(id).unwrap();
                let pos = state.entities.pos(slot);
                assert_eq!(category(&state, id), wanted);
                let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
                    panic!("natural spawn must be a mob");
                };
                assert_eq!(
                    state.registry.blueprint(type_id).unwrap().name,
                    name(wanted)
                );
                assert!(horizontal(pos, observer) >= 20.0);
                if wanted == MobCategory::Creature {
                    assert_eq!(pos[1], 65.0);
                } else {
                    assert!(pos[1] < 63.0, "cave/water habitat: {wanted:?} at {pos:?}");
                }
            }
            assert!(count(&state, wanted) <= cap, "{wanted:?} local cap");
        }
        assert_eq!(spawned, cap, "{wanted:?} reaches its own category cap");
        crate::natural_spawn::census(&mut state, &players);
        assert_eq!(
            spawn_group(
                &mut state,
                &players,
                observer,
                wanted,
                0,
                &mut Rng(9),
                &mut ChangeSet::default()
            ),
            0
        );
    }
}

#[test]
fn overlapping_players_share_global_resident_chunk_caps() {
    for wanted in CATEGORIES {
        let mut state = habitat(wanted);
        let observer = join(&mut state);
        let _ = run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(2),
                uuid: uuid::Uuid::new_v4(),
                name: "second".into(),
                permission_level: 0,
            }],
        );
        let second = state.connections.entity_of(ConnectionId(2)).unwrap();
        let slot = state.entities.slot_of(second).unwrap();
        state.entities.set_pos(slot, observer);
        simulate_fixture_chunks(&mut state);
        let eligible = state
            .world
            .chunks()
            .filter(|chunk| {
                let px = (observer[0].floor() as i32).div_euclid(16);
                let pz = (observer[2].floor() as i32).div_euclid(16);
                (chunk.pos.x - px).abs() <= 8 && (chunk.pos.z - pz).abs() <= 8
            })
            .count();
        let cap = wanted.max_instances_per_chunk() as usize * eligible / 289;
        let mut changes = ChangeSet::default();
        for tick in (400..=12_800).step_by(400) {
            state.tick = tick;
            // This test isolates cap admission from the separately tested idle rule.
            for index in 0..state.entities.len() as u32 {
                state.entities.row_mut(EntitySlot(index)).no_action_time = 0;
            }
            crate::natural_spawn::run(&mut state, &mut changes);
            assert!(
                count(&state, wanted) <= cap,
                "{wanted:?}: overlap cannot multiply eligible chunks"
            );
        }
        assert_eq!(count(&state, wanted), cap, "{wanted:?} global cap reached");
    }
}

#[test]
fn despawn_distances_persistence_and_peaceful_override_are_category_aware() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let observer = join(&mut state);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.day_time = 18_000;
    let mut retained = Vec::new();
    let mut removed = Vec::new();
    for (kind, distance, keep, persistent) in [
        ("minecraft:skeleton", 128.0, true, false),
        ("minecraft:skeleton", 128.01, false, false),
        ("minecraft:skeleton", 200.0, true, true),
        ("minecraft:cow", 200.0, true, false),
        ("minecraft:cod", 64.0, true, false),
        ("minecraft:cod", 64.01, false, false),
        ("minecraft:squid", 100.0, true, false),
        ("minecraft:bat", 200.0, false, false),
    ] {
        let type_id = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(kind).unwrap())
            .unwrap();
        let id = state.spawn_mob(
            type_id,
            [observer[0] + distance, observer[1], observer[2]],
            20.0,
        );
        state
            .entities
            .row_mut(state.entities.slot_of(id).unwrap())
            .persistent = persistent;
        if keep {
            retained.push(id);
        } else {
            removed.push(id);
        }
    }
    simulate_fixture_chunks(&mut state);
    state.tick = 20;
    let mut changes = ChangeSet::default();
    crate::natural_spawn::run(&mut state, &mut changes);
    for id in removed {
        assert!(changes.entities_removed.contains(&id));
    }
    for id in &retained {
        assert!(state.entities.slot_of(*id).is_some());
    }
    state.difficulty = crate::survival::Difficulty::Peaceful;
    state.tick = 40;
    crate::natural_spawn::run(&mut state, &mut changes);
    for id in retained {
        if state.entities.slot_of(id).is_some() {
            assert_ne!(
                category(&state, id),
                MobCategory::Monster,
                "peaceful removes even persistent monsters"
            );
        }
    }
}

#[test]
fn random_despawn_waits_for_idle_grace_and_near_players_reset_it() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let observer = join(&mut state);
    state.day_time = 18_000;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let id = state.spawn_mob(
        state.registry.zombie_type_id,
        [observer[0] + 50.0, observer[1], observer[2]],
        20.0,
    );
    for tick in 1..=600 {
        state.tick = tick;
        crate::natural_spawn::run(&mut state, &mut ChangeSet::default());
        assert!(
            state.entities.slot_of(id).is_some(),
            "no random despawn before idle grace"
        );
    }
    let slot = state.entities.slot_of(id).unwrap();
    state
        .entities
        .set_pos(slot, [observer[0] + 32.0, observer[1], observer[2]]);
    state.tick = 620;
    crate::natural_spawn::run(&mut state, &mut ChangeSet::default());
    assert_eq!(state.entities.row(slot).no_action_time, 0);
    state
        .entities
        .set_pos(slot, [observer[0] + 50.0, observer[1], observer[2]]);
    state.entities.row_mut(slot).no_action_time = 601;
    let tick = (640_u64..20_000)
        .find(|tick| {
            Rng((state.world.seed() as u64) ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)).below(800)
                == 0
        })
        .unwrap();
    state.tick = tick;
    let mut changes = ChangeSet::default();
    crate::natural_spawn::run(&mut state, &mut changes);
    assert!(changes.entities_removed.contains(&id));
}

#[test]
fn block_light_rejects_daytime_cave_monsters_and_removal_reenables_them() {
    let mut state = habitat(MobCategory::Monster);
    let glowstone = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:glowstone").unwrap())
        .unwrap()
        .default
        .0;
    // Both enclosed floors need their own emitters: the stone ceiling at
    // y=44 blocks light from y=40 reaching the second cave at y=45.
    // Vanilla Monster.isDarkEnoughToSpawn samples block light at the feet.
    for x in (-64..80).step_by(8) {
        for z in (-64..80).step_by(8) {
            for y in [40, 44] {
                state.world.set_block(x, y, z, glowstone);
            }
        }
    }
    state.world.fold_overlay();
    let mut changes = ChangeSet::default();
    for seed in 0..2048 {
        crate::natural_spawn::census(&mut state, &[]);
        assert_eq!(
            spawn_group(
                &mut state,
                &[],
                [0.5, 65.0, 0.5],
                MobCategory::Monster,
                70,
                &mut Rng(seed),
                &mut changes
            ),
            0
        );
    }
    let stone = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    for x in (-64..80).step_by(8) {
        for z in (-64..80).step_by(8) {
            for y in [40, 44] {
                state.world.set_block(x, y, z, stone);
            }
        }
    }
    state.world.fold_overlay();
    let mut total = 0;
    for seed in 0..2048 {
        crate::natural_spawn::census(&mut state, &[]);
        total += spawn_group(
            &mut state,
            &[],
            [0.5, 65.0, 0.5],
            MobCategory::Monster,
            70,
            &mut Rng(seed),
            &mut changes,
        );
    }
    assert!(
        total > 0,
        "removing block-light source permits cave spawns in daylight"
    );
}

#[test]
fn spawn_monsters_command_gates_spawning_but_not_despawn_or_daylight_animals() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let observer = join(&mut state);
    for player in state.players.values_mut() {
        player.game_mode = crate::commands::GameMode::Creative;
    }
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            command: "gamerule spawnMonsters false".into(),
            permission_level: 4,
        }],
    );
    let far = state.spawn_mob(
        state.registry.zombie_type_id,
        [observer[0] + 200.0, observer[1], observer[2]],
        20.0,
    );
    simulate_fixture_chunks(&mut state);
    let mut changes = ChangeSet::default();
    for tick in (400..=3_200).step_by(400) {
        state.tick = tick;
        state.day_time = 6_000;
        crate::natural_spawn::run(&mut state, &mut changes);
    }
    assert!(changes.entities_removed.contains(&far));
    assert!(count(&state, MobCategory::Creature) > 0);
    for tick in (3_220..=4_000).step_by(20) {
        state.tick = tick;
        state.day_time = 18_000;
        crate::natural_spawn::run(&mut state, &mut changes);
    }
    assert_eq!(count(&state, MobCategory::Monster), 0);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            command: "gamerule spawnMonsters true".into(),
            permission_level: 4,
        }],
    );
    for tick in (4_020..=6_000).step_by(20) {
        state.tick = tick;
        state.day_time = 18_000;
        crate::natural_spawn::run(&mut state, &mut changes);
    }
    assert!(count(&state, MobCategory::Monster) > 0);
}

#[test]
fn no_observers_pause_distance_despawn_but_not_peaceful_removal() {
    let mut state = habitat(MobCategory::Monster);
    let distant = state.spawn_mob(state.registry.zombie_type_id, [500.0, 65.0, 0.0], 20.0);
    let slot = state.entities.slot_of(distant).unwrap();
    state.entities.row_mut(slot).no_action_time = 700;
    simulate_fixture_chunks(&mut state);
    state.tick = 20;
    let mut changes = ChangeSet::default();
    crate::natural_spawn::run(&mut state, &mut changes);
    assert!(state.entities.slot_of(distant).is_some());
    assert_eq!(state.entities.row(slot).no_action_time, 700);
    state.entities.row_mut(slot).persistent = true;
    state.difficulty = crate::survival::Difficulty::Peaceful;
    state.tick = 40;
    crate::natural_spawn::run(&mut state, &mut changes);
    assert!(changes.entities_removed.contains(&distant));
}

#[test]
fn persistent_named_and_tamed_creatures_do_not_consume_local_caps() {
    let mut state = habitat(MobCategory::Creature);
    let observer = [0.5, 65.0, 0.5];
    for mode in 0..3 {
        for _ in 0..10 {
            let id = state.spawn_mob(state.registry.cow_type_id, observer, 10.0);
            let row = state.entities.row_mut(state.entities.slot_of(id).unwrap());
            match mode {
                0 => row.persistent = true,
                1 => row.entity_life.tamed = true,
                _ => {
                    Arc::make_mut(&mut row.appearance).custom_name =
                        Some("{\"text\":\"kept\"}".into())
                }
            }
        }
    }
    let mut changes = ChangeSet::default();
    crate::natural_spawn::census(&mut state, &[observer]);
    let spawned = spawn_group(
        &mut state,
        &[observer],
        observer,
        MobCategory::Creature,
        10,
        &mut Rng(0),
        &mut changes,
    );
    assert!(
        spawned > 0,
        "thirty persistent bodies cannot exhaust a natural creature cap"
    );
    assert_eq!(count(&state, MobCategory::Creature), 30 + spawned);
}

#[test]
fn native_rabbit_and_mooshroom_tables_admit_old_natural_ground_fallbacks() {
    for (floor_name, species) in [
        ("minecraft:sand", "minecraft:rabbit"),
        ("minecraft:mycelium", "minecraft:mooshroom"),
    ] {
        let mut state = habitat(MobCategory::Creature);
        let registry = Arc::get_mut(&mut state.registry).unwrap();
        let species_id = mc_data::Identifier::parse(species).unwrap();
        let type_id = registry.entity_types.id_of(&species_id).unwrap();
        registry.biome_spawns =
            mc_data::biomes::BiomeSpawnRules::from_entries(std::collections::BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:plains").unwrap(),
                std::collections::BTreeMap::from([(
                    "creature".into(),
                    vec![mc_data::biomes::BiomeSpawnEntry {
                        entity_type: species_id,
                        weight: 1,
                        min_count: 2,
                        max_count: 2,
                    }],
                )]),
            )]));
        let floor = registry
            .blocks
            .block(&mc_data::Identifier::parse(floor_name).unwrap())
            .unwrap()
            .default
            .0;
        let mut reference = Rng(0);
        let angle = reference.unit() * std::f64::consts::TAU;
        let radius = 24.0 + reference.unit() * 40.0;
        let column = [
            (0.5 + f64::from(mc_physics::mth::cos(angle)) * radius).floor() as i32,
            (0.5 + f64::from(mc_physics::mth::sin(angle)) * radius).floor() as i32,
        ];
        for x in (column[0] - 3)..=(column[0] + 3) {
            for z in (column[1] - 3)..=(column[1] + 3) {
                state.world.set_block(x, 64, z, floor);
            }
        }
        state.world.fold_overlay();
        let mut changes = ChangeSet::default();
        crate::natural_spawn::census(&mut state, &[]);
        let spawned = spawn_group(
            &mut state,
            &[],
            [0.5, 65.0, 0.5],
            MobCategory::Creature,
            10,
            &mut Rng(0),
            &mut changes,
        );
        assert!(
            spawned > 0 && spawned <= 2,
            "{species} uses biome pack on {floor_name}"
        );
        for id in changes.entities_spawned {
            let slot = state.entities.slot_of(id).unwrap();
            assert_eq!(state.entities.body(slot).kind, EntityKind::Mob { type_id });
            let pos = state.entities.pos(slot);
            assert_eq!(
                state
                    .world
                    .block_at(pos[0].floor() as i32, 64, pos[2].floor() as i32),
                Some(floor)
            );
        }
    }
}

#[test]
fn spawn_census_matches_population_oracle_for_all_categories_and_observers() {
    let mut state = habitat(MobCategory::Creature);
    let players = [[-0.5, 65.0, -0.5], [128.5, 65.0, 0.5], [-0.5, 65.0, -0.5]];
    for wanted in CATEGORIES {
        let type_id = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name(wanted)).unwrap())
            .unwrap();
        for x in [-145.0, -128.0, -0.5, 0.5, 127.5, 128.5, 272.5] {
            state.spawn_mob(type_id, [x, 65.0, -0.5], 20.0);
        }
        let id = state.spawn_mob(type_id, [0.5, 65.0, 0.5], 20.0);
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).persistent = true;
        let id = state.spawn_mob(type_id, [0.5, 65.0, 0.5], 20.0);
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.body_mut(slot).alive = false;
    }
    let global = crate::natural_spawn::census(&mut state, &players);
    for (category_index, wanted) in CATEGORIES.into_iter().enumerate() {
        let mut expected_global = 0;
        let mut expected_local = [0; 3];
        for index in 0..state.entities.len() {
            let slot = crate::entities::EntitySlot(index as u32);
            let row = state.entities.row(slot);
            if !state.entities.body(slot).alive
                || row.persistent
                || category(&state, state.entities.body(slot).id) != wanted
            {
                continue;
            }
            expected_global += 1;
            let pos = state.entities.pos(slot);
            let center = [
                f64::from((pos[0].floor() as i32).div_euclid(16) * 16 + 8),
                f64::from((pos[2].floor() as i32).div_euclid(16) * 16 + 8),
            ];
            for (observer, player) in players.iter().enumerate() {
                let dx = player[0] - center[0];
                let dz = player[2] - center[1];
                if dx * dx + dz * dz <= 128.0 * 128.0 {
                    expected_local[observer] += 1;
                }
            }
        }
        assert_eq!(global[category_index], expected_global, "{wanted:?}");
        for (observer, expected) in expected_local.into_iter().enumerate() {
            assert_eq!(
                state.spawn_census.local_count(observer, category_index),
                expected,
                "{wanted:?} observer {observer}"
            );
        }
    }
}

#[test]
fn spawn_census_overlap_matches_strict_bodies_across_negative_and_giant_chunk_edges() {
    let mut state = habitat(MobCategory::Creature);
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let giant = state.spawn_mob(type_id, [-1.0, 65.0, -0.5], 20.0);
    let slot = state.entities.slot_of(giant).unwrap();
    state.entities.set_half_width(slot, 36.0);
    state.spawn_mob(type_id, [15.9, 65.0, 64.0], 20.0);
    crate::natural_spawn::census(&mut state, &[]);
    for position in [
        [31.5, 65.0, -0.5],
        [36.0, 65.0, -0.5],
        [16.1, 65.0, 64.0],
        [16.1, 70.0, 64.0],
        [-37.3, 65.0, -0.5],
    ] {
        let expected = (0..state.entities.len()).any(|index| {
            let slot = crate::entities::EntitySlot(index as u32);
            let row = state.entities.body(slot);
            let other = state.entities.pos(slot);
            row.alive
                && (position[0] - other[0]).abs() < 0.3 + row.half_width
                && (position[2] - other[2]).abs() < 0.3 + row.half_width
                && position[1] < other[1] + row.height
                && other[1] < position[1] + 1.8
        });
        assert_eq!(
            state
                .spawn_census
                .overlaps(&state.entities, position, 0.3, 1.8),
            expected,
            "{position:?}"
        );
    }
}

#[test]
fn successive_packs_update_local_cap_and_newborn_collision_without_rebuilding_census() {
    let mut state = habitat(MobCategory::Creature);
    let observer = [0.5, 65.0, 0.5];
    let players = [observer, observer];
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    for _ in 0..9 {
        state.spawn_mob(type_id, observer, 20.0);
    }
    crate::natural_spawn::census(&mut state, &players);
    let mut changes = ChangeSet::default();
    assert_eq!(
        spawn_group(
            &mut state,
            &players,
            observer,
            MobCategory::Creature,
            10,
            &mut Rng(0),
            &mut changes
        ),
        1
    );
    for observer in 0..players.len() {
        assert_eq!(state.spawn_census.local_count(observer, 1), 10);
    }
    let newborn = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    let pos = state.entities.pos(newborn);
    let row = state.entities.body(newborn);
    assert!(
        state
            .spawn_census
            .overlaps(&state.entities, pos, row.half_width, row.height)
    );
    assert_eq!(
        spawn_group(
            &mut state,
            &players,
            observer,
            MobCategory::Creature,
            10,
            &mut Rng(1),
            &mut changes
        ),
        0
    );
    assert_eq!(count(&state, MobCategory::Creature), 10);
    assert_eq!(changes.entities_spawned.len(), 1);
}

#[test]
fn observer_assembly_keeps_sorted_ids_and_reuses_buffers_without_reallocating() {
    let mut state = habitat(MobCategory::Creature);
    let observer = join(&mut state);
    // A second observer exercises deterministic identity ordering.
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: uuid::Uuid::new_v4(),
            name: "second".into(),
            permission_level: 0,
        }],
    );
    let first = state.connections.entity_of(ConnectionId(1)).unwrap();
    let second = state.connections.entity_of(ConnectionId(2)).unwrap();
    assert!(first.0 < second.0);
    let slot = state.entities.slot_of(second).unwrap();
    state.entities.set_pos(slot, observer);
    simulate_fixture_chunks(&mut state);

    state.tick = 20;
    crate::natural_spawn::run(&mut state, &mut ChangeSet::default());
    let mut table = state.spawn_census.observer_table.clone();
    assert_eq!(table.len(), 2);
    table.sort_unstable_by_key(|(id, _)| id.0);
    assert_eq!(
        state.spawn_census.observer_table.as_slice(),
        table.as_slice(),
        "observers stay sorted by entity id for RNG/local-cap accounting"
    );
    for (observer, position) in state.spawn_census.observer_table.iter() {
        let slot = state.entities.slot_of(*observer).expect("live observer");
        assert_eq!(*position, state.entities.pos(slot));
    }
    let positions: Vec<[f64; 3]> = state
        .spawn_census
        .observer_table
        .iter()
        .map(|(_, pos)| *pos)
        .collect();
    assert_eq!(
        state.spawn_census.observer_positions.as_slice(),
        positions.as_slice(),
        "positions project once from the same sorted table"
    );
    assert_eq!(
        state.spawn_census.observer_positions.len(),
        state.spawn_census.observer_table.len()
    );

    // Warm buffers must not reallocate on a steady-state pass with the same
    // observer set; behavior is pinned by entity removal, not capacity.
    let table_capacity = state.spawn_census.observer_table.capacity();
    let positions_capacity = state.spawn_census.observer_positions.capacity();
    assert!(table_capacity >= 2 && positions_capacity >= 2);
    state.tick = 40;
    crate::natural_spawn::run(&mut state, &mut ChangeSet::default());
    assert_eq!(state.spawn_census.observer_table.capacity(), table_capacity);
    assert_eq!(
        state.spawn_census.observer_positions.capacity(),
        positions_capacity
    );
}

#[test]
fn spawn_disabled_pass_skips_position_assembly_but_still_despawns() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let observer = join(&mut state);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let zombie = state.registry.zombie_type_id;
    let distant = state.spawn_mob(
        zombie,
        [observer[0] + 200.0, observer[1], observer[2]],
        20.0,
    );
    simulate_fixture_chunks(&mut state);
    state.tick = 20;
    let mut changes = ChangeSet::default();
    crate::natural_spawn::run(&mut state, &mut changes);
    assert!(changes.entities_removed.contains(&distant));
    assert!(
        state.spawn_census.observer_positions.is_empty(),
        "disabled spawn path must not project positions"
    );
    assert_eq!(state.spawn_census.observer_table.len(), 1);
    // Spectators never observe: a spectator watcher adds no table entry.
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: uuid::Uuid::new_v4(),
            name: "spectator".into(),
            permission_level: 0,
        }],
    );
    let watcher = state.connections.entity_of(ConnectionId(2)).unwrap();
    state
        .players
        .get_mut(&watcher)
        .expect("spectator")
        .game_mode = crate::commands::GameMode::Spectator;
    state.tick = 40;
    crate::natural_spawn::run(&mut state, &mut ChangeSet::default());
    assert_eq!(state.spawn_census.observer_table.len(), 1);
    assert!(state.spawn_census.observer_positions.is_empty());
}

#[test]
fn every_production_biome_spawn_entry_has_a_selectable_native_blueprint() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let registry = CoreRegistry::load(Some(&dir)).unwrap();
    let mut entries_checked = 0;
    let mut families = std::collections::BTreeSet::new();
    for biome in registry.biome_spawns.biomes() {
        for category in CATEGORIES {
            for entry in registry.biome_spawns.exact_entries(biome, key(category)) {
                assert!(entry.weight > 0, "{biome}: {}", entry.entity_type);
                assert_eq!(
                    crate::natural_spawn::choose_spawn(&registry, std::slice::from_ref(entry), 0),
                    Some(entry),
                    "{biome} {category:?}: {}",
                    entry.entity_type
                );
                entries_checked += 1;
                families.insert(entry.entity_type.as_str());
            }
        }
    }
    assert!(
        entries_checked > 100,
        "production table was not exercised: {entries_checked}"
    );
    for family in ["minecraft:witch", "minecraft:ghast"] {
        assert!(
            families.contains(family),
            "missing production family {family}"
        );
    }
    // Guardians are authored by the monument's structure override, not by a
    // biome table. Exercise that real production entry through the same picker.
    let monument_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/data/minecraft/worldgen/structure/monument.json");
    let monument: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&monument_path).unwrap()).unwrap();
    for raw in monument["spawn_overrides"]["monster"]["spawns"]
        .as_array()
        .unwrap()
    {
        let entry = mc_data::biomes::BiomeSpawnEntry {
            entity_type: mc_data::Identifier::parse(raw["type"].as_str().unwrap()).unwrap(),
            weight: raw["weight"].as_u64().unwrap() as u32,
            min_count: raw["minCount"].as_u64().unwrap() as u32,
            max_count: raw["maxCount"].as_u64().unwrap() as u32,
        };
        assert_eq!(
            crate::natural_spawn::choose_spawn(&registry, std::slice::from_ref(&entry), 0),
            Some(&entry)
        );
        assert_eq!(entry.entity_type.as_str(), "minecraft:guardian");
        let type_id = registry.entity_types.id_of(&entry.entity_type).unwrap();
        assert_eq!(
            registry.blueprint(type_id).unwrap().movement,
            crate::blueprint::MovementClass::Swimmer
        );
    }
}
