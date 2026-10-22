use std::sync::Arc;

use crate::blueprint::MobCategory;
use crate::boundary::{ConnectionId, EntityId, PlayerInput};
use crate::entities::{EntityKind, EntitySlot};
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;

fn test_state(sim_distance: i32) -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry loads"));
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().expect("test terrain spawn");
    let world_spawn = [spawn.block_x, spawn.surface_y + 1, spawn.block_z];
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        world_spawn,
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, sim_distance, 512)
}

fn join(state: &mut TickState) -> [f64; 3] {
    let _ = run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "watcher".to_owned(),
            permission_level: 0,
        }],
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    simulate_fixture_chunks(state);
    let player = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("joined");
    state
        .entities
        .pos(state.entities.slot_of(player).expect("slot"))
}

// Direct natural-spawn passes bypass run_tick's active/resident admission.
// Include authored distant bodies so these tests exercise despawn, not freezing.
fn simulate_fixture_chunks(state: &mut TickState) {
    let wanted = state
        .entities
        .slots()
        .map(|slot| {
            let pos = state.entities.pos(slot);
            (
                crate::boundary::ChunkPos {
                    x: (pos[0].floor() as i32).div_euclid(16),
                    z: (pos[2].floor() as i32).div_euclid(16),
                },
                0,
            )
        })
        .collect();
    state.world.ensure_blocking(&wanted);
    let active = state.world.chunks().map(|chunk| chunk.pos).collect();
    state.world.set_active_chunks(active, state.tick);
}

fn category(state: &TickState, id: EntityId) -> MobCategory {
    let slot = state.entities.slot_of_any(id).expect("entity row");
    match state.entities.body(slot).kind {
        EntityKind::Mob { type_id } => {
            state
                .registry
                .blueprint(type_id)
                .expect("blueprint")
                .category
        }
        _ => MobCategory::Misc,
    }
}

fn count(state: &TickState, wanted: MobCategory) -> usize {
    state
        .entities
        .slots()
        .filter(|slot| category(state, state.entities.body(*slot).id) == wanted)
        .count()
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[test]
fn disabling_mob_spawning_keeps_existing_mob_despawns_active() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let around = join(&mut state);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.players.values_mut().next().unwrap().game_mode = crate::commands::GameMode::Creative;
    state.day_time = 18_000;
    let zombie = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let distant = state.spawn_mob(zombie, [around[0] + 200.0, around[1], around[2]], 20.0);
    simulate_fixture_chunks(&mut state);
    let mut changes = crate::boundary::ChangeSet::default();
    for tick in (400..=1200).step_by(20) {
        state.tick = tick;
        crate::natural_spawn::run(&mut state, &mut changes);
    }
    assert!(changes.entities_removed.contains(&distant));
    assert!(changes.entities_spawned.is_empty());
    assert_eq!(count(&state, MobCategory::Monster), 0);
}

/// Daylight creatures and night surface monsters honor distance and caps.
#[test]
fn night_spawns_monsters_away_from_players_and_creatures_on_grass() {
    // Test spawn cadence/distance/caps on an authored eligible biome, not a
    // seed-dependent terrain column whose biome may forbid farm animals.
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let player = join(&mut state);
    // Keep the observer alive on unobstructed terrain: combat is covered
    // separately, and a dead player stops natural-spawn passes.
    state
        .players
        .values_mut()
        .next()
        .expect("watcher")
        .game_mode = crate::commands::GameMode::Creative;

    let mut monsters = 0;
    let mut creatures = 0;
    for tick in 1..=840 {
        state.day_time = if tick <= 420 { 1_000 } else { 18_000 };
        let (output, _, _) = run_tick(&mut state, &[]);
        for id in &output.changes.entities_spawned {
            let slot = state.entities.slot_of(*id).expect("spawned entity is live");
            let pos = state.entities.pos(slot);
            // One tick of movement after the spawn.
            assert!(
                horizontal(pos, player) >= 23.0,
                "spawned too close: {pos:?}"
            );
            match category(&state, *id) {
                MobCategory::Monster => {
                    assert!(tick > 420, "surface monster requires darkness");
                    monsters += 1;
                }
                MobCategory::Creature => {
                    creatures += 1;
                    assert_eq!(
                        state
                            .world
                            .biome_at(
                                pos[0].floor() as i32,
                                pos[1].floor() as i32,
                                pos[2].floor() as i32,
                            )
                            .expect("resident spawn biome")
                            .as_str(),
                        "minecraft:plains",
                    );
                    let below = state.world.block_at(
                        pos[0].floor() as i32,
                        pos[1].floor() as i32 - 1,
                        pos[2].floor() as i32,
                    );
                    assert_eq!(
                        below,
                        Some(state.registry.states.grass_block),
                        "creature on grass"
                    );
                }
                MobCategory::Misc => panic!("misc entities never spawn naturally"),
                _ => {}
            }
        }
    }
    assert!(monsters > 0, "night spawns monsters");
    assert!(creatures > 0, "creatures spawn every 400 ticks");
    assert!(
        count(&state, MobCategory::Monster) <= 70,
        "one player's monster cap"
    );
    assert!(
        count(&state, MobCategory::Creature) <= 10,
        "one player's creature cap"
    );
}

