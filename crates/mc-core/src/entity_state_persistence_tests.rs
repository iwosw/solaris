use super::*;
use crate::journal::{
    EntityStateImage, JournalEntry, JournalWriter, Replay, ReplayEntity,
    read_complete_batches_after, replay_batches,
};

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.0),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}
fn fixture() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 65, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(
        &[(mc_world::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    let mut state = TickState::new(registry, world, 2, 512);
    state.tickets.insert(
        "entity-state-test".into(),
        (mc_world::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.refresh_active_chunks(state.tick);
    state
}
fn replay_record(state: &TickState, native: &mc_data::entity_nbt::VanillaEntity) -> ReplayEntity {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(&native.id).unwrap())
        .unwrap();
    let image = EntityStateImage::from_record(&state.registry, native).unwrap();
    let mut appearance = appearance_from_vanilla_entity(native);
    appearance.baby = image.entity_life.child_ticks != 0;
    ReplayEntity {
        uuid: native.uuid.to_string(),
        type_id,
        pos: native.pos,
        vel: velocity_from_vanilla_motion(native.motion),
        yaw: native.yaw,
        pitch: native.pitch,
        ground: native.on_ground,
        health: native.health,
        alive: true,
        age: image.age,
        breed_cooldown: image.breed_cooldown,
        pickup_delay: native.pickup_delay as u32,
        item: native.item.clone(),
        appearance,
        active_effects: Default::default(),
        villager: None,
        state: image,
        projectile: None,
        tick: 1,
    }
}

#[test]
fn mount_cargo_region_roundtrip_uses_current_custody_not_imported_items() {
    use mc_nbt::{ListTag, Tag, tag_type};
    for name in ["donkey", "mule", "llama", "trader_llama"] {
        let mut state = fixture();
        let diamond = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:diamond").unwrap())
            .unwrap();
        let mut cargo = mc_data::ItemStack::new(diamond, 7);
        cargo.custom_name = Some("retained cargo".into());
        let mut item_fields =
            mc_data::item_nbt::stack_to_fields(&state.registry.items, &cargo).unwrap();
        item_fields.push(("Slot".into(), Tag::Byte(0)));
        let native = mc_data::entity_nbt::VanillaEntity {
            id: format!("minecraft:{name}"),
            uuid: uuid::Uuid::new_v4(),
            wire_id: 200,
            pos: [8.5, 65.0, 8.5],
            health: 20.0,
            extra_fields: vec![
                ("ChestedHorse".into(), Tag::Byte(1)),
                ("Strength".into(), Tag::Int(3)),
                (
                    "Items".into(),
                    Tag::List(ListTag {
                        element_type: tag_type::COMPOUND,
                        elements: vec![Tag::Compound(item_fields)],
                    }),
                ),
                ("test:opaque".into(), Tag::String("kept".into())),
            ],
            ..Default::default()
        };
        let record = replay_record(&state, &native);
        restore_entity(&mut state, 200, &record).unwrap();
        let slot = state.entities.slot_of(EntityId(200)).unwrap();
        assert_eq!(state.entities.row(slot).entity_life.mount.cargo[0], cargo);
        let dir = tempfile::tempdir().unwrap();
        for chested in [true, false] {
            let mount = &mut state.entities.row_mut(slot).entity_life.mount;
            mount.chested = chested;
            if chested {
                mount.cargo[0].count = 3;
                mount.cargo[1] = cargo.clone();
            } else {
                mount.cargo.clear();
            }
            let expected = mount.clone();
            let (_, regions, _) = checkpoint_data_for(&state, 1);
            mc_world::anvil::entity_regions::write_entity_regions(
                dir.path(),
                &state.registry.items,
                &regions,
                1,
            )
            .unwrap();
            let loaded = mc_world::anvil::entity_regions::read_entity_regions(
                dir.path(),
                &state.registry.items,
            )
            .unwrap();
            let exported = loaded
                .values()
                .flat_map(|chunk| &chunk.entities)
                .find(|entity| entity.uuid == native.uuid)
                .unwrap();
            assert!(
                exported
                    .extra_fields
                    .contains(&("test:opaque".into(), Tag::String("kept".into()),))
            );
            assert_eq!(
                exported
                    .extra_fields
                    .iter()
                    .filter(|(key, _)| key == "Items")
                    .count(),
                usize::from(chested),
                "never retain a duplicate or stale opaque Items field",
            );
            let restored = EntityStateImage::from_record(&state.registry, exported).unwrap();
            assert_eq!(restored.entity_life.mount.chested, expected.chested);
            assert_eq!(restored.entity_life.mount.cargo, expected.cargo);
            if name.ends_with("llama") {
                assert_eq!(restored.entity_life.mount.strength, expected.strength);
            }
        }
    }
}

#[test]
fn actual_checkpoint_regions_restore_persistence_idle_air_fire_and_child_size() {
    let mut state = fixture();
    let skeleton = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:skeleton").unwrap())
        .unwrap();
    let id = state.spawn_mob(skeleton, [31.5, 65.0, -0.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    let (body, row) = state.entities.parts_mut(slot);
    row.persistent = true;
    row.no_action_time = 712;
    body.air_supply = 123;
    body.burn_ticks = 77;
    row.age = 123;
    row.protected_until = 61;
    let cow = state.spawn_mob(state.registry.cow_type_id, [-0.5, 65.0, 31.5], 10.0);
    let child = state.entities.slot_of(cow).unwrap();
    let row = state.entities.row_mut(child);
    row.age = 99;
    row.love = 321;
    row.breed_cooldown = 420;
    row.entity_life.child_ticks = 1234;
    row.entity_life.harvest_ticks = 456;
    Arc::make_mut(&mut row.appearance).baby = true;
    let expected_mob =
        EntityStateImage::from_row(state.entities.body(slot), state.entities.row(slot));
    let expected_child =
        EntityStateImage::from_row(state.entities.body(child), state.entities.row(child));
    let (_, regions, _) = checkpoint_data_for(&state, 1);
    let dir = std::env::temp_dir().join(format!("solaris-spawn-state-{}", uuid::Uuid::new_v4()));
    mc_world::anvil::entity_regions::write_entity_regions(&dir, &state.registry.items, &regions, 1)
        .unwrap();
    let regions =
        mc_world::anvil::entity_regions::read_entity_regions(&dir, &state.registry.items).unwrap();
    let mut restored = fixture();
    for native in regions.values().flat_map(|chunk| &chunk.entities) {
        let record = replay_record(&restored, native);
        restore_entity(&mut restored, native.wire_id, &record).unwrap();
        let slot = restored.entities.slot_of(EntityId(native.wire_id)).unwrap();
        let body = restored.entities.body(slot);
        let row = restored.entities.row(slot);
        if native.id == "minecraft:cow" {
            assert_eq!(native.age, -1234, "Age is child age, not lifetime age");
            assert_eq!(EntityStateImage::from_row(body, row), expected_child);
            let blueprint = restored.registry.blueprint(record.type_id).unwrap();
            assert!(row.appearance.baby);
            assert_eq!(body.height, blueprint.height * 0.5);
            assert_eq!(body.half_width, blueprint.half_width * 0.5);
        } else {
            assert_eq!(EntityStateImage::from_row(body, row), expected_mob);
            assert!(
                native
                    .extra_fields
                    .contains(&("PersistenceRequired".into(), mc_nbt::Tag::Byte(1)))
            );
            assert!(
                native
                    .extra_fields
                    .contains(&("Fire".into(), mc_nbt::Tag::Short(77)))
            );
            assert!(
                native
                    .extra_fields
                    .contains(&("Air".into(), mc_nbt::Tag::Short(123)))
            );
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn compact_timer_postimages_replay_without_appearance_or_birth_motion_changes() {
    let mut state = fixture();
    let id = state.spawn_mob(state.registry.cow_type_id, [31.5, 65.0, 31.5], 10.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).persistent = true;
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    Arc::get_mut(&mut output.changes)
        .expect("unique tick change set")
        .entities_spawned
        .push(id);
    let birth = journal_entries_for_tick(&mut state, &output);
    assert!(
        birth.iter().any(
            |entry| matches!(entry, JournalEntry::Entity { id: entity, .. } if *entity == id.0)
        )
    );
    let (body, row) = state.entities.parts_mut(slot);
    row.no_action_time = 701;
    body.air_supply = 112;
    row.breed_cooldown = 601;
    row.love = 99;
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let expected = EntityStateImage::from_row(state.entities.body(slot), state.entities.row(slot));
    let tail = journal_entries_for_tick(&mut state, &output);
    assert!(tail.iter().any(
        |entry| matches!(entry, JournalEntry::EntityState { id: entity, .. } if *entity == id.0)
    ));
    let compact_state = tail
        .iter()
        .find_map(|entry| match entry {
            JournalEntry::EntityState {
                id: entity, state, ..
            } if *entity == id.0 => Some(state),
            _ => None,
        })
        .expect("compact retained state");
    assert_eq!(
        serde_json::to_vec(compact_state).unwrap(),
        serde_json::to_vec(&expected).unwrap(),
        "boxing must preserve the unboxed retained-image bytes"
    );
    assert!(
        !tail.iter().any(
            |entry| matches!(entry, JournalEntry::Entity { id: entity, .. } if *entity == id.0)
        )
    );
    let dir =
        std::env::temp_dir().join(format!("solaris-spawn-state-wal-{}", uuid::Uuid::new_v4()));
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, 1, &birth).unwrap();
    writer.append(2, 2, &tail).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let replay = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&dir.join("journal"), 0).unwrap(),
    );
    let record = &replay.entities[&id.0];
    assert_eq!(record.state, expected);
    assert_eq!(record.breed_cooldown, expected.breed_cooldown);
    assert_eq!(record.pos, [31.5, 65.0, 31.5]);
    let mut restored = fixture();
    restore_entity(&mut restored, id.0, record).unwrap();
    let slot = restored.entities.slot_of(id).unwrap();
    assert_eq!(
        EntityStateImage::from_row(restored.entities.body(slot), restored.entities.row(slot)),
        expected
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn native_falling_checkpoint_restores_block_state_and_stationary_control() {
    let mut state = fixture();
    let sand = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:sand").unwrap())
        .unwrap()
        .default
        .0;
    let falling = crate::falling::FallingState {
        block_state: sand,
        age: 37,
        touched_water: true,
    };
    let native = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:falling_block".into(),
        wire_id: 42,
        pos: [8.5, 75.0, 8.5],
        extra_fields: crate::falling::to_fields(&falling, &state.registry.blocks),
        ..Default::default()
    };
    let record = replay_record(&state, &native);
    restore_entity(&mut state, 42, &record).unwrap();
    let slot = state.entities.slot_of(EntityId(42)).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    assert_eq!(row.falling, Some(falling));
    assert_eq!(body.movement, crate::blueprint::MovementClass::Stationary);
    assert_eq!((body.half_width, body.height), (0.49, 0.98));
    assert_eq!(row.appearance.spawn_data, sand as i32);
    let (_, checkpoint, _) = checkpoint_data_for(&state, 1);
    let native = &checkpoint[&(0, 0)].entities[0];
    let fields = mc_data::entity_nbt::entity_to_fields(&state.registry.items, native).unwrap();
    let decoded =
        mc_data::entity_nbt::entity_from_fields(&state.registry.items, Default::default(), &fields)
            .unwrap();
    let image = EntityStateImage::from_record(&state.registry, &decoded).unwrap();
    assert_eq!(image.falling, Some(falling));
    assert!(image.persistent);
}

fn age_base(age: u32, tick: u64) -> Replay {
    let mut state = fixture();
    let id = state.spawn_mob(state.registry.cow_type_id, [8.5, 65.0, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).age = age;
    let (_, chunks, _) = checkpoint_data_for(&state, tick);
    let native = &chunks[&(0, 0)].entities[0];
    let mut record = replay_record(&state, native);
    record.tick = tick;
    Replay {
        tick,
        entities: std::collections::BTreeMap::from([(id.0, record)]),
        sim_debt: active_clock(tick),
        ..Default::default()
    }
}

#[test]
fn lifetime_age_empty_tail_wraps_and_repeated_fold_is_idempotent() {
    let base = age_base(u32::MAX - 2, 100);
    let empty = replay_batches(base, &[]);
    assert_eq!(empty.entities.values().next().unwrap().age, u32::MAX - 2);
    let top = 100 + (1_u64 << 32) + 6;
    let tail = [(101, top, vec![])];
    let replay = replay_batches(empty, &tail);
    let record = replay.entities.values().next().unwrap();
    assert_eq!(
        record.age, 3,
        "elapsed ticks use wrapping u32 lifetime arithmetic"
    );
    assert_eq!(record.state.age, 3);
    assert_eq!(record.tick, top);
    let replay = replay_batches(replay, &tail);
    assert_eq!(replay.entities.values().next().unwrap().age, 3);
}

#[test]
fn lifetime_age_vitals_state_anchors_and_stale_folds_converge() {
    let base = age_base(300, 100);
    let id = *base.entities.keys().next().unwrap();
    let mut state_anchor = base.entities[&id].state.clone();
    state_anchor.age = 777;
    state_anchor.protected_until = 140;
    let tail = [
        (
            101,
            105,
            vec![JournalEntry::EntityVitals {
                id,
                health: 19.0,
                alive: true,
                tick: 105,
            }],
        ),
        (
            106,
            110,
            vec![JournalEntry::EntityState {
                id,
                state: Arc::new(state_anchor.clone()),
                tick: 110,
            }],
        ),
        (
            111,
            115,
            vec![JournalEntry::EntityVitals {
                id,
                health: 18.0,
                alive: true,
                tick: 115,
            }],
        ),
        (116, 120, vec![]),
    ];
    // Vitals alone advances from the prior record tick, before overwriting it.
    let vitals = replay_batches(age_base(300, 100), &tail[..1]);
    assert_eq!(vitals.entities[&id].age, 305);
    let mut replay = replay_batches(base, &tail);
    assert_eq!(replay.entities[&id].age, 787);
    assert_eq!(replay.entities[&id].state.protected_until, 140);
    assert_eq!(replay.entities[&id].health, 18.0);
    state_anchor.age = 1;
    state_anchor.protected_until = 0;
    let stale = [(
        90,
        119,
        vec![
            JournalEntry::EntityState {
                id,
                state: Arc::new(state_anchor),
                tick: 109,
            },
            JournalEntry::EntityVitals {
                id,
                health: 1.0,
                alive: true,
                tick: 108,
            },
            JournalEntry::EntityVitals {
                id,
                health: 0.0,
                alive: false,
                tick: 107,
            },
            JournalEntry::RemoveEntity { id, tick: 106 },
        ],
    )];
    replay = replay_batches(replay, &stale);
    replay = replay_batches(replay, &tail);
    let record = &replay.entities[&id];
    assert_eq!((record.age, record.tick, record.health), (787, 120, 18.0));
    assert_eq!(record.state.age, 787);
    assert_eq!(record.state.protected_until, 140);
}

#[test]
fn lifetime_age_full_images_anchor_and_stale_full_images_cannot_rewind() {
    let base = age_base(300, 100);
    let id = *base.entities.keys().next().unwrap();
    let record = &base.entities[&id];
    let full = |age, tick, health, alive| JournalEntry::Entity {
        id,
        uuid: record.uuid.clone(),
        type_id: record.type_id,
        pos: [9.5, 70.0, 9.5],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        health,
        alive,
        age,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: record.appearance.clone(),
        active_effects: Default::default(),
        villager: None,
        state: Arc::new(EntityStateImage {
            age,
            ..Default::default()
        }),
        projectile: None,
        tick,
    };
    let tail = [(101, 120, vec![full(500, 110, 19.0, true)])];
    let stale = [(
        90,
        119,
        vec![full(1, 99, 1.0, true), full(0, 98, 0.0, false)],
    )];
    let replay = replay_batches(base, &tail);
    let replay = replay_batches(replay, &stale);
    let replay = replay_batches(replay, &tail);
    assert_eq!(replay.entities[&id].age, 510);
    assert_eq!(replay.entities[&id].state.age, 510);
    assert_eq!(replay.entities[&id].health, 19.0);
    assert_eq!(replay.entities[&id].pos, [8.5, 65.0, 8.5]);
}

#[test]
fn lifetime_age_only_changes_match_and_do_not_emit_timer_journal_entries() {
    let mut state = fixture();
    let id = state.spawn_mob(state.registry.cow_type_id, [8.5, 65.0, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).persistent = true;
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    journal_entries_for_tick(&mut state, &output);
    let image = EntityStateImage::from_row(state.entities.body(slot), state.entities.row(slot));
    state.entities.row_mut(slot).age = state.entities.row(slot).age.wrapping_add(42);
    assert!(
        image.matches(state.entities.body(slot), state.entities.row(slot)),
        "age is not a dirty key"
    );
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = journal_entries_for_tick(&mut state, &output);
    assert!(!entries.iter().any(|entry| matches!(
        entry, JournalEntry::EntityState { id: changed, .. }
            | JournalEntry::Entity { id: changed, .. } if *changed == id.0
    )));
    state.entities.row_mut(slot).protected_until = state.tick + 60;
    assert!(
        !image.matches(state.entities.body(slot), state.entities.row(slot)),
        "protection is durable state"
    );
    let entries = journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|entry| matches!(
        entry, JournalEntry::EntityState { id: changed, state: image, .. }
            if *changed == id.0 && image.protected_until == state.tick + 60
    )));
}

#[test]
fn chicken_countdown_uses_batch_clock_and_reset_journals_the_egg_birth() {
    let mut state = fixture();
    let chicken = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:chicken").unwrap())
        .unwrap();
    let id = state.spawn_mob(chicken, [8.5, 65.0, 8.5], 4.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).entity_life.harvest_ticks = 5;
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_spawned
        .push(id);
    let birth = journal_entries_for_tick(&mut state, &output);
    let mut replay = replay_batches(Replay::default(), &[(output.tick, output.tick, birth)]);
    for expected in [3, 2, 1] {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let entries = journal_entries_for_tick(&mut state, &output);
        assert!(
            !entries.iter().any(|entry| matches!(
                entry, JournalEntry::EntityState { id: changed, .. }
                    | JournalEntry::Entity { id: changed, .. } if *changed == id.0
            )),
            "a deterministic egg countdown must not allocate a retained WAL image"
        );
        replay = replay_batches(replay, &[(output.tick, output.tick, entries)]);
        assert_eq!(
            replay.entities[&id.0].state.entity_life.harvest_ticks,
            expected
        );
        assert_eq!(state.entities.row(slot).entity_life.harvest_ticks, expected);
        let unchanged = replay.entities[&id.0].state.clone();
        replay = replay_batches(replay, &[]);
        assert_eq!(replay.entities[&id.0].state, unchanged);
    }
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = journal_entries_for_tick(&mut state, &output);
    let reset = state.entities.row(slot).entity_life.harvest_ticks;
    assert!((6000..=12000).contains(&reset));
    assert!(entries.iter().any(|entry| matches!(
        entry, JournalEntry::EntityState { id: changed, state: image, .. }
            if *changed == id.0 && image.entity_life.harvest_ticks == reset
    )));
    let egg = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:egg").unwrap())
        .unwrap();
    assert!(entries.iter().any(|entry| matches!(
        entry, JournalEntry::Entity { item: Some(stack), .. } if stack.item_id == egg
    )));
    replay = replay_batches(replay, &[(output.tick, output.tick, entries)]);
    let mut restored = fixture();
    restore_entity(&mut restored, id.0, &replay.entities[&id.0]).unwrap();
    let slot = restored.entities.slot_of(id).unwrap();
    assert_eq!(restored.entities.row(slot).entity_life.harvest_ticks, reset);
}

#[test]
fn chicken_clock_survives_vitals_and_multibatch_replay_without_advancing_other_mobs() {
    let state = fixture();
    let chicken = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:chicken".into(),
        extra_fields: vec![("EggLayTime".into(), mc_nbt::Tag::Int(100))],
        ..Default::default()
    };
    let mut record = replay_record(&state, &chicken);
    record.tick = 10;
    let mut cow = record.clone();
    cow.type_id = state.registry.cow_type_id;
    let mut baby = record.clone();
    baby.appearance.baby = true;
    let base = || Replay {
        tick: 10,
        entities: [(1, record.clone()), (2, cow.clone()), (3, baby.clone())].into(),
        sim_debt: active_clock(10),
        ..Default::default()
    };
    let batches = [
        (
            11,
            13,
            vec![JournalEntry::EntityVitals {
                id: 1,
                health: 3.0,
                alive: true,
                tick: 12,
            }],
        ),
        (14, 20, vec![]),
    ];
    let together = replay_batches(base(), &batches);
    let staged = replay_batches(replay_batches(base(), &batches[..1]), &batches[1..]);
    for (id, record) in &together.entities {
        assert_eq!(record.state, staged.entities[id].state);
        assert_eq!(record.tick, staged.entities[id].tick);
        assert_eq!(record.age, staged.entities[id].age);
        assert_eq!(record.health, staged.entities[id].health);
    }
    assert_eq!(together.entities[&1].state.entity_life.harvest_ticks, 90);
    assert_eq!(together.entities[&2].state.entity_life.harvest_ticks, 100);
    assert_eq!(together.entities[&3].state.entity_life.harvest_ticks, 100);
}

#[test]
fn idle_villager_and_mob_clocks_do_not_emit_images_over_sixty_four_ticks() {
    let mut state = fixture();
    let villager_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let villager = state.spawn_mob(villager_type, [8.5, 65.0, 8.5], 20.0);
    let cow = state.spawn_mob(state.registry.cow_type_id, [9.5, 65.0, 8.5], 20.0);
    let slot = state.entities.slot_of(villager).unwrap();
    state.entities.row_mut(slot).controls.no_ai = true;
    state.entities.row_mut(slot).villager = Some(Box::new(crate::villager::VillagerState {
        age: -200,
        ..Default::default()
    }));
    let slot = state.entities.slot_of(cow).unwrap();
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    row.controls.no_ai = true;
    row.breed_cooldown = 200;
    row.entity_life.child_ticks = 200;
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    journal_entries_for_tick(&mut state, &output);
    for _ in 0..64 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let entries = journal_entries_for_tick(&mut state, &output);
        assert!(
            !entries.iter().any(|entry| matches!(
                entry, JournalEntry::Entity { id, .. } | JournalEntry::EntityState { id, .. }
                    if *id == villager.0 || *id == cow.0
            )),
            "idle clocks must not allocate retained images"
        );
    }
    let slot = state.entities.slot_of(villager).unwrap();
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().age,
        -200
    );
    let slot = state.entities.slot_of(cow).unwrap();
    assert_eq!(state.entities.row(slot).breed_cooldown, 135);
    assert_eq!(state.entities.row(slot).entity_life.child_ticks, 135);
}

#[test]
fn dead_curing_villager_freezes_without_timer_only_images() {
    let mut state = fixture();
    let kind = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie_villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(kind, [8.5, 65.0, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    let (body, row) = state.entities.parts_mut(slot);
    body.alive = false;
    row.health = 0.0;
    row.villager = Some(Box::new(crate::villager::VillagerState {
        conversion_time: Some(100),
        ..Default::default()
    }));
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    journal_entries_for_tick(&mut state, &output);
    for _ in 0..8 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let entries = journal_entries_for_tick(&mut state, &output);
        assert!(!entries.iter().any(|entry| matches!(
            entry, JournalEntry::Entity { id: image, .. }
                | JournalEntry::EntityState { id: image, .. } if *image == id.0
        )));
        let slot = state.entities.slot_of_any(id).unwrap();
        assert_eq!(
            state
                .entities
                .row(slot)
                .villager
                .as_ref()
                .unwrap()
                .conversion_time,
            Some(100)
        );
    }
}

#[test]
fn live_villager_clocks_match_replay_across_loaded_frozen_and_resumed_intervals() {
    assert_live_villager_clock_intervals(true);
}

#[test]
fn resident_inactive_villager_clocks_match_replay_without_timer_image_churn() {
    assert_live_villager_clock_intervals(false);
}

fn assert_live_villager_clock_intervals(evict: bool) {
    fn step(
        state: &mut TickState,
        batches: &mut Vec<(u64, u64, Vec<JournalEntry>)>,
        id: EntityId,
        transition: bool,
    ) {
        let (output, _, _) = crate::tick::run_tick(state, &[]);
        let entries = journal_entries_for_tick(state, &output);
        if !transition {
            assert!(
                !entries.iter().any(|entry| matches!(
                    entry, JournalEntry::Entity { id: image, .. }
                        | JournalEntry::EntityState { id: image, .. } if *image == id.0
                )),
                "an admitted idle brain must not emit timer-only images at tick {}: {entries:#?}",
                output.tick
            );
        }
        batches.push((output.tick, output.tick, entries));
    }
    fn check(state: &TickState, batches: &[(u64, u64, Vec<JournalEntry>)], id: EntityId) {
        let replay = replay_batches(Replay::default(), batches);
        let slot = state.entities.slot_of(id).unwrap();
        let live = state.entities.row(slot).villager.as_ref().unwrap();
        let recovered = replay.entities[&id.0].villager.as_ref().unwrap();
        assert_eq!(
            (
                recovered.age,
                recovered.last_work,
                recovered.last_slept,
                recovered.panic_until
            ),
            (live.age, live.last_work, live.last_slept, live.panic_until)
        );
        let mut streamed = Replay::default();
        for batch in batches {
            streamed = crate::journal::apply_batch_images(streamed, std::slice::from_ref(batch));
        }
        crate::journal::advance_replay_clocks(&mut streamed);
        assert_eq!(
            streamed.entities[&id.0].villager,
            replay.entities[&id.0].villager
        );
    }
    let mut state = fixture();
    state.tick = 100;
    state.day_time = 6500;
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state.tickets.insert("clock-test".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    state.world.set_block(8, 64, 8, state.registry.states.stone);
    state.world.fold_overlay();
    let kind = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(kind, [8.5, 65.0, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    row.speed = 0.0;
    row.villager = Some(Box::new(crate::villager::VillagerState {
        age: -1000,
        last_work: 23,
        last_slept: Some(17),
        ..Default::default()
    }));
    let mut batches = Vec::new();
    step(&mut state, &mut batches, id, true);
    let initial_age = state.entities.row(slot).villager.as_ref().unwrap().age;
    for _ in 0..64 {
        step(&mut state, &mut batches, id, false);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().age,
        initial_age + 64
    );
    check(&state, &batches, id);

    state.tickets.clear();
    if evict {
        state.world.capture_dirty(&Default::default());
        let written = state
            .world
            .resident_positions()
            .into_iter()
            .map(|pos| (pos, state.world.chunk_revision(pos.x, pos.z)))
            .collect::<Vec<_>>();
        state.world.mark_flushed(&written);
        state.world.evict_outside(&Default::default());
        assert!(!state.world.has_chunk(0, 0));
    }
    let frozen_age = state.entities.row(slot).villager.as_ref().unwrap().age;
    step(&mut state, &mut batches, id, true);
    assert_eq!(state.world.has_chunk(0, 0), !evict);
    assert!(!state.world.is_active(0, 0));
    for _ in 0..64 {
        step(&mut state, &mut batches, id, false);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().age,
        frozen_age
    );
    check(&state, &batches, id);

    state.tickets.insert("clock-test".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    step(&mut state, &mut batches, id, true);
    for _ in 0..64 {
        step(&mut state, &mut batches, id, false);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().age,
        frozen_age + 65
    );
    check(&state, &batches, id);
}

#[test]
fn villager_clocks_survive_compact_state_vitals_and_staged_replay() {
    let state = fixture();
    let native = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:villager".into(),
        ..Default::default()
    };
    let mut record = replay_record(&state, &native);
    record.tick = 100;
    record.villager = Some(crate::villager::VillagerState {
        age: -200,
        activity: crate::villager::Activity::Work,
        last_work: 99,
        ..Default::default()
    });
    record.state.breed_cooldown = 200;
    let base = || Replay {
        tick: 100,
        entities: [(1, record.clone())].into(),
        sim_debt: active_clock(100),
        ..Default::default()
    };
    let batches = [
        (
            101,
            110,
            vec![JournalEntry::EntityVitals {
                id: 1,
                health: 19.0,
                alive: true,
                tick: 105,
            }],
        ),
        (
            111,
            120,
            vec![JournalEntry::EntityState {
                id: 1,
                state: Arc::new(EntityStateImage {
                    breed_cooldown: 185,
                    protected_until: 200,
                    ..Default::default()
                }),
                tick: 115,
            }],
        ),
        (121, 150, vec![]),
    ];
    let together = replay_batches(base(), &batches);
    let staged = replay_batches(replay_batches(base(), &batches[..1]), &batches[1..]);
    let actual = &together.entities[&1];
    assert_eq!(actual.villager, staged.entities[&1].villager);
    assert_eq!(actual.state, staged.entities[&1].state);
    assert_eq!(actual.villager.as_ref().unwrap().age, -150);
    assert_eq!(actual.villager.as_ref().unwrap().last_work, 149);
    assert_eq!(actual.breed_cooldown, 150);
}

#[test]
fn checkpoint_item_age_5999_expires_on_the_next_live_tick_after_empty_replay() {
    let mut state = fixture();
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let id = crate::tick::spawn_item_entity(
        &mut state,
        [8.5, 65.0, 8.5],
        mc_data::ItemStack::new(item, 1),
    );
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).age = 5999;
    let (_, chunks, _) = checkpoint_data_for(&state, 200);
    let dir = tempfile::tempdir().unwrap();
    mc_world::anvil::write_entity_regions(dir.path(), &state.registry.items, &chunks, 1).unwrap();
    let saved = mc_world::anvil::read_entity_regions(dir.path(), &state.registry.items).unwrap();
    let native = &saved[&(0, 0)].entities[0];
    let mut record = replay_record(&state, native);
    record.tick = 200;
    let replay = replay_batches(
        Replay {
            tick: 200,
            entities: std::collections::BTreeMap::from([(id.0, record)]),
            ..Default::default()
        },
        &[],
    );
    assert_eq!(replay.entities[&id.0].age, 5999);
    let mut restored = fixture();
    restored.tick = replay.tick;
    restore_entity(&mut restored, id.0, &replay.entities[&id.0]).unwrap();
    let (output, _, _) = crate::tick::run_tick(&mut restored, &[]);
    assert!(restored.entities.slot_of(id).is_none());
    assert_eq!(output.tick, 201);
    assert!(output.changes.entities_removed.contains(&id));
}

#[test]
fn lifetime_clock_advances_excluded_families_once_without_changing_payload_timers() {
    let mut state = fixture();
    for name in [
        "minecraft:cow",
        "minecraft:villager",
        "minecraft:falling_block",
        "minecraft:tnt",
    ] {
        let type_id = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap();
        let id = state.spawn_mob(type_id, [8.5, 65.0, 8.5], 20.0);
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).age = 40;
    }
    let before: Vec<_> = state
        .entities
        .slots()
        .map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            (body.id, row.falling, row.tnt)
        })
        .collect();
    crate::natural_spawn::advance_ages(&mut state, &mut crate::boundary::ChangeSet::default());
    for (id, falling, tnt) in before {
        let row = state.entities.row(state.entities.slot_of(id).unwrap());
        assert_eq!(row.age, 41);
        assert_eq!(row.falling, falling);
        assert_eq!(row.tnt, tnt);
    }
}

#[test]
fn age_clock_keeps_players_global_and_inactive_mobs_frozen() {
    let mut state = fixture();
    crate::tick::run_tick(
        &mut state,
        &[crate::boundary::PlayerInput::Join {
            connection: crate::boundary::ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "clock-player".into(),
            permission_level: 0,
        }],
    );
    let player = state
        .connections
        .entity_of(crate::boundary::ConnectionId(1))
        .unwrap();
    let player = state.entities.slot_of(player).unwrap();
    let cow = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let mob = state.spawn_mob(cow, [8.5, 65.0, 8.5], 20.0);
    let mob = state.entities.slot_of(mob).unwrap();
    state.entities.row_mut(player).age = 40;
    state.entities.row_mut(mob).age = 40;
    // Pin the frozen interval independently of joined-player ticket hysteresis.
    state
        .world
        .set_active_chunks(Default::default(), state.tick);
    assert!(!state.world.is_active(0, 0));
    crate::natural_spawn::advance_ages(&mut state, &mut crate::boundary::ChangeSet::default());
    assert_eq!(state.entities.row(player).age, 41);
    assert_eq!(state.entities.row(mob).age, 40);
}

fn active_clock(tick: u64) -> std::collections::BTreeMap<[i32; 2], crate::journal::SimDebtImage> {
    [(
        [0, 0],
        crate::journal::SimDebtImage {
            chunk: [0, 0],
            tick,
            last_sim: tick,
            remaining: 0,
            active: true,
        },
    )]
    .into()
}

#[test]
fn replay_chunk_clock_counts_every_active_interval_before_projecting_entities() {
    let mut base = age_base(100, 10);
    let id = *base.entities.keys().next().unwrap();
    let record = base.entities.get_mut(&id).unwrap();
    record.state.breed_cooldown = 100;
    record.state.love = 100;
    let boundary = |active, last_sim, tick| JournalEntry::SimDebt {
        image: crate::journal::SimDebtImage {
            chunk: [0, 0],
            last_sim,
            remaining: 0,
            active,
            tick,
        },
    };
    let batches = [
        (11, 14, vec![]),
        (15, 24, vec![boundary(false, 14, 24)]),
        (
            25,
            28,
            vec![
                JournalEntry::EntityVitals {
                    id,
                    health: 19.0,
                    alive: true,
                    tick: 28,
                },
                boundary(true, 25, 25),
            ],
        ),
        (29, 40, vec![boundary(false, 30, 40)]),
    ];
    let mut replay = base;
    for batch in &batches {
        replay = crate::journal::apply_batch_images(replay, std::slice::from_ref(batch));
    }
    crate::journal::advance_replay_clocks(&mut replay);
    assert_eq!(replay.entities[&id].age, 110);
    assert_eq!(replay.entities[&id].state.age, 110);
    assert_eq!(replay.entities[&id].state.breed_cooldown, 90);
    assert_eq!(replay.entities[&id].state.love, 90);
    let replay = replay_batches(replay, &batches);
    assert_eq!(replay.entities[&id].age, 110, "repeated fold is idempotent");
}

#[test]
fn replay_entity_without_a_chunk_clock_stays_frozen() {
    let mut base = age_base(100, 10);
    base.sim_debt.clear();
    let replay = replay_batches(base, &[(11, 40, vec![])]);
    assert_eq!(replay.entities.values().next().unwrap().age, 100);
}

#[test]
fn replay_crossing_image_moves_clock_attribution_without_persisting_intra_chunk_motion() {
    let base = age_base(100, 10);
    let id = *base.entities.keys().next().unwrap();
    let record = &base.entities[&id];
    let crossing = JournalEntry::Entity {
        id,
        uuid: record.uuid.clone(),
        type_id: record.type_id,
        pos: [24.5, 65.0, 8.5],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        health: 20.0,
        alive: true,
        age: 104,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: record.appearance.clone(),
        active_effects: Default::default(),
        villager: None,
        state: Arc::new(EntityStateImage {
            age: 104,
            ..Default::default()
        }),
        projectile: None,
        tick: 14,
    };
    let replay = replay_batches(base, &[(11, 20, vec![crossing])]);
    assert_eq!(replay.entities[&id].pos, [24.5, 65.0, 8.5]);
    assert_eq!(
        replay.entities[&id].age, 104,
        "destination without a clock is inactive"
    );
    let replay = replay_batches(
        replay,
        &[(
            21,
            24,
            vec![JournalEntry::SimDebt {
                image: crate::journal::SimDebtImage {
                    chunk: [1, 0],
                    tick: 21,
                    last_sim: 21,
                    remaining: 0,
                    active: true,
                },
            }],
        )],
    );
    assert_eq!(replay.entities[&id].age, 108, "activation tick counts once");
}

#[test]
fn chunk_crossing_emits_existing_full_image_but_intra_chunk_motion_does_not() {
    let mut state = fixture();
    let destination = mc_world::ChunkPos { x: 1, z: 0 };
    state
        .tickets
        .insert("crossing-test".into(), (destination, 0));
    state
        .world
        .ensure_blocking(&[(destination, 0)].into_iter().collect());
    state.refresh_active_chunks(state.tick);
    let id = state.spawn_mob(state.registry.cow_type_id, [8.5, 65.0, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).controls.no_ai = true;
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_spawned
        .push(id);
    let birth = journal_entries_for_tick(&mut state, &output);
    let mut replay = replay_batches(Replay::default(), &[(output.tick, output.tick, birth)]);
    state.entities.set_pos(slot, [9.5, 65.0, 8.5]);
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let within = journal_entries_for_tick(&mut state, &output);
    assert!(
        !within.iter().any(|entry| matches!(entry,
            JournalEntry::Entity { id: image, .. } if *image == id.0
        )),
        "ordinary intra-chunk motion remains checkpoint-only"
    );
    replay = replay_batches(replay, &[(output.tick, output.tick, within)]);
    state.entities.set_pos(slot, [24.5, 65.0, 8.5]);
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let crossing = journal_entries_for_tick(&mut state, &output);
    assert!(
        crossing.iter().any(|entry| matches!(entry,
            JournalEntry::Entity { id: image, pos, .. } if *image == id.0 && pos[0] == 24.5
        )),
        "a chunk crossing must persist existing position/clock attribution"
    );
    replay = replay_batches(replay, &[(output.tick, output.tick, crossing)]);
    assert_eq!(replay.entities[&id.0].pos[0], 24.5);
    assert_eq!(replay.entities[&id.0].age, state.entities.row(slot).age);
}

#[test]
fn first_tick_after_restore_persists_a_chunk_crossing_without_a_warmup_tick() {
    assert_restored_chunk_crossing(false);
}

#[test]
fn remove_and_restore_same_identity_keeps_the_fresh_chunk_anchor() {
    assert_restored_chunk_crossing(true);
}

fn assert_restored_chunk_crossing(remove_first: bool) {
    let mut base = age_base(40, 10);
    let id = *base.entities.keys().next().unwrap();
    base.entities.get_mut(&id).unwrap().state.controls.no_ai = true;
    let mut state = fixture();
    state.tick = base.tick;
    let destination = mc_world::ChunkPos { x: 1, z: 0 };
    state
        .tickets
        .insert("crossing-test".into(), (destination, 0));
    state
        .world
        .ensure_blocking(&[(destination, 0)].into_iter().collect());
    state.refresh_active_chunks(state.tick);
    restore_entity(&mut state, id, &base.entities[&id]).unwrap();
    if remove_first {
        let slot = state.entities.slot_of(EntityId(id)).unwrap();
        state.remove_entity(slot);
        restore_entity(&mut state, id, &base.entities[&id]).unwrap();
    }
    let slot = state.entities.slot_of(EntityId(id)).unwrap();
    state.entities.set_pos(slot, [24.5, 65.0, 8.5]);
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = journal_entries_for_tick(&mut state, &output);
    assert!(
        entries.iter().any(|entry| matches!(entry,
            JournalEntry::Entity { id: image, pos, .. } if *image == id && pos[0] == 24.5
        )),
        "the recovery image must seed the chunk attribution anchor"
    );
    let replay = replay_batches(base, &[(output.tick, output.tick, entries)]);
    assert_eq!(replay.entities[&id].pos[0], 24.5);
}

/// The slot-order serial capture the parallel one must reproduce exactly.
fn checkpoint_entities_serial(
    state: &TickState,
    tick: u64,
) -> std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk> {
    let slots: Vec<crate::entities::EntitySlot> = state.entities.slots().collect();
    let extras = entity_capture_extras(state, &slots);
    let view = EntityCaptureView {
        entities: &state.entities,
        registry: &state.registry,
        tick,
    };
    group_entity_records(slots.iter().enumerate().map(|(index, &slot)| {
        entity_capture_record(&view, slot, capture_extras_for(&extras, index))
    }))
}

#[test]
fn parallel_checkpoint_entity_capture_matches_the_serial_slot_order_capture() {
    let mut state = fixture();
    let id_of = |state: &TickState, name: &str| {
        state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let kinds = [
        state.registry.cow_type_id,
        id_of(&state, "minecraft:skeleton"),
        id_of(&state, "minecraft:sheep"),
        id_of(&state, "minecraft:pillager"),
    ];
    // Well above the 256-row parallel slice, over positive and negative chunks.
    let count = 2_000;
    for index in 0..count {
        let x = f64::from(index % 61 - 30) * 9.7 + 0.5;
        let z = f64::from(index / 61 - 16) * 11.3 - 0.5;
        let id = state.spawn_mob(kinds[index as usize % kinds.len()], [x, 65.0, z], 20.0);
        let slot = state.entities.slot_of(id).unwrap();
        let row = state.entities.row_mut(slot);
        row.age = index as u32;
        row.breed_cooldown = (index % 7) as u32;
    }
    let parallel = checkpoint_entities(&state, 9);
    assert_eq!(parallel, checkpoint_entities_serial(&state, 9));
    assert_eq!(parallel, checkpoint_data_for(&state, 9).1);
    assert!(parallel.len() > 40, "records spread over many chunks");
    assert_eq!(
        parallel
            .values()
            .map(|chunk| chunk.entities.len())
            .sum::<usize>(),
        count as usize
    );
    // State-dependent owner fields were spliced in (raiders carry `Wave`).
    assert!(
        parallel
            .values()
            .flat_map(|chunk| &chunk.entities)
            .filter(|record| record.id == "minecraft:pillager")
            .all(|record| record
                .extra_fields
                .iter()
                .any(|(key, _)| key == "CanJoinRaid"))
    );
}

#[test]
fn fall_distance_growth_is_silent_but_the_landing_reset_is_journaled() {
    let mut state = fixture();
    let id = state.spawn_mob(state.registry.cow_type_id, [8.5, 65.0, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    let grounded = EntityStateImage::from_row(state.entities.body(slot), state.entities.row(slot));
    state.entities.row_mut(slot).controls.fall_distance = 4.5;
    assert!(
        grounded.matches(state.entities.body(slot), state.entities.row(slot)),
        "mid-air growth is transient"
    );
    let airborne = EntityStateImage::from_row(state.entities.body(slot), state.entities.row(slot));
    state.entities.row_mut(slot).controls.fall_distance = 7.0;
    assert!(
        airborne.matches(state.entities.body(slot), state.entities.row(slot)),
        "further growth stays silent"
    );
    state.entities.row_mut(slot).controls.fall_distance = 0.0;
    assert!(
        !airborne.matches(state.entities.body(slot), state.entities.row(slot)),
        "landing resets a journaled non-zero distance"
    );
}
