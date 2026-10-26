use super::*;
use crate::boundary::EntityId;

fn fixture(name: &str) -> (TickState, EntityId, Key) {
    let (mut state, player, pos) = crate::hostile_tests::setup(10.0);
    state.day_time = 18_000;
    let key = (pos[0] as i32, pos[1] as i32 + 1, pos[2] as i32);
    let id = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:spawner").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(key.0, key.1, key.2, id);
    let fields = vec![
        ("Delay".into(), Tag::Short(0)),
        (
            "SpawnData".into(),
            Tag::Compound(vec![(
                "entity".into(),
                Tag::Compound(vec![(
                    "id".into(),
                    Tag::String(format!("minecraft:{name}")),
                )]),
            )]),
        ),
    ];
    let pos = BlockPos {
        x: key.0,
        y: key.1,
        z: key.2,
    };
    let bytes = nbt::encode(fields, "minecraft:mob_spawner", [key.0, key.1, key.2]);
    state.world.set_opaque_block_entity(pos, Some(bytes));
    state.world.fold_overlay();
    let chunk = state
        .world
        .chunk(ChunkPos {
            x: key.0.div_euclid(16),
            z: key.2.div_euclid(16),
        })
        .unwrap();
    hydrate_chunk(&mut state.monster_spawners, &state.registry, chunk).unwrap();
    (state, player, key)
}
fn entry(state: &TickState, key: Key) -> &Spawner {
    &state.monster_spawners.chunks[&(key.0.div_euclid(16), key.2.div_euclid(16))][&key]
}
fn step(state: &mut TickState) -> ChangeSet {
    let mut changes = ChangeSet::default();
    run(state, &mut changes);
    state.tick += 1;
    changes
}
#[test]
fn regular_spawner_activates_spawns_and_respects_local_cap() {
    let (mut state, _, key) = fixture("silverfish");
    let mut spawned = Vec::new();
    for _ in 0..1600 {
        spawned.extend(step(&mut state).entities_spawned);
    }
    assert!(!spawned.is_empty());
    assert!(
        spawned.len() <= 6,
        "nearby cap includes previous successful attempts"
    );
    let spawner = entry(&state, key);
    assert!((200..800).contains(&spawner.delay));
    assert!(spawned.iter().all(|id| {
        !state
            .entities
            .row(state.entities.slot_of(*id).unwrap())
            .persistent
    }));
}
#[test]
fn regular_spawner_pauses_without_players_and_rehydrates_deadline() {
    let (mut state, player, key) = fixture("silverfish");
    step(&mut state);
    let deadline = entry(&state, key).deadline.unwrap();
    assert!(deadline > state.tick);
    for _ in 0..30 {
        step(&mut state);
    }
    let pos = BlockPos {
        x: key.0,
        y: key.1,
        z: key.2,
    };
    let chunk_pos = ChunkPos {
        x: key.0.div_euclid(16),
        z: key.2.div_euclid(16),
    };
    state.world.fold_overlay();
    let bytes = state.world.chunk(chunk_pos).unwrap().block_entities[&pos].clone();
    let restored = Spawner::decode(&bytes, &state.registry).unwrap();
    assert_eq!(restored.deadline, Some(deadline));
    let slot = state.entities.slot_of(player).unwrap();
    state
        .entities
        .set_pos(slot, [key.0 as f64 + 40.0, key.1 as f64, key.2 as f64]);
    step(&mut state);
    let paused = entry(&state, key).delay;
    assert!(entry(&state, key).deadline.is_none());
    for _ in 0..100 {
        assert!(step(&mut state).entities_spawned.is_empty());
    }
    assert_eq!(entry(&state, key).delay, paused);
    state
        .entities
        .set_pos(slot, [key.0 as f64 + 10.0, key.1 as f64, key.2 as f64]);
    let now = state.tick;
    step(&mut state);
    assert_eq!(entry(&state, key).deadline, Some(now + paused));
}
#[test]
fn regular_spawner_rejects_light_solid_space_and_peaceful() {
    let (mut state, _, key) = fixture("zombie");
    let type_id = entry(&state, key).type_id;
    let at = [key.0 as f64 + 3.5, key.1 as f64, key.2 as f64 + 3.5];
    state.day_time = 6000;
    assert!((0..64).all(|seed| !can_spawn(
        &state,
        type_id,
        at,
        &mut crate::natural_spawn::Rng(seed),
        None,
    )));
    state.day_time = 18000;
    assert!((0..64).any(|seed| can_spawn(
        &state,
        type_id,
        at,
        &mut crate::natural_spawn::Rng(seed),
        None,
    )));
    state.world.set_block(
        at[0] as i32,
        at[1] as i32 + 1,
        at[2] as i32,
        state.registry.states.stone,
    );
    assert!((0..64).all(|seed| !can_spawn(
        &state,
        type_id,
        at,
        &mut crate::natural_spawn::Rng(seed),
        None,
    )));
    state.world.set_block(
        at[0] as i32,
        at[1] as i32 + 1,
        at[2] as i32,
        state.registry.states.air,
    );
    state.difficulty = crate::survival::Difficulty::Peaceful;
    assert!((0..64).all(|seed| !can_spawn(
        &state,
        type_id,
        at,
        &mut crate::natural_spawn::Rng(seed),
        None,
    )));
}
#[test]
fn regular_spawner_corrupt_nbt_fails_admission() {
    let (state, _, key) = fixture("blaze");
    let mut fields = entry(&state, key).fields.clone();
    nbt::set(&mut fields, "SpawnRange", Tag::Short(32767));
    let bytes = nbt::encode(fields, "minecraft:mob_spawner", [key.0, key.1, key.2]);
    assert!(Spawner::decode(&bytes, &state.registry).is_err());
}
#[test]
fn regular_spawner_break_removes_runtime_without_recreating_nbt() {
    let (mut state, _, key) = fixture("silverfish");
    step(&mut state);
    let pos = BlockPos {
        x: key.0,
        y: key.1,
        z: key.2,
    };
    let air = state.registry.states.air;
    state.world.set_block(pos.x, pos.y, pos.z, air);
    after_blocks(&mut state, &[(pos, air)]);
    state.opaque_changes.clear();
    step(&mut state);
    assert!(state.opaque_changes.is_empty());
}