/// Far monsters and five-minute-old items despawn; creatures, near
/// monsters and fresh items stay.
#[test]
fn monsters_far_from_every_player_despawn() {
    let mut state = test_state(2);
    let player = join(&mut state);
    state.day_time = 18_000;
    let registry = Arc::clone(&state.registry);
    let far = [player[0] + 200.0, player[1], player[2]];
    let near = [player[0] + 10.0, player[1] + 5.0, player[2]];
    // Keep the far chunk resident: only the despawn rule may remove them.
    let far_chunk = crate::boundary::ChunkPos {
        x: (far[0].floor() as i32).div_euclid(16),
        z: (far[2].floor() as i32).div_euclid(16),
    };
    state.tickets.insert("far".to_owned(), (far_chunk, 0));
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let far_zombie = state.spawn_mob(registry.zombie_type_id, far, 20.0);
    let far_cow = state.spawn_mob(registry.cow_type_id, far, 10.0);
    let near_zombie = state.spawn_mob(registry.zombie_type_id, near, 20.0);
    let old_item = state.spawn_mob(registry.item_type_id, near, 1.0);
    let fresh_item = state.spawn_mob(registry.item_type_id, near, 1.0);
    let slot = state.entities.slot_of(old_item).expect("item");
    state.entities.row_mut(slot).age = 6_000;
    let mut removed = Vec::new();
    for _ in 0..crate::natural_spawn::CYCLE {
        let (output, _, _) = run_tick(&mut state, &[]);
        removed.extend(output.changes.entities_removed.iter().copied());
    }
    removed.sort_unstable_by_key(|id| id.0);
    let mut expected = vec![far_zombie, old_item];
    expected.sort_unstable_by_key(|id| id.0);
    assert_eq!(
        removed, expected,
        "the far monster and the old item despawn"
    );
    assert!(
        state.entities.slot_of(fresh_item).is_some(),
        "fresh items stay"
    );
    assert!(
        state.entities.slot_of(far_cow).is_some(),
        "creatures never despawn"
    );
    assert!(
        state.entities.slot_of(near_zombie).is_some(),
        "near monsters stay"
    );
}

/// A monster under open sky burns by day (damage and flames) and stops at
/// night.
#[test]
fn monsters_burn_under_open_sky_by_day() {
    let mut state = test_state(2);
    let player = join(&mut state);
    state.players.values_mut().next().unwrap().game_mode = crate::commands::GameMode::Creative;
    let (x, z) = (player[0].floor() as i32 + 3, player[2].floor() as i32);
    let mut y = mc_world::MAX_Y - 1;
    while state.world.block_at(x, y, z) == Some(state.registry.states.air) {
        y -= 1;
    }
    let registry = Arc::clone(&state.registry);
    let zombie = state.spawn_mob(
        registry.zombie_type_id,
        [f64::from(x) + 0.5, f64::from(y + 1), f64::from(z) + 0.5],
        20.0,
    );
    let row = |state: &TickState| {
        let slot: EntitySlot = state.entities.slot_of(zombie).expect("zombie");
        let row = state.entities.row(slot);
        (row.health, row.appearance.shared_flags & 0x01 != 0)
    };
    state.day_time = 6_000;
    for _ in 0..crate::natural_spawn::CYCLE {
        let _ = run_tick(&mut state, &[]);
    }
    assert!(row(&state).1, "sunlight ignites the retained fire timer");
    let slot = state.entities.slot_of(zombie).unwrap();
    let timer = state.entities.body(slot).burn_ticks;
    assert!(timer > 0);
    state.day_time = 18_000;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    for _ in 0..160 {
        let _ = run_tick(&mut state, &[]);
    }
    assert!(!row(&state).1, "night does not refresh daylight ignition");
}

/// A sheared sheep on grass eventually grazes: the grass under it turns to
/// dirt and its wool grows back, color kept.
#[test]
fn sheared_sheep_regrow_wool_by_grazing() {
    let mut state = test_state(2);
    let player = join(&mut state);
    let registry = Arc::clone(&state.registry);
    let sheep_type = registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").expect("id"))
        .expect("sheep");
    let grass = registry.states.grass_block;
    let (x, y, z) = (-8..=8)
        .flat_map(|dx| (-8..=8).map(move |dz| (dx, dz)))
        .find_map(|(dx, dz)| {
            let (x, z) = (player[0].floor() as i32 + dx, player[2].floor() as i32 + dz);
            let mut y = mc_world::MAX_Y - 1;
            while state.world.block_at(x, y, z) == Some(registry.states.air) {
                y -= 1;
            }
            (state.world.block_at(x, y, z) == Some(grass)).then_some((x, y, z))
        })
        .expect("grass near spawn");
    let sheep = state.spawn_mob(
        sheep_type,
        [f64::from(x) + 0.5, f64::from(y + 1), f64::from(z) + 0.5],
        8.0,
    );
    let slot = state.entities.slot_of(sheep).expect("sheep");
    crate::tick::set_wool(
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
        0x10 | 12,
    );
    for _ in 0..8_000 {
        let _ = run_tick(&mut state, &[]);
        let slot = state.entities.slot_of(sheep).expect("sheep");
        if crate::tick::wool_of(&state.entities.row(slot).appearance) == 12 {
            let pos = state.entities.pos(slot);
            let below = state.world.block_at(
                pos[0].floor() as i32,
                pos[1].floor() as i32 - 1,
                pos[2].floor() as i32,
            );
            assert_eq!(
                below,
                Some(registry.states.dirt),
                "grazed grass turns to dirt"
            );
            return;
        }
    }
    panic!("the sheep never grazed");
}

