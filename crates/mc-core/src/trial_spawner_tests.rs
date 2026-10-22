use super::*;
use crate::boundary::{ChunkPos, ConnectionId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use std::sync::{Arc, LazyLock};
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };
fn state(dir: Option<std::path::PathBuf>) -> TickState {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let mut world = if let Some(dir) = dir {
        ChunkWorld::new_native(
            42,
            crate::world::world_tests::native_store(&dir, &registry),
            0,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 10],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 10],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .tickets
        .insert("trial-spawner-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    state
}
fn join(state: &mut TickState, connection: u64) -> EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(connection),
            uuid: uuid::Uuid::new_v4(),
            name: format!("trial{connection}"),
            permission_level: 0,
        }],
    );
    let id = state
        .connections
        .entity_of(ConnectionId(connection))
        .unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, [8.5, 201.0, 10.5]);
    id
}
fn place(state: &mut TickState) {
    let id = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:trial_spawner").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(POS.x, POS.y, POS.z, id);
    state.world.fold_overlay();
    after_blocks(state, &[(POS, id)]);
}
fn step(state: &mut TickState) -> ChangeSet {
    let mut changes = ChangeSet::default();
    run(state, &mut Vec::new(), &mut changes);
    state.world.fold_overlay();
    state.tick += 1;
    changes
}
fn kill_wave(state: &mut TickState) {
    let mobs: Vec<_> = state.trial_spawners[&key(POS)]
        .data
        .mobs
        .iter()
        .copied()
        .collect();
    for uuid in mobs {
        let slot = state.entities.slot_of_uuid(uuid).unwrap();
        state.remove_entity(slot);
    }
}
#[test]
fn trial_spawner_caps_wave_tracks_players_and_ejects_keys_then_cools_down() {
    let mut state = state(None);
    let first = join(&mut state, 1);
    place(&mut state);
    for _ in 0..160 {
        step(&mut state);
    }
    assert_eq!(state.trial_spawners[&key(POS)].data.mobs.len(), 2);
    assert_eq!(state.trial_spawners[&key(POS)].data.total, 2);
    let _second = join(&mut state, 2);
    for _ in 0..80 {
        step(&mut state);
    }
    assert_eq!(state.trial_spawners[&key(POS)].data.players.len(), 2);
    assert_eq!(state.trial_spawners[&key(POS)].data.mobs.len(), 3);
    assert!(
        state.trial_spawners[&key(POS)].data.players.contains(
            &state
                .entities
                .row(state.entities.slot_of(first).unwrap())
                .uuid
        )
    );
    let mut drops = Vec::new();
    for _ in 0..8 {
        kill_wave(&mut state);
        for _ in 0..160 {
            drops.extend(step(&mut state).entities_spawned);
        }
        if property(&state, POS, "trial_spawner_state") == Some("cooldown") {
            break;
        }
    }
    assert_eq!(state.trial_spawners[&key(POS)].data.total, 8);
    assert_eq!(
        property(&state, POS, "trial_spawner_state"),
        Some("cooldown")
    );
    let keys = drops
        .iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .filter(|item| state.registry.items.name_of(item.item_id).unwrap().path() == "trial_key")
        .count();
    assert_eq!(keys, 2);
    let cooldown = state.trial_spawners[&key(POS)].data.cooldown;
    assert!(cooldown > state.tick);
    for _ in 0..100 {
        assert!(step(&mut state).entities_spawned.is_empty());
    }
}
#[test]
fn trial_spawner_ominous_conversion_item_marker_and_consumption() {
    let mut state = state(None);
    let entity = join(&mut state, 1);
    state
        .players
        .get_mut(&entity)
        .unwrap()
        .give_effect(crate::effects::ActiveEffect::new(
            crate::effects::EffectId::BadOmen,
            120_000,
            2,
        ));
    place(&mut state);
    step(&mut state);
    let player = &state.players[&entity];
    assert!(!player.active_effects.has(crate::effects::EffectId::BadOmen));
    assert_eq!(
        player
            .active_effects
            .get(crate::effects::EffectId::TrialOmen)
            .unwrap()
            .duration,
        54_000
    );
    assert_eq!(property(&state, POS, "ominous"), Some("true"));
    state.tick = 160;
    let changes = step(&mut state);
    assert!(state.trial_spawners[&key(POS)].pending.is_some());
    let marker_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:ominous_item_spawner").unwrap())
        .unwrap();
    assert!(changes.entities_spawned.iter().any(|id| {
        let crate::entities::EntityKind::Mob { type_id } = state
            .entities
            .body(state.entities.slot_of(*id).unwrap())
            .kind
        else {
            return false;
        };
        type_id == marker_type
    }));
    for _ in 0..8 {
        kill_wave(&mut state);
        for _ in 0..160 {
            step(&mut state);
        }
        if property(&state, POS, "trial_spawner_state") == Some("cooldown") {
            break;
        }
    }
    assert!(
        !state.players[&entity]
            .active_effects
            .has(crate::effects::EffectId::TrialOmen)
    );
    assert!(
        state
            .entities
            .slots()
            .any(|slot| state
                .entities
                .row(slot)
                .item
                .as_ref()
                .is_some_and(
                    |item| state.registry.items.name_of(item.item_id).unwrap().path()
                        == "ominous_trial_key"
                ))
    );
}
#[test]
fn trial_spawner_cooldown_and_uuid_sets_survive_native_checkpoint() {
    let dir = std::env::temp_dir().join(format!("trial-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    place(&mut original);
    let spawner = original.trial_spawners.get_mut(&key(POS)).unwrap();
    spawner.data.cooldown = 36_500;
    spawner.data.total = 6;
    spawner.data.players.insert(uuid::Uuid::new_v4());
    let saved = spawner.clone();
    saved.persist(&mut original, POS);
    set_properties(
        &mut original,
        POS,
        &[("trial_spawner_state", "cooldown")],
        &mut Vec::new(),
    );
    original.world.fold_overlay();
    crate::world::world_tests::write_capture(&original.world.capture_dirty(&BTreeMap::new()))
        .unwrap();
    drop(original);
    let mut restored = state(Some(dir.clone()));
    let chunk = restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    hydrate_chunk(&mut restored.trial_spawners, &restored.registry, chunk).unwrap();
    assert_eq!(
        restored.trial_spawners[&key(POS)].data.cooldown,
        saved.data.cooldown
    );
    assert_eq!(
        restored.trial_spawners[&key(POS)].data.players,
        saved.data.players
    );
    restored.tick = 36_499;
    assert!(step(&mut restored).entities_spawned.is_empty());
    assert_eq!(
        property(&restored, POS, "trial_spawner_state"),
        Some("cooldown")
    );
    assert!(step(&mut restored).entities_spawned.is_empty());
    assert_eq!(
        property(&restored, POS, "trial_spawner_state"),
        Some("waiting_for_players")
    );
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn trial_spawner_corrupt_uuid_and_timer_fields_fail_closed() {
    let fields = vec![("cooldown_ends_at".into(), Tag::Int(5))];
    assert_eq!(
        SpawnerData::from_fields(fields).unwrap_err(),
        "cooldown_ends_at: expected nonnegative Long"
    );
    let fields = vec![(
        "registered_players".into(),
        Tag::List(ListTag {
            element_type: tag_type::INT_ARRAY,
            elements: vec![Tag::IntArray(vec![1, 2, 3])],
        }),
    )];
    assert_eq!(
        SpawnerData::from_fields(fields).unwrap_err(),
        "registered_players[0]: expected IntArray[4]"
    );
}

#[test]
fn trial_spawner_wal_postimage_is_idempotent_and_corruption_is_rejected() {
    use crate::journal::{
        JournalEntry, JournalWriter, Replay, read_complete_batches_after, replay_batches,
    };
    let dir = std::env::temp_dir().join(format!("trial-wal-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    place(&mut original);
    let spawner = original.trial_spawners.get_mut(&key(POS)).unwrap();
    spawner.data.cooldown = 36_500;
    spawner.data.total = 6;
    spawner.data.players.insert(uuid::Uuid::new_v4());
    let saved = spawner.clone();
    saved.persist(&mut original, POS);
    set_properties(
        &mut original,
        POS,
        &[("trial_spawner_state", "cooldown")],
        &mut Vec::new(),
    );
    original.world.fold_overlay();
    crate::world::world_tests::write_capture(&original.world.capture_dirty(&BTreeMap::new()))
        .unwrap();
    let spawner = original.trial_spawners.get_mut(&key(POS)).unwrap();
    spawner.data.cooldown = 72_000;
    let newest = spawner.clone();
    newest.persist(&mut original, POS);
    let (output, _, _) = run_tick(&mut original, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut original, &output);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer.gate(0, output.tick).unwrap();
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    let expected = replay.opaque_block_entities.clone();
    let twice = replay_batches(replay, &batches);
    assert_eq!(twice.opaque_block_entities, expected);
    drop(original);
    let mut restored = state(Some(dir.clone()));
    for (cell, bytes) in twice.opaque_block_entities {
        restored
            .world
            .set_opaque_block_entity(position(cell), bytes);
    }
    let chunk = restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    hydrate_chunk(&mut restored.trial_spawners, &restored.registry, chunk).unwrap();
    assert_eq!(restored.trial_spawners[&key(POS)].data.cooldown, 72_000);
    assert!(step(&mut restored).entities_spawned.is_empty());
    let corrupt = JournalEntry::OpaqueBlockEntity {
        pos: [POS.x, POS.y, POS.z],
        nbt: Some(nbt::encode(
            vec![("cooldown_ends_at".into(), Tag::Int(4))],
            "minecraft:trial_spawner",
            [POS.x, POS.y, POS.z],
        )),
    };
    let error = serde_json::from_slice::<JournalEntry>(&serde_json::to_vec(&corrupt).unwrap())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("trial_spawner: cooldown_ends_at: expected nonnegative Long"),
        "{error}"
    );
    drop(writer);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn trial_spawner_named_presets_and_direct_config_control_caps() {
    let mut data = SpawnerData::default();
    data.players.insert(uuid::Uuid::new_v4());
    nbt::set(
        &mut data.fields,
        "normal_config",
        Tag::String("minecraft:trial_chamber/breeze/normal".into()),
    );
    nbt::set(
        &mut data.fields,
        "ominous_config",
        Tag::String("minecraft:trial_chamber/breeze/ominous".into()),
    );
    assert_eq!(limits(&data, false), (2, 1, 20, 4));
    assert_eq!(limits(&data, true), (4, 2, 20, 4));
    data.players.insert(uuid::Uuid::new_v4());
    assert_eq!(limits(&data, false), (3, 1, 20, 4));
    nbt::set(
        &mut data.fields,
        "normal_config",
        Tag::Compound(vec![
            ("total_mobs".into(), Tag::Float(1.0)),
            ("simultaneous_mobs".into(), Tag::Float(1.0)),
            ("total_mobs_added_per_player".into(), Tag::Float(0.0)),
            ("simultaneous_mobs_added_per_player".into(), Tag::Float(0.0)),
            ("ticks_between_spawn".into(), Tag::Int(5)),
            ("spawn_range".into(), Tag::Int(2)),
        ]),
    );
    assert_eq!(limits(&data, false), (1, 1, 5, 2));
}

#[test]
fn trial_spawner_ominous_marker_roundtrip_retains_payload_and_ejects_once() {
    let mut state = state(None);
    join(&mut state, 1);
    place(&mut state);
    let mut spawner = state.trial_spawners.remove(&key(POS)).unwrap();
    let mut changes = ChangeSet::default();
    spawn_ominous_item(&mut state, &mut spawner, POS, 42, &mut changes);
    let pending = spawner.pending.clone().unwrap();
    let bytes = nbt::encode(
        spawner.fields(&state.registry.items),
        "minecraft:trial_spawner",
        [POS.x, POS.y, POS.z],
    );
    let mut restored = Spawner::decode(&bytes, &state.registry.items).unwrap();
    assert_eq!(restored.pending, spawner.pending);
    state.trial_spawners.insert(key(POS), spawner);
    let fields = entity_fields(&state, pending.uuid);
    assert!(matches!(field(&fields, "item"), Some(Tag::Compound(_))));
    assert_eq!(
        field(&fields, "spawn_item_after_ticks"),
        Some(&Tag::Long(
            pending.deadline.saturating_sub(state.tick) as i64
        ))
    );
    state.tick = pending.deadline;
    let before = state.entities.len();
    pending_tick(&mut state, &mut restored, &mut changes);
    assert!(state.entities.slot_of_uuid(pending.uuid).is_none());
    assert!(restored.pending.is_none());
    assert_eq!(
        state.entities.len(),
        before,
        "marker is replaced by exactly one payload"
    );
    let after = state.entities.len();
    pending_tick(&mut state, &mut restored, &mut changes);
    assert_eq!(
        state.entities.len(),
        after,
        "repeated completion cannot duplicate payload"
    );
}

#[test]
fn trial_spawner_corrupt_native_chunk_is_unavailable_and_never_overwritten() {
    for fields in [
        vec![(
            "normal_config".into(),
            Tag::String("minecraft:unknown_trial".into()),
        )],
        vec![("required_player_range".into(), Tag::Int(129))],
    ] {
        let directory = tempfile::tempdir().unwrap();
        let registry = CoreRegistry::load(None).unwrap();
        let pos = ChunkPos { x: 0, z: 0 };
        let mut chunk = mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(registry.states.air),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        let block = registry
            .blocks
            .block(&Identifier::parse("minecraft:trial_spawner").unwrap())
            .unwrap()
            .default;
        chunk.set_block(8, POS.y, 8, block);
        chunk.block_entities.insert(
            POS,
            nbt::encode(fields, "minecraft:trial_spawner", [POS.x, POS.y, POS.z]),
        );
        let store = crate::world::world_tests::native_store(directory.path(), &registry);
        let before = store.read_checkpoint().unwrap();
        let mut globals = before.globals.clone();
        for record in globals.values_mut() {
            record.captured_tick = 1;
        }
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&globals["checkpoint"].payload).unwrap();
        manifest["tick"] = 1.into();
        for child in manifest["dimensions"].as_object_mut().unwrap().values_mut() {
            child["tick"] = 1.into();
        }
        globals.get_mut("checkpoint").unwrap().payload = serde_json::to_vec(&manifest).unwrap();
        let base = mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(registry.states.air),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        let snapshots = [mc_world::native::NativeChunkSnapshot {
            dimension: 0,
            base: &base,
            effective: &chunk,
            dirty_sections: None,
        }];
        store
            .commit_checkpoint(
                mc_world::native::NativeCheckpoint {
                    tick: 1,
                    previous_tick: 0,
                    chunks: &snapshots,
                    entities: &[],
                    players: &[],
                    globals: &globals,
                },
                &registry.blocks,
                &registry.items,
            )
            .unwrap();
        let saved = store.read_checkpoint().unwrap();
        let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(
            mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks)),
        );
        let stored_error = store
            .read_chunk(
                0,
                pos,
                &registry.blocks,
                &registry.items,
                &registry.chest_loot,
                &*generator,
            )
            .unwrap_err()
            .to_string();
        let mut world = ChunkWorld::new_native(
            42,
            Arc::clone(&store),
            0,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [POS.x, POS.y, POS.z],
            mc_world::OVERWORLD_GEOMETRY,
        );
        assert!(
            world
                .ensure_blocking(&[(pos, 0)].into_iter().collect())
                .is_empty()
        );
        assert!(!world.has_chunk(0, 0));
        assert!(
            world
                .load_error(pos)
                .unwrap()
                .reason
                .contains("trial_spawner:")
        );
        world.mark_block_entity_dirty(POS);
        assert!(
            crate::world::world_tests::write_capture(&world.capture_dirty(&BTreeMap::new()),)
                .is_err()
        );
        assert!(world.take_generated(0).is_none());
        let after = store.read_checkpoint().unwrap();
        assert_eq!(after.tick, saved.tick);
        assert_eq!(after.globals, saved.globals);
        let generator = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
        let durable_error = store
            .read_chunk(
                0,
                pos,
                &registry.blocks,
                &registry.items,
                &registry.chest_loot,
                &generator,
            )
            .unwrap_err()
            .to_string();
        assert_eq!(durable_error, stored_error);
    }
}

#[test]
fn trial_spawner_truncated_resident_record_returns_error_without_tick_panic() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = state(Some(directory.path().to_owned()));
    place(&mut state);
    let capture = state.world.capture_dirty(&BTreeMap::new());
    crate::world::world_tests::write_capture(&capture).unwrap();
    let store = capture.store().unwrap();
    let original = store.read_checkpoint().unwrap();
    let generator = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&state.registry.blocks));
    let saved = store
        .read_chunk(
            0,
            ChunkPos { x: 0, z: 0 },
            &state.registry.blocks,
            &state.registry.items,
            &state.registry.chest_loot,
            &generator,
        )
        .unwrap()
        .unwrap();
    state.trial_spawners.remove(&key(POS));
    state
        .world
        .set_opaque_block_entity(POS, Some(vec![mc_nbt::tag_type::COMPOUND]));
    let chunk = state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    assert!(hydrate_chunk(&mut state.trial_spawners, &state.registry, chunk).is_err());
    run_tick(&mut state, &[]);
    assert!(!state.world.has_chunk(0, 0));
    assert!(
        state
            .world
            .load_error(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .reason
            .contains("trial_spawner at")
    );
    assert!(
        crate::world::world_tests::write_capture(&state.world.capture_dirty(&BTreeMap::new()),)
            .is_err()
    );
    let after = store.read_checkpoint().unwrap();
    assert_eq!(after.tick, original.tick);
    assert_eq!(after.globals, original.globals);
    let durable = store
        .read_chunk(
            0,
            ChunkPos { x: 0, z: 0 },
            &state.registry.blocks,
            &state.registry.items,
            &state.registry.chest_loot,
            &generator,
        )
        .unwrap()
        .unwrap();
    assert_eq!(durable.block_entities, saved.block_entities);
}