#[test]
fn regular_spawner_counts_monsters_in_resident_index() {
    let (mut state, _, key) = fixture("zombie_villager");
    let type_id = entry(&state, key).type_id;
    for offset in 0..6 {
        state.spawn_mob(
            type_id,
            [
                key.0 as f64 + offset as f64 * 0.5,
                key.1 as f64,
                key.2 as f64,
            ],
            20.0,
        );
    }
    assert_eq!(nearby(&state, key, type_id, 4, 6), 6);
    assert!(step(&mut state).entities_spawned.is_empty());
    assert!(
        entry(&state, key)
            .deadline
            .is_some_and(|deadline| deadline > state.tick)
    );
}

#[test]
fn fix_mobs_passive_spawner_hydrates_and_spawns_in_peaceful() {
    let (mut state, _, key) = fixture("cow");
    state.difficulty = crate::survival::Difficulty::Peaceful;
    state.day_time = 6000;
    for x in key.0 - 5..=key.0 + 5 {
        for z in key.2 - 5..=key.2 + 5 {
            state
                .world
                .set_block(x, key.1 - 2, z, state.registry.states.grass_block);
        }
    }
    assert!((0..100).any(|_| !step(&mut state).entities_spawned.is_empty()));
}

#[test]
fn fix_mobs_spawner_accepts_custom_light_rules() {
    let (mut state, _, key) = fixture("zombie");
    let mut fields = entry(&state, key).fields.clone();
    let (_, Tag::Compound(spawn)) = fields
        .iter_mut()
        .find(|(key, _)| key == "SpawnData")
        .unwrap()
    else {
        panic!("SpawnData")
    };
    spawn.push((
        "custom_spawn_rules".into(),
        Tag::Compound(vec![
            (
                "block_light_limit".into(),
                Tag::Compound(vec![
                    ("min_inclusive".into(), Tag::Int(0)),
                    ("max_inclusive".into(), Tag::Int(15)),
                ]),
            ),
            (
                "sky_light_limit".into(),
                Tag::Compound(vec![
                    ("min_inclusive".into(), Tag::Int(0)),
                    ("max_inclusive".into(), Tag::Int(15)),
                ]),
            ),
        ]),
    ));
    let bytes = nbt::encode(fields, "minecraft:mob_spawner", [key.0, key.1, key.2]);
    let spawner = Spawner::decode(&bytes, &state.registry).unwrap();
    state.day_time = 6000;
    let at = [key.0 as f64 + 3.5, key.1 as f64, key.2 as f64 + 3.5];
    assert!(can_spawn(
        &state,
        spawner.type_id,
        at,
        &mut crate::natural_spawn::Rng(1),
        spawner.light_rules
    ));
    assert!(!can_spawn(
        &state,
        spawner.type_id,
        at,
        &mut crate::natural_spawn::Rng(1),
        Some([[1, 15], [0, 15]])
    ));
    state.difficulty = crate::survival::Difficulty::Peaceful;
    assert!(!can_spawn(
        &state,
        spawner.type_id,
        at,
        &mut crate::natural_spawn::Rng(1),
        spawner.light_rules
    ));
}