/// `[mob_griefing] sheep_grazing = false` keeps the grass block: wool still
/// regrows because vanilla's rule only guards the block change.
#[test]
fn sheep_grazing_switch_keeps_grass_while_wool_regrows() {
    let mut state = test_state(2);
    state.mob_griefing.sheep_grazing = false;
    let player = join(&mut state);
    let registry = Arc::clone(&state.registry);
    let sheep_type = registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").expect("id"))
        .expect("sheep");
    let grass = registry.states.grass_block;
    let (x, y, z) = (-8..=8)
        .flat_map(|dx| (-8..=8).map(move |dz| (dx, dz)))
        .find_map(|(dx, dz)| {
            let (x, z) = (player[0].floor() as i32 + dx, player[2].floor() as i32 + dz);
            let mut y = mc_world::MAX_Y - 1;
            while state.world.block_at(x, y, z) == Some(registry.states.air) {
                y -= 1;
            }
            (state.world.block_at(x, y, z) == Some(grass)).then_some((x, y, z))
        })
        .expect("grass near spawn");
    let sheep = state.spawn_mob(
        sheep_type,
        [f64::from(x) + 0.5, f64::from(y + 1), f64::from(z) + 0.5],
        8.0,
    );
    let slot = state.entities.slot_of(sheep).expect("sheep");
    crate::tick::set_wool(
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
        0x10 | 12,
    );
    for _ in 0..8_000 {
        let _ = run_tick(&mut state, &[]);
        let slot = state.entities.slot_of(sheep).expect("sheep");
        if crate::tick::wool_of(&state.entities.row(slot).appearance) == 12 {
            let pos = state.entities.pos(slot);
            assert_eq!(
                state.world.block_at(
                    pos[0].floor() as i32,
                    pos[1].floor() as i32 - 1,
                    pos[2].floor() as i32,
                ),
                Some(grass),
                "the disabled switch leaves grass instead of dirt"
            );
            return;
        }
    }
    panic!("the sheep never regrew wool");
}

#[test]
fn vegetation_does_not_hide_a_creature_or_monster_spawn_floor() {
    let mut state = test_state(2);
    let pos = join(&mut state);
    let x = pos[0].floor() as i32;
    let z = pos[2].floor() as i32;
    let y = pos[1].ceil() as i32;
    // This fixture state need not have every vanilla plant in its registry:
    // select a fingerprint-compatible empty-collision state that it does.
    let plant = state
        .registry
        .blocks
        .states()
        .find(|entry| {
            entry.block.id.as_str() != "minecraft:air"
                && !entry.has_fluid()
                && state.registry.materials.classify(entry.id.0) == mc_physics::BlockMaterial::Air
        })
        .expect("non-colliding plant or sign state")
        .id
        .0;
    for clear_y in y..mc_world::MAX_Y {
        state
            .world
            .set_block(x, clear_y, z, state.registry.states.air);
    }
    state
        .world
        .set_block(x, y - 1, z, state.registry.states.grass_block);
    state.world.set_block(x, y, z, plant);
    state.world.fold_overlay();
    for category in [MobCategory::Creature, MobCategory::Monster] {
        assert_eq!(
            crate::natural_spawn::spawn_floor(&state, x, z, category),
            Some(y)
        );
    }
}

struct GrassWorld {
    air: u32,
    grass: u32,
    west_biome: &'static str,
    east_biome: &'static str,
}

impl mc_world::ChunkGenerator for GrassWorld {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let biome = if pos.x < 0 {
            self.west_biome
        } else {
            self.east_biome
        };
        let mut chunk = mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.air),
            mc_data::Identifier::parse(biome).expect("fixture biome"),
        );
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 64, z, mc_world::BlockStateId(self.grass));
            }
        }
        chunk
    }
}

fn grass_state(west_biome: &'static str, east_biome: &'static str) -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(GrassWorld {
        air: registry.states.air,
        grass: registry.states.grass_block,
        west_biome,
        east_biome,
    });
    let mut world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 65, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let want = crate::world::chunks_in_radius(mc_world::ChunkPos { x: 0, z: 0 }, 5)
        .into_iter()
        .map(|pos| (pos, 0))
        .collect();
    world.ensure_blocking(&want);
    TickState::new(registry, world, 5, 512)
}

#[test]
fn vanilla_plains_creature_weights_counts_and_supported_probability_are_preserved() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let registry = CoreRegistry::load(Some(&dir)).expect("full vanilla registry");
    let plains = mc_data::Identifier::parse("minecraft:plains").expect("plains");
    let entries = registry.biome_spawns.entries(&plains, "creature");
    let expected = [
        ("minecraft:sheep", 12, 4, 4),
        ("minecraft:pig", 10, 4, 4),
        ("minecraft:chicken", 10, 4, 4),
        ("minecraft:cow", 8, 4, 4),
        ("minecraft:horse", 5, 2, 6),
        ("minecraft:donkey", 1, 1, 3),
    ];
    assert_eq!(entries.len(), expected.len());
    let mut roll = 0;
    for (entry, (name, weight, min, max)) in entries.iter().zip(expected) {
        assert_eq!(entry.entity_type.as_str(), name);
        assert_eq!(
            (entry.weight, entry.min_count, entry.max_count),
            (weight, min, max)
        );
        let type_id = registry
            .entity_types
            .id_of(&entry.entity_type)
            .expect("vanilla type");
        if registry.blueprint(type_id).is_some() {
            for _ in 0..weight {
                assert_eq!(
                    crate::natural_spawn::choose_spawn(&registry, entries, roll),
                    Some(entry)
                );
                roll += 1;
            }
        }
    }
    assert_eq!(roll, 46, "all plains creatures now have native blueprints");
    assert!(crate::natural_spawn::choose_spawn(&registry, entries, roll).is_none());
    let savanna = mc_data::Identifier::parse("minecraft:savanna").expect("savanna");
    let savanna_entries = registry.biome_spawns.exact_entries(&savanna, "creature");
    assert_eq!(savanna_entries.len(), 7);
    assert_eq!(
        &savanna_entries[..4],
        &entries[..4],
        "scenario14 farm-animal entries match plains"
    );
    assert_eq!(
        savanna_entries[4..]
            .iter()
            .map(|entry| (
                entry.entity_type.as_str(),
                entry.weight,
                entry.min_count,
                entry.max_count
            ))
            .collect::<Vec<_>>(),
        vec![
            ("minecraft:horse", 1, 2, 6),
            ("minecraft:donkey", 1, 1, 1),
            ("minecraft:armadillo", 10, 2, 3)
        ],
    );
    let snowy = mc_data::Identifier::parse("minecraft:snowy_plains").expect("snowy plains");
    assert!(
        registry
            .biome_spawns
            .entries(&snowy, "creature")
            .iter()
            .all(|entry| !matches!(
                entry.entity_type.as_str(),
                "minecraft:sheep" | "minecraft:pig" | "minecraft:chicken" | "minecraft:cow"
            )),
        "a snowy grass platform must not inherit plains farm animals"
    );
}

#[test]
fn savanna_tables_spawn_every_available_native_creature() {
    let mut state = grass_state("minecraft:savanna", "minecraft:savanna");
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    state.registry = Arc::new(CoreRegistry::load(Some(&dir)).expect("full vanilla registry"));
    let mut species = std::collections::BTreeSet::new();
    let seeds = (0..64).chain([400_u64, 800].map(|tick| tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)));
    for seed in seeds {
        clear_mobs(&mut state);
        let mut changes = crate::boundary::ChangeSet::default();
        crate::natural_spawn::census(&mut state, &[]);
        let spawned = crate::natural_spawn::spawn_group(
            &mut state,
            &[],
            [0.5, 65.0, 0.5],
            MobCategory::Creature,
            10,
            &mut crate::natural_spawn::Rng(seed),
            &mut changes,
        );
        assert!(
            spawned > 0 && spawned <= 6,
            "biome-selected herd with collision admission"
        );
        for id in changes.entities_spawned {
            let slot = state.entities.slot_of(id).expect("spawned");
            let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
                panic!("creature is a mob");
            };
            species.insert(state.registry.blueprint(type_id).expect("supported").name);
        }
    }
    assert_eq!(
        species,
        std::collections::BTreeSet::from([
            "minecraft:sheep",
            "minecraft:pig",
            "minecraft:chicken",
            "minecraft:cow",
            "minecraft:horse",
            "minecraft:donkey",
            "minecraft:armadillo",
        ]),
    );
}

#[test]
fn biome_with_only_unsupported_creatures_does_not_fall_back_to_farm_animals() {
    let mut state = grass_state("minecraft:savanna", "minecraft:savanna");
    let registry = Arc::get_mut(&mut state.registry).expect("unshared registry");
    registry.biome_spawns =
        mc_data::biomes::BiomeSpawnRules::from_entries(std::collections::BTreeMap::from([(
            mc_data::Identifier::parse("minecraft:savanna").expect("savanna"),
            std::collections::BTreeMap::from([(
                "creature".to_owned(),
                vec![mc_data::biomes::BiomeSpawnEntry {
                    entity_type: mc_data::Identifier::parse("minecraft:arrow")
                        .expect("non-creature"),
                    weight: 10,
                    min_count: 2,
                    max_count: 3,
                }],
            )]),
        )]));
    let mut changes = crate::boundary::ChangeSet::default();
    crate::natural_spawn::census(&mut state, &[]);
    assert_eq!(
        crate::natural_spawn::spawn_group(
            &mut state,
            &[],
            [0.5, 65.0, 0.5],
            MobCategory::Creature,
            10,
            &mut crate::natural_spawn::Rng(0),
            &mut changes,
        ),
        0,
    );
    assert!(changes.entities_spawned.is_empty());
}

#[test]
fn plains_creature_herds_attempt_four_members_and_respect_remaining_cap() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    for room in [10, 6, 2] {
        let mut changes = crate::boundary::ChangeSet::default();
        crate::natural_spawn::census(&mut state, &[]);
        let spawned = crate::natural_spawn::spawn_group(
            &mut state,
            &[],
            [0.5, 65.0, 0.5],
            MobCategory::Creature,
            room,
            &mut crate::natural_spawn::Rng(room as u64),
            &mut changes,
        );
        assert!(
            spawned > 0 && spawned <= room.min(4),
            "herd attempts are bounded by table and room"
        );
        assert_eq!(changes.entities_spawned.len(), spawned);
        let mut type_id = None;
        for id in changes.entities_spawned {
            let slot = state.entities.slot_of(id).expect("spawned");
            assert_eq!(state.entities.pos(slot)[1], 65.0);
            let EntityKind::Mob { type_id: kind } = state.entities.body(slot).kind else {
                panic!("natural creature is a mob");
            };
            assert_eq!(*type_id.get_or_insert(kind), kind, "one species per herd");
        }
    }
    assert!(count(&state, MobCategory::Creature) <= 10);
}