#[test]
fn fix_mobs_weighted_spawn_potential_replaces_current_data_on_reset() {
    let (state, _, key) = fixture("silverfish");
    let mut fields = entry(&state, key).fields.clone();
    let cow = Tag::Compound(vec![(
        "entity".into(),
        Tag::Compound(vec![("id".into(), Tag::String("minecraft:cow".into()))]),
    )]);
    nbt::set(
        &mut fields,
        "SpawnPotentials",
        Tag::List(mc_nbt::ListTag {
            element_type: 10,
            elements: vec![Tag::Compound(vec![
                ("weight".into(), Tag::Int(1)),
                ("data".into(), cow.clone()),
            ])],
        }),
    );
    let bytes = nbt::encode(fields, "minecraft:mob_spawner", [key.0, key.1, key.2]);
    let mut spawner = Spawner::decode(&bytes, &state.registry).unwrap();
    spawner.reset(100, &mut crate::natural_spawn::Rng(1));
    assert_eq!(field(&spawner.fields, "SpawnData"), Some(&cow));
    assert_eq!(
        state
            .registry
            .entity_types
            .name_of(spawner.type_id)
            .unwrap()
            .as_str(),
        "minecraft:cow"
    );
    let recovered = Spawner::decode(
        &nbt::encode(
            spawner.fields,
            "minecraft:mob_spawner",
            [key.0, key.1, key.2],
        ),
        &state.registry,
    )
    .unwrap();
    assert_eq!(recovered.type_id, spawner.type_id);
}

#[test]
fn fix_mobs_turtle_spawner_uses_sand_and_sea_level_not_grass() {
    let (mut state, _, key) = fixture("turtle");
    state.day_time = 6000;
    let type_id = entry(&state, key).type_id;
    let sand = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:sand").unwrap())
        .unwrap()
        .default
        .0;
    let x = key.0 + 14;
    let z = key.2 + 14;
    for (y, support, expected) in [
        (64, sand, true),
        (67, sand, false),
        (64, state.registry.states.grass_block, false),
    ] {
        state.world.set_block(x, y - 1, z, support);
        assert_eq!(
            can_spawn(
                &state,
                type_id,
                [x as f64 + 0.5, y as f64, z as f64 + 0.5],
                &mut crate::natural_spawn::Rng(1),
                None
            ),
            expected
        );
        state
            .world
            .set_block(x, y - 1, z, state.registry.states.air);
    }
}