#[test]
fn creature_selection_and_group_range_use_biome_data_not_blueprint_weight() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let cow = mc_data::Identifier::parse("minecraft:cow").expect("cow");
    let registry = Arc::get_mut(&mut state.registry).expect("unshared registry");
    registry.biome_spawns =
        mc_data::biomes::BiomeSpawnRules::from_entries(std::collections::BTreeMap::from([(
            mc_data::Identifier::parse("minecraft:plains").expect("plains"),
            std::collections::BTreeMap::from([(
                "creature".to_owned(),
                vec![mc_data::biomes::BiomeSpawnEntry {
                    entity_type: cow,
                    min_count: 2,
                    max_count: 5,
                    weight: 7,
                }],
            )]),
        )]));
    for (_, blueprint) in &mut registry.blueprints {
        if blueprint.category == MobCategory::Creature {
            blueprint.spawn_weight = 0;
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..32 {
        clear_mobs(&mut state);
        let mut reference = crate::natural_spawn::Rng(seed);
        // Column angle/distance, biome entry, then inclusive herd count.
        reference.next();
        reference.next();
        reference.below(7);
        let expected = 2 + reference.below(4) as usize;
        let mut changes = crate::boundary::ChangeSet::default();
        crate::natural_spawn::census(&mut state, &[]);
        let spawned = crate::natural_spawn::spawn_group(
            &mut state,
            &[],
            [0.5, 65.0, 0.5],
            MobCategory::Creature,
            10,
            &mut crate::natural_spawn::Rng(seed),
            &mut changes,
        );
        assert!(
            spawned > 0 && spawned <= expected,
            "collision admission cannot enlarge table pack"
        );
        seen.insert(expected);
        for id in changes.entities_spawned {
            let slot = state.entities.slot_of(id).expect("spawned");
            assert_eq!(
                state.entities.body(slot).kind,
                EntityKind::Mob {
                    type_id: state.registry.cow_type_id,
                }
            );
        }
    }
    assert_eq!(seen, std::collections::BTreeSet::from([2, 3, 4, 5]));
}

#[test]
fn grass_in_an_empty_creature_biome_spawns_no_farm_animals() {
    let mut state = grass_state("minecraft:desert", "minecraft:desert");
    let mut changes = crate::boundary::ChangeSet::default();
    for seed in 0..16 {
        crate::natural_spawn::census(&mut state, &[]);
        assert_eq!(
            crate::natural_spawn::spawn_group(
                &mut state,
                &[],
                [0.5, 65.0, 0.5],
                MobCategory::Creature,
                10,
                &mut crate::natural_spawn::Rng(seed),
                &mut changes,
            ),
            0
        );
    }
    assert!(changes.entities_spawned.is_empty());
}

#[test]
fn creature_pack_members_cannot_cross_into_a_forbidden_biome() {
    let mut state = grass_state("minecraft:plains", "minecraft:desert");
    assert_eq!(
        state
            .world
            .biome_at(-1, -1, -1)
            .expect("negative quart")
            .as_str(),
        "minecraft:plains"
    );
    assert_eq!(
        state.world.biome_at(0, 65, 0).expect("east biome").as_str(),
        "minecraft:desert"
    );
    assert!(state.world.biome_at(0, mc_world::MIN_Y - 1, 0).is_none());
    assert!(state.world.biome_at(0, mc_world::MAX_Y, 0).is_none());
    assert!(state.world.biome_at(1000, 65, 1000).is_none());
    let mut total = 0;
    for seed in 0..16 {
        let mut reference = crate::natural_spawn::Rng(seed);
        let angle = reference.unit() * std::f64::consts::TAU;
        let distance = 24.0 + reference.unit() * 40.0;
        // Put the chosen column at (-1, 0), immediately west of the boundary.
        let around = [
            -0.5 - f64::from(mc_physics::mth::cos(angle)) * distance,
            65.0,
            0.5 - f64::from(mc_physics::mth::sin(angle)) * distance,
        ];
        let mut changes = crate::boundary::ChangeSet::default();
        crate::natural_spawn::census(&mut state, &[]);
        total += crate::natural_spawn::spawn_group(
            &mut state,
            &[],
            around,
            MobCategory::Creature,
            10,
            &mut crate::natural_spawn::Rng(seed),
            &mut changes,
        );
        for id in changes.entities_spawned {
            let slot = state.entities.slot_of(id).expect("spawned");
            assert!(
                state.entities.pos(slot)[0] < 0.0,
                "desert member was rejected"
            );
        }
    }
    assert!(total > 0, "allowed plains members still spawn");
    assert!(
        total < 64,
        "some four-member packs cross the boundary and are clipped"
    );
}

#[test]
fn missing_biome_table_does_not_inherit_plains_creatures() {
    let mut state = grass_state("minecraft:snowy_plains", "minecraft:snowy_plains");
    let mut changes = crate::boundary::ChangeSet::default();
    crate::natural_spawn::census(&mut state, &[]);
    assert_eq!(
        crate::natural_spawn::spawn_group(
            &mut state,
            &[],
            [0.5, 65.0, 0.5],
            MobCategory::Creature,
            10,
            &mut crate::natural_spawn::Rng(0),
            &mut changes,
        ),
        0
    );
    assert!(changes.entities_spawned.is_empty());
}

#[test]
fn unsupported_creature_roll_does_not_spawn_a_supported_substitute() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    Arc::get_mut(&mut state.registry)
        .expect("unshared registry")
        .biome_spawns =
        mc_data::biomes::BiomeSpawnRules::from_entries(std::collections::BTreeMap::from([(
            mc_data::Identifier::parse("minecraft:plains").expect("plains"),
            std::collections::BTreeMap::from([(
                "creature".to_owned(),
                vec![mc_data::biomes::BiomeSpawnEntry {
                    entity_type: mc_data::Identifier::parse("minecraft:arrow")
                        .expect("non-creature"),
                    min_count: 2,
                    max_count: 6,
                    weight: 5,
                }],
            )]),
        )]));
    let mut changes = crate::boundary::ChangeSet::default();
    crate::natural_spawn::census(&mut state, &[]);
    assert_eq!(
        crate::natural_spawn::spawn_group(
            &mut state,
            &[],
            [0.5, 65.0, 0.5],
            MobCategory::Creature,
            10,
            &mut crate::natural_spawn::Rng(0),
            &mut changes,
        ),
        0
    );
    assert!(changes.entities_spawned.is_empty());
}

#[test]
fn monster_selection_uses_biome_table_and_not_blueprint_weight() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    state.day_time = 18_000;
    let registry = Arc::get_mut(&mut state.registry).unwrap();
    registry.biome_spawns =
        mc_data::biomes::BiomeSpawnRules::from_entries(std::collections::BTreeMap::from([(
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
            std::collections::BTreeMap::from([(
                "monster".into(),
                vec![mc_data::biomes::BiomeSpawnEntry {
                    entity_type: mc_data::Identifier::parse("minecraft:skeleton").unwrap(),
                    weight: 1,
                    min_count: 2,
                    max_count: 2,
                }],
            )]),
        )]));
    for (_, blueprint) in &mut registry.blueprints {
        blueprint.spawn_weight = 0;
    }
    let mut total = 0;
    // Uniform hostile Y attempts usually fail; table selection is still
    // exercised by the successful candidates without retrying their heights.
    for seed in 0..4096 {
        clear_mobs(&mut state);
        let mut changes = crate::boundary::ChangeSet::default();
        crate::natural_spawn::census(&mut state, &[]);
        let spawned = crate::natural_spawn::spawn_group(
            &mut state,
            &[],
            [0.5, 65.0, 0.5],
            MobCategory::Monster,
            70,
            &mut crate::natural_spawn::Rng(seed),
            &mut changes,
        );
        assert!(spawned <= 2);
        total += spawned;
        for id in changes.entities_spawned {
            let slot = state.entities.slot_of(id).unwrap();
            assert_eq!(
                state.entities.body(slot).kind,
                EntityKind::Mob {
                    type_id: state
                        .registry
                        .entity_types
                        .id_of(&mc_data::Identifier::parse("minecraft:skeleton").unwrap())
                        .unwrap()
                }
            );
        }
    }
    assert!(total > 0);
}

fn clear_mobs(state: &mut TickState) {
    for index in (0..state.entities.len() as u32).rev() {
        let slot = EntitySlot(index);
        if matches!(state.entities.body(slot).kind, EntityKind::Mob { .. }) {
            state.remove_entity(slot);
        }
    }
}

#[path = "natural_spawn_breadth_tests.rs"]
mod breadth;

#[test]
fn daylight_head_items_protect_and_only_damageable_head_equipment_wears() {
    let mut state = test_state(2);
    let mut player = join(&mut state);
    player[1] = 250.0;
    let observer = *state.players.keys().next().unwrap();
    let observer_slot = state.entities.slot_of(observer).unwrap();
    state.entities.set_pos(observer_slot, player);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.day_time = 6_000;
    let pos = [player[0] + 3.0, 250.0, player[2]];
    let zombie = state.spawn_mob(state.registry.zombie_type_id, pos, 20.0);
    let slot = state.entities.slot_of(zombie).unwrap();
    let helmet_id = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_helmet").unwrap())
        .unwrap();
    let stone_id = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let helmet = mc_data::ItemStack::new(helmet_id, 1);
    let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
    appearance.equipment[5] = Some(helmet.clone());
    appearance.equipment[4] = Some(helmet.clone());
    let mut worn = false;
    for pass in 1..=32 {
        state.tick = pass * crate::natural_spawn::CYCLE;
        let mut changes = crate::boundary::ChangeSet::default();
        crate::natural_spawn::run(&mut state, &mut changes);
        let row = state.entities.row(slot);
        assert_eq!(
            state.entities.body(slot).burn_ticks,
            0,
            "head equipment prevents sunlight ignition"
        );
        assert_eq!(
            row.appearance.equipment[4],
            Some(helmet.clone()),
            "sunlight must not wear other slots"
        );
        if row.appearance.equipment[5]
            .as_ref()
            .unwrap()
            .damage
            .unwrap_or(0)
            > 0
        {
            assert!(changes.entities_appearance_changed.contains(&zombie));
            worn = true;
            break;
        }
    }
    assert!(
        worn,
        "vanilla nextInt(2) eventually wears a damageable head item"
    );
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[5] =
        Some(mc_data::ItemStack::new(stone_id, 1));
    state.tick += crate::natural_spawn::CYCLE;
    crate::natural_spawn::run(&mut state, &mut crate::boundary::ChangeSet::default());
    assert_eq!(
        state.entities.body(slot).burn_ticks,
        0,
        "a nondamageable head item also protects"
    );
    assert_eq!(
        state.entities.row(slot).appearance.equipment[5],
        Some(mc_data::ItemStack::new(stone_id, 1))
    );
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[5] =
        Some(helmet.with_max_damage(1).unwrap());
    for _ in 0..32 {
        state.tick += crate::natural_spawn::CYCLE;
        crate::natural_spawn::run(&mut state, &mut crate::boundary::ChangeSet::default());
        assert_eq!(
            state.entities.body(slot).burn_ticks,
            0,
            "breaking head equipment protects this pass"
        );
        if state.entities.row(slot).appearance.equipment[5].is_none() {
            break;
        }
    }
    assert!(state.entities.row(slot).appearance.equipment[5].is_none());
    state.tick += crate::natural_spawn::CYCLE;
    crate::natural_spawn::run(&mut state, &mut crate::boundary::ChangeSet::default());
    assert!(
        state.entities.body(slot).burn_ticks > 0,
        "the next unprotected pass ignites"
    );
    state.entities.body_mut(slot).burn_ticks = 0;
    state.day_time = 18_000;
    state.tick += crate::natural_spawn::CYCLE;
    crate::natural_spawn::run(&mut state, &mut crate::boundary::ChangeSet::default());
    assert_eq!(
        state.entities.body(slot).burn_ticks,
        0,
        "night never ignites an unprotected mob"
    );
}

#[test]
fn despawn_pass_only_resets_retained_idle_clocks_in_active_columns() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let around = join(&mut state);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let zombie = state.registry.zombie_type_id;
    let mut ids = Vec::new();
    for dx in [0.0, 160.0, 0.0, 160.0] {
        let id = state.spawn_mob(zombie, [around[0] + dx, around[1], around[2]], 20.0);
        let row = state.entities.row_mut(state.entities.slot_of(id).unwrap());
        row.persistent = true;
        row.no_action_time = 77;
        ids.push(id);
    }
    simulate_fixture_chunks(&mut state);
    let active = std::iter::once(crate::boundary::ChunkPos {
        x: (around[0].floor() as i32).div_euclid(16),
        z: (around[2].floor() as i32).div_euclid(16),
    })
    .collect();
    state.world.set_active_chunks(active, state.tick);
    crate::natural_spawn::advance_ages(&mut state, &mut crate::boundary::ChangeSet::default());
    crate::natural_spawn::run(&mut state, &mut crate::boundary::ChangeSet::default());
    for (index, id) in ids.into_iter().enumerate() {
        let row = state.entities.row(state.entities.slot_of(id).unwrap());
        assert_eq!(row.no_action_time, if index % 2 == 0 { 0 } else { 77 });
        assert_eq!(row.age, u32::from(index % 2 == 0));
    }
}

#[test]
fn age_admission_preserves_global_player_clock_and_reverse_item_removal() {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let around = join(&mut state);
    let active = state.spawn_mob(state.registry.item_type_id, around, 1.0);
    let inactive = state.spawn_mob(
        state.registry.item_type_id,
        [around[0] + 320.0, around[1], around[2]],
        1.0,
    );
    for id in [active, inactive] {
        state
            .entities
            .row_mut(state.entities.slot_of(id).unwrap())
            .age = 5999;
    }
    simulate_fixture_chunks(&mut state);
    state.world.set_active_chunks(
        std::iter::once(crate::boundary::ChunkPos {
            x: (around[0].floor() as i32).div_euclid(16),
            z: (around[2].floor() as i32).div_euclid(16),
        })
        .collect(),
        state.tick,
    );
    let mut changes = crate::boundary::ChangeSet::default();
    crate::natural_spawn::advance_ages(&mut state, &mut changes);
    assert_eq!(changes.entities_removed, vec![active]);
    let slot = state.entities.slot_of(inactive).unwrap();
    assert_eq!(state.entities.row(slot).age, 5999);
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    let player_slot = state.entities.slot_of(player).unwrap();
    let before = state.entities.row(player_slot).age;
    state
        .world
        .set_active_chunks(Default::default(), state.tick);
    crate::natural_spawn::advance_ages(&mut state, &mut changes);
    assert_eq!(state.entities.row(player_slot).age, before + 1);
    assert_eq!(state.entities.row(slot).age, 5999);
}

fn fused_fixture(
    peaceful: bool,
    expiring_items: bool,
    filler: usize,
) -> (TickState, Vec<EntityId>) {
    let mut state = grass_state("minecraft:plains", "minecraft:plains");
    let around = join(&mut state);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    if peaceful {
        state.difficulty = crate::survival::Difficulty::Peaceful;
    }
    let zombie = state.registry.zombie_type_id;
    let cow = state.registry.cow_type_id;
    let item = state.registry.item_type_id;
    let mut ids = Vec::new();
    for (index, dx) in [0.0, 40.0, 200.0, 60.0, 400.0, 0.0, 90.0, 260.0]
        .into_iter()
        .enumerate()
    {
        let at = [around[0] + dx, around[1], around[2]];
        let id = state.spawn_mob(if index % 3 == 2 { cow } else { zombie }, at, 20.0);
        let row = state.entities.row_mut(state.entities.slot_of(id).unwrap());
        row.persistent = index % 4 == 1;
        row.no_action_time = if index % 2 == 0 { 0 } else { 599 };
        ids.push(id);
        let stack = state.spawn_mob(item, at, 1.0);
        let row = state
            .entities
            .row_mut(state.entities.slot_of(stack).unwrap());
        row.age = if expiring_items && index % 3 != 0 {
            5999
        } else {
            10
        };
        ids.push(stack);
    }
    // Filler rows make the population large enough for the pool to split it.
    for index in 0..filler {
        let at = [
            around[0] + 20.0 + (index % 60) as f64,
            around[1],
            around[2] + 1.0,
        ];
        let id = if index % 700 == 0 {
            let id = state.spawn_mob(item, at, 1.0);
            let row = state.entities.row_mut(state.entities.slot_of(id).unwrap());
            row.age = 5999;
            id
        } else {
            let id = state.spawn_mob(cow, at, 10.0);
            let row = state.entities.row_mut(state.entities.slot_of(id).unwrap());
            row.persistent = index % 5 != 0;
            row.no_action_time = (index % 3) as u32;
            id
        };
        ids.push(id);
    }
    simulate_fixture_chunks(&mut state);
    // The player's own column and the farthest columns stay inactive: their
    // rows skip the age clock and despawn, while the player body keeps its
    // global age clock (the extra global-player admission slot).
    let active = state
        .entities
        .slots()
        .map(|slot| state.entities.pos(slot))
        .filter(|pos| pos[0] > around[0] + 20.0 && pos[0] < around[0] + 300.0)
        .map(|pos| crate::boundary::ChunkPos {
            x: (pos[0].floor() as i32).div_euclid(16),
            z: (pos[2].floor() as i32).div_euclid(16),
        })
        .collect();
    state.world.set_active_chunks(active, state.tick);
    (state, ids)
}

#[test]
fn fused_age_and_despawn_match_the_separate_passes() {
    for (peaceful, expiring_items, min_parallel, filler) in [false, true]
        .into_iter()
        .flat_map(|peaceful| [false, true].map(|expiring| (peaceful, expiring)))
        .flat_map(|(peaceful, expiring)| [0, usize::MAX].map(|min| (peaceful, expiring, min)))
        .flat_map(|(peaceful, expiring, min)| {
            [0, 9_000].map(|filler| (peaceful, expiring, min, filler))
        })
    {
        {
            let (mut fused, ids) = fused_fixture(peaceful, expiring_items, filler);
            let (mut reference, _) = fused_fixture(peaceful, expiring_items, filler);
            // Peaceful monsters leave at once; odd-index idle clocks start at 599; the
            // non-persistent zombie 60 blocks out then reaches the random idle despawn.
            // Large filler populations only need a few passes to differ.
            for tick in 0..if filler == 0 { 140 } else { 6 } {
                fused.tick = tick;
                reference.tick = tick;
                let mut fused_changes = crate::boundary::ChangeSet::default();
                let mut reference_changes = crate::boundary::ChangeSet::default();
                crate::natural_spawn::advance_ages_and_run_for_test(
                    &mut fused,
                    &mut fused_changes,
                    min_parallel,
                );
                crate::natural_spawn::advance_ages_reference(
                    &mut reference,
                    &mut reference_changes,
                );
                crate::natural_spawn::run(&mut reference, &mut reference_changes);
                assert_eq!(
                    fused_changes.entities_removed, reference_changes.entities_removed,
                    "removals peaceful={peaceful} expiring={expiring_items} pool={min_parallel} filler={filler} tick={tick}"
                );
                assert_eq!(fused.entities.len(), reference.entities.len());
                for slot in fused.entities.slots() {
                    let (left, right) = (fused.entities.row(slot), reference.entities.row(slot));
                    let (left_body, right_body) =
                        (fused.entities.body(slot), reference.entities.body(slot));
                    assert_eq!(
                        (left_body.id, left.age, left.no_action_time, left_body.alive),
                        (
                            right_body.id,
                            right.age,
                            right.no_action_time,
                            right_body.alive
                        ),
                        "slot {slot:?} tick {tick}"
                    );
                }
            }
            if peaceful || expiring_items {
                assert!(
                    ids.iter().any(|id| fused.entities.slot_of(*id).is_none()),
                    "scenario removes something"
                );
            }
        }
    }
}

#[test]
fn despawn_prefilter_never_skips_a_row_the_full_decision_acts_on() {
    for peaceful in [false, true] {
        let (mut state, ids) = fused_fixture(peaceful, false, 0);
        let zombie = state.registry.zombie_type_id;
        let at = state.entities.pos(state.entities.slot_of(ids[0]).unwrap());
        for (persistent, tamed, named, warden, idle, no_players) in (0..64).map(|bits: u32| {
            (
                bits & 1 != 0,
                bits & 2 != 0,
                bits & 4 != 0,
                bits & 8 != 0,
                if bits & 16 != 0 { 5 } else { 0 },
                bits & 32 != 0,
            )
        }) {
            for type_id in [zombie, state.registry.cow_type_id] {
                let id = state.spawn_mob(type_id, at, 20.0);
                let slot = state.entities.slot_of(id).unwrap();
                let row = state.entities.row_mut(slot);
                row.persistent = persistent;
                row.entity_life.tamed = tamed;
                row.no_action_time = idle;
                if named {
                    let mut appearance = (*row.appearance).clone();
                    appearance.custom_name = Some("named".into());
                    row.appearance = Arc::new(appearance);
                }
                if warden {
                    row.special_mob = Some(Box::new(crate::special_mobs::Retained::Warden(
                        Default::default(),
                    )));
                }
                let row = state.entities.row(slot);
                let category = crate::natural_spawn::category_of(&state, slot);
                let acts_on = (category == MobCategory::Monster && peaceful)
                    || !(no_players
                        || (crate::natural_spawn::persistent(row)
                            || category.is_persistent()
                            || matches!(
                                row.special_mob.as_deref(),
                                Some(crate::special_mobs::Retained::Warden(_))
                            ))
                            && row.no_action_time == 0);
                if crate::natural_spawn::no_work_before_category(row, peaceful, no_players) {
                    assert!(!acts_on, "prefilter skipped an acting row");
                }
            }
        }
    }
}
