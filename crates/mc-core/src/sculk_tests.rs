use super::*;
use crate::registry::CoreRegistry;
use crate::vibration::{GameEvent, emit};
use crate::world::ChunkWorld;
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
const SENSOR: BlockPos = BlockPos { x: 3, y: 201, z: 3 };
const CATALYST: BlockPos = BlockPos { x: 8, y: 201, z: 3 };
fn state(dir: Option<std::path::PathBuf>) -> TickState {
    state_in(dir, crate::dimensions::Dimension::Overworld)
}

fn state_in(dir: Option<std::path::PathBuf>, dimension: crate::dimensions::Dimension) -> TickState {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let config = crate::CoreConfig::new(dir.clone().unwrap_or_default(), 42);
    let generator = crate::native_checkpoint::generator(&config, &registry, dimension).unwrap();
    let air = mc_world::BlockStateId(registry.states.air);
    let mut world = if dir.is_some() {
        ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&config).unwrap(),
            crate::native_checkpoint::dimension_id(dimension),
            generator,
            &registry,
            air,
            [3, 201, 3],
            dimension.geometry(),
        )
    } else {
        ChunkWorld::new(
            42,
            generator,
            &registry,
            air,
            [3, 201, 3],
            dimension.geometry(),
        )
    };
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 512);
    state.dimension = dimension;
    state
        .tickets
        .insert("sculk-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    // Strict radius split: fixture intent carries a ticket; activation flows
    // through the same refresh run_tick uses (tickets simulate, Main binding).
    state.refresh_active_chunks(state.tick);
    state
}
fn place(s: &mut TickState, pos: BlockPos, name: &str, props: &[(&str, &str)]) {
    let name = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
    let base = s.registry.blocks.block(&name).unwrap().default;
    let mut values = s.registry.blocks.by_id(base).unwrap().properties.clone();
    for (k, v) in props {
        values.iter_mut().find(|(key, _)| key == k).unwrap().1 = (*v).into();
    }
    let id = s
        .registry
        .blocks
        .by_name_and_props(&name, &values)
        .unwrap()
        .0;
    s.world.set_block(pos.x, pos.y, pos.z, id);
    s.world.fold_overlay();
    changed(s, pos);
}
fn advance(s: &mut TickState, n: u64) -> Vec<(BlockPos, u32)> {
    let mut writes = Vec::new();
    for _ in 0..n {
        s.tick += 1;
        run(s, &mut writes);
        s.world.fold_overlay();
    }
    writes
}
fn prop(s: &TickState, pos: BlockPos, key: &str) -> String {
    property(block(s, pos).unwrap(), key).unwrap().to_owned()
}
#[test]
fn wool_wall_and_wool_floor_block_vibrations_but_stone_transmits() {
    let mut s = state(None);
    place(&mut s, SENSOR, "sculk_sensor", &[]);
    let wall = BlockPos { x: 5, ..SENSOR };
    place(&mut s, wall, "white_wool", &[]);
    emit(&mut s, [7.5, 201.5, 3.5], GameEvent::Resonate(7), None);
    advance(&mut s, 6);
    assert_eq!(prop(&s, SENSOR, "power"), "0");
    place(&mut s, wall, "stone", &[]);
    emit(&mut s, [7.5, 201.5, 3.5], GameEvent::Resonate(7), None);
    advance(&mut s, 4);
    assert_eq!(comparator(&s, SENSOR), Some(7));
    assert_eq!(prop(&s, SENSOR, "power"), "8");
    advance(&mut s, 40);
    let floor = BlockPos { x: 7, y: 200, z: 3 };
    place(&mut s, floor, "white_carpet", &[]);
    emit(&mut s, [7.5, 201.0, 3.5], GameEvent::Step, None);
    advance(&mut s, 8);
    assert_eq!(comparator(&s, SENSOR), Some(0));
}
#[test]
fn frequency_comparator_distance_power_and_cooldown_are_independent() {
    let mut s = state(None);
    place(&mut s, SENSOR, "sculk_sensor", &[]);
    emit(
        &mut s,
        vibration::centre(SENSOR),
        GameEvent::Resonate(15),
        None,
    );
    advance(&mut s, 1);
    assert_eq!(comparator(&s, SENSOR), Some(15));
    assert_eq!(prop(&s, SENSOR, "power"), "15");
    assert_eq!(
        crate::redstone::output(&s, SENSOR, BlockPos { x: 4, ..SENSOR }),
        15
    );
    emit(
        &mut s,
        vibration::centre(SENSOR),
        GameEvent::Resonate(7),
        None,
    );
    advance(&mut s, 30);
    assert_eq!(prop(&s, SENSOR, "sculk_sensor_phase"), "cooldown");
    assert_eq!(comparator(&s, SENSOR), Some(0));
    advance(&mut s, 10);
    assert_eq!(prop(&s, SENSOR, "sculk_sensor_phase"), "inactive");
    emit(&mut s, [5.5, 201.5, 3.5], GameEvent::Resonate(7), None);
    advance(&mut s, 2);
    assert_eq!(comparator(&s, SENSOR), Some(7));
    assert_eq!(prop(&s, SENSOR, "power"), "12");
}
#[test]
fn calibrated_sensor_filters_only_its_back_and_has_sixteen_block_range() {
    let mut s = state(None);
    place(
        &mut s,
        SENSOR,
        "calibrated_sculk_sensor",
        &[("facing", "north")],
    );
    place(
        &mut s,
        BlockPos { z: 4, ..SENSOR },
        "redstone_wire",
        &[("power", "7")],
    );
    emit(&mut s, [14.5, 201.5, 3.5], GameEvent::Resonate(8), None);
    advance(&mut s, 12);
    assert_eq!(comparator(&s, SENSOR), Some(0));
    emit(&mut s, [14.5, 201.5, 3.5], GameEvent::Resonate(7), None);
    advance(&mut s, 11);
    assert_eq!(comparator(&s, SENSOR), Some(7));
    assert_eq!(
        crate::redstone::output(&s, SENSOR, BlockPos { z: 4, ..SENSOR }),
        0
    );
    assert!(crate::redstone::output(&s, SENSOR, BlockPos { x: 4, ..SENSOR }) > 0);
    advance(&mut s, 10);
    assert_eq!(prop(&s, SENSOR, "sculk_sensor_phase"), "cooldown");
}
#[test]
fn amethyst_retransmits_received_frequency_to_a_second_sensor() {
    let mut s = state(None);
    place(&mut s, SENSOR, "sculk_sensor", &[]);
    let second = BlockPos { x: 12, ..SENSOR };
    place(&mut s, second, "sculk_sensor", &[]);
    place(&mut s, BlockPos { x: 4, ..SENSOR }, "amethyst_block", &[]);
    emit(&mut s, [1.5, 201.5, 3.5], GameEvent::Resonate(6), None);
    advance(&mut s, 10);
    assert_eq!(comparator(&s, second), Some(6));
}
#[test]
fn committed_mob_death_near_catalyst_suppresses_orbs_and_spreads_once() {
    let mut s = state(None);
    place(&mut s, CATALYST, "sculk_catalyst", &[]);
    for x in 6..=10 {
        for z in 1..=5 {
            place(&mut s, BlockPos { x, y: 200, z }, "stone", &[]);
        }
    }
    let cow = s
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let entity = s.spawn_mob(cow, [7.5, 201.0, 3.5], 10.0);
    let slot = s.entities.slot_of(entity).unwrap();
    s.entities.row_mut(slot).last_player_hit = Some((crate::boundary::EntityId(999), 0));
    let mut intents = vec![crate::intent::Intent {
        actor: entity,
        seq: 0,
        kind: crate::intent::IntentKind::DamageEntity {
            target: entity,
            amount: 1000.0,
            kind: crate::survival::DamageKind::Magic,
            source: None,
        },
        footprint: crate::intent::Footprint {
            entity_writes: vec![entity],
            ..Default::default()
        },
    }];
    assert_eq!(s.merge(&mut intents), 1);
    let (_, _, spawned, _) = s.commit_merge_effects();
    assert!(spawned.iter().all(|id| !matches!(
        s.entities.body(s.entities.slot_of(*id).unwrap()).kind,
        crate::entities::EntityKind::ExperienceOrb { .. }
    )));
    assert!(
        matches!(&s.sculk.entries[&CATALYST],Entry::Catalyst{cursors,..} if !cursors.is_empty())
    );
    advance(&mut s, 1);
    assert_eq!(prop(&s, CATALYST, "bloom"), "true");
    assert_eq!(
        block(&s, BlockPos { x: 7, y: 200, z: 3 })
            .unwrap()
            .block
            .id
            .path(),
        "sculk"
    );
    assert_eq!(
        block(&s, BlockPos { x: 7, y: 201, z: 3 })
            .unwrap()
            .block
            .id
            .path(),
        "sculk_vein"
    );
    s.commit_merge_effects();
    advance(&mut s, 20);
    assert!(s.entities.experience_orbs().next().is_none());
}
#[test]
fn sensor_pending_and_catalyst_cursors_survive_wal_then_native_checkpoint() {
    let dir = std::env::temp_dir().join(format!("sculk-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    place(&mut original, SENSOR, "sculk_sensor", &[]);
    place(&mut original, CATALYST, "sculk_catalyst", &[]);
    for x in 6..=10 {
        place(&mut original, BlockPos { x, y: 200, z: 3 }, "stone", &[]);
    }
    original.tick = 100;
    emit(
        &mut original,
        [7.5, 201.5, 3.5],
        GameEvent::Resonate(7),
        None,
    );
    assert!(absorb(&mut original, [7.5, 201.0, 3.5], 5));
    let entries: Vec<_> = original
        .opaque_changes
        .iter()
        .map(|(p, nbt)| crate::journal::JournalEntry::OpaqueBlockEntity {
            pos: [p.x, p.y, p.z],
            nbt: nbt.clone(),
        })
        .collect();
    let bytes = serde_json::to_vec(&entries).unwrap();
    let entries = serde_json::from_slice(&bytes).unwrap();
    let replay =
        crate::journal::replay_batches(crate::journal::Replay::default(), &[(101, 101, entries)]);
    let before = original.sculk.entries[&CATALYST].clone();
    let capture = original.world.capture_dirty(&BTreeMap::new());
    assert!(!capture.write().unwrap().is_empty());
    drop(original);
    let mut restored = state(Some(dir.clone()));
    restored.tick = 101;
    for ((x, y, z), nbt) in replay.opaque_block_entities {
        restored
            .world
            .set_opaque_block_entity(BlockPos { x, y, z }, nbt);
    }
    validate(&restored).unwrap();
    hydrate(&mut restored);
    assert_eq!(restored.sculk.entries[&CATALYST], before);
    assert_eq!(vibration::pending(&restored, SENSOR).unwrap().arrival, 104);
    advance(&mut restored, 3);
    assert_eq!(comparator(&restored, SENSOR), Some(7));
    advance(&mut restored, 40);
    assert!(
        matches!(&restored.sculk.entries[&CATALYST],Entry::Catalyst{cursors,..} if cursors.is_empty())
    );
    let capture = restored.world.capture_dirty(&BTreeMap::new());
    capture.write().unwrap();
    drop(restored);
    let mut again = state(Some(dir.clone()));
    again.tick = 144;
    hydrate(&mut again);
    advance(&mut again, 4);
    assert!(
        matches!(&again.sculk.entries[&CATALYST],Entry::Catalyst{cursors,..} if cursors.is_empty())
    );
    assert_eq!(comparator(&again, SENSOR), Some(0));
    drop(again);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn corrupt_cursor_and_frequency_images_fail_closed_with_exact_reason() {
    let mut fields = base_fields(CATALYST, "sculk_catalyst");
    fields.push((
        "cursors".into(),
        list(
            vec![Tag::Compound(vec![
                ("pos".into(), Tag::IntArray(vec![1, 2, 3])),
                ("charge".into(), Tag::Int(1001)),
            ])],
            mc_nbt::tag_type::COMPOUND,
        ),
    ));
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).unwrap();
    assert_eq!(
        validate_record(&bytes).unwrap_err(),
        "charge must be an int in 0..=1000"
    );
    let image = crate::journal::JournalEntry::OpaqueBlockEntity {
        pos: [8, 201, 3],
        nbt: Some(bytes),
    };
    assert!(
        serde_json::from_slice::<crate::journal::JournalEntry>(
            &serde_json::to_vec(&image).unwrap()
        )
        .unwrap_err()
        .to_string()
        .contains("charge must be an int in 0..=1000")
    );
    let mut fields = base_fields(SENSOR, "sculk_sensor");
    fields.push(("last_vibration_frequency".into(), Tag::Int(16)));
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).unwrap();
    assert_eq!(
        validate_record(&bytes).unwrap_err(),
        "last_vibration_frequency must be an int in 0..=15"
    );
}

fn opaque(s: &TickState, pos: BlockPos) -> Option<&Vec<u8>> {
    s.world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|c| c.block_entities.get(&pos))
}

fn journal_writes(
    s: &mut TickState,
    template: &crate::boundary::TickOutput,
    writes: &[(BlockPos, u32)],
) -> Vec<crate::journal::JournalEntry> {
    let mut output = template.clone();
    output.tick = s.tick;
    output.changes = Arc::new(crate::boundary::ChangeSet {
        blocks: writes
            .iter()
            .map(|(position, state)| crate::boundary::BlockChange {
                position: *position,
                state: *state,
            })
            .collect(),
        ..Default::default()
    });
    crate::core::journal_entries_for_tick(s, &output)
}

fn disk_replay(
    dir: &std::path::Path,
    tick: u64,
    entries: &[crate::journal::JournalEntry],
) -> crate::journal::Replay {
    let mut writer = crate::journal::JournalWriter::open(dir, 1).unwrap();
    writer.append(tick, tick, entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = crate::journal::read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let once = crate::journal::replay_batches(Default::default(), &batches);
    crate::journal::replay_batches(once, &batches)
}

fn apply_replay(s: &mut TickState, replay: crate::journal::Replay) {
    for ((x, y, z), id) in replay.blocks {
        s.world.set_block(x, y, z, id);
    }
    s.world.fold_overlay();
    for ((x, y, z), image) in replay.opaque_block_entities {
        s.world.set_opaque_block_entity(BlockPos { x, y, z }, image);
    }
    hydrate(s);
}

#[test]
fn exploded_charged_catalyst_tombstones_old_native_image_before_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = state(None);
    place(&mut s, CATALYST, "sculk_catalyst", &[]);
    assert!(absorb(&mut s, [7.5, 201.0, 3.5], 5));
    checkpoint(&mut s);
    let charged = s.sculk.entries[&CATALYST].clone();
    let config = crate::CoreConfig::new(dir.path().to_owned(), 42);
    crate::native_checkpoint::tests::fixture_store(&config)
        .unwrap()
        .import_chunk(
            crate::native_checkpoint::dimension_id(crate::dimensions::Dimension::Overworld),
            s.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
            &s.registry.blocks,
            &s.registry.items,
        )
        .unwrap();
    drop(s);
    let mut s = state(Some(dir.path().to_owned()));
    s.tickets
        .insert("sculk".into(), (ChunkPos { x: 0, z: 0 }, 0));
    hydrate(&mut s);
    assert_eq!(s.sculk.entries[&CATALYST], charged);
    assert!(opaque(&s, CATALYST).is_some());
    s.opaque_changes.clear();
    crate::tnt::prime_at(&mut s, vibration::centre(CATALYST), 1).unwrap();
    let (output, _, _) = crate::tick::run_tick(&mut s, &[]);
    assert_eq!(output.changes.explosions.len(), 1);
    assert_eq!(
        s.world.block_at(CATALYST.x, CATALYST.y, CATALYST.z),
        Some(s.registry.states.air)
    );
    assert!(opaque(&s, CATALYST).is_none());
    checkpoint(&mut s);
    assert!(
        opaque(&s, CATALYST).is_none(),
        "checkpoint must not resurrect a late removed owner"
    );
    let entries = crate::core::journal_entries_for_tick(&mut s, &output);
    assert!(entries.iter().any(|e| matches!(e, crate::journal::JournalEntry::RemoveBlockEntity { pos, .. } if *pos == [CATALYST.x, CATALYST.y, CATALYST.z])));
    let replay = disk_replay(dir.path(), s.tick, &entries);
    assert_eq!(
        replay
            .opaque_block_entities
            .get(&(CATALYST.x, CATALYST.y, CATALYST.z)),
        Some(&None)
    );
    drop(s);
    let mut restored = state(Some(dir.path().to_owned()));
    assert!(
        opaque(&restored, CATALYST).is_some(),
        "fixture must actually load the charged old native base"
    );
    hydrate(&mut restored);
    assert_eq!(restored.sculk.entries[&CATALYST], charged);
    drop(restored);
    // Startup applies durable replay before hydrating sculk owners.
    let mut restored = state(Some(dir.path().to_owned()));
    apply_replay(&mut restored, replay);
    assert!(opaque(&restored, CATALYST).is_none());
    assert!(!restored.sculk.entries.contains_key(&CATALYST));
    checkpoint(&mut restored);
    restored.world.flush_dirty(&BTreeMap::new()).unwrap();
    drop(restored);
    let mut again = state(Some(dir.path().to_owned()));
    assert!(opaque(&again, CATALYST).is_none());
    place(&mut again, CATALYST, "sculk_catalyst", &[]);
    assert!(
        matches!(&again.sculk.entries[&CATALYST], Entry::Catalyst { cursors, bloom_until: 0 } if cursors.is_empty())
    );
}

#[test]
fn popped_opaque_sign_tombstones_native_base_and_cannot_be_readopted() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = state(None);
    let (template, _, _) = crate::tick::run_tick(&mut s, &[]);
    let support = BlockPos {
        y: SENSOR.y + 1,
        ..SENSOR
    };
    place(&mut s, support, "stone", &[]);
    place(&mut s, SENSOR, "oak_hanging_sign", &[]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(
        &mut bytes,
        &Tag::Compound(base_fields(SENSOR, "hanging_sign")),
    )
    .unwrap();
    s.world.set_opaque_block_entity(SENSOR, Some(bytes));
    let config = crate::CoreConfig::new(dir.path().to_owned(), 42);
    crate::native_checkpoint::tests::fixture_store(&config)
        .unwrap()
        .import_chunk(
            crate::native_checkpoint::dimension_id(crate::dimensions::Dimension::Overworld),
            s.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
            &s.registry.blocks,
            &s.registry.items,
        )
        .unwrap();
    let tick = s.tick;
    drop(s);
    let mut s = state(Some(dir.path().to_owned()));
    s.tick = tick;
    assert!(
        opaque(&s, SENSOR).is_some(),
        "load the imported native sign"
    );
    let air = s.registry.states.air;
    s.world.set_block(support.x, support.y, support.z, air);
    let mut writes = vec![(support, air)];
    crate::tick::pop_unsupported(&mut s, &mut writes, &mut Vec::new());
    assert!(writes.contains(&(SENSOR, air)));
    assert!(
        opaque(&s, SENSOR).is_none(),
        "admission invalidates opaque storage before fold"
    );
    s.world.fold_overlay();
    let entries = journal_writes(&mut s, &template, &writes);
    let replay = disk_replay(dir.path(), s.tick, &entries);
    assert_eq!(
        replay
            .opaque_block_entities
            .get(&(SENSOR.x, SENSOR.y, SENSOR.z)),
        Some(&None)
    );
    drop(s);
    let mut restored = state(Some(dir.path().to_owned()));
    assert!(opaque(&restored, SENSOR).is_some());
    apply_replay(&mut restored, replay);
    assert!(opaque(&restored, SENSOR).is_none());
    restored.world.flush_dirty(&BTreeMap::new()).unwrap();
    drop(restored);
    let mut again = state(Some(dir.path().to_owned()));
    assert!(opaque(&again, SENSOR).is_none());
    place(&mut again, SENSOR, "sculk_sensor", &[]);
    assert_eq!(
        again.sculk.entries[&SENSOR],
        Entry::Sensor {
            frequency: 0,
            deadline: 0
        }
    );
    assert!(vibration::pending(&again, SENSOR).is_none());
}

#[test]
fn coalesced_break_and_replacement_preserve_only_the_final_sensor_image() {
    let mut s = state(None);
    let (template, _, _) = crate::tick::run_tick(&mut s, &[]);
    place(&mut s, SENSOR, "sculk_sensor", &[]);
    emit(&mut s, [7.5, 201.5, 3.5], GameEvent::Resonate(7), None);
    let old_image = opaque(&s, SENSOR).unwrap().clone();
    let sensor = s.world.block_at(SENSOR.x, SENSOR.y, SENSOR.z).unwrap();
    let air = s.registry.states.air;
    s.opaque_changes.clear();
    s.world.set_block(SENSOR.x, SENSOR.y, SENSOR.z, air);
    assert!(opaque(&s, SENSOR).is_none());
    s.world.set_block(SENSOR.x, SENSOR.y, SENSOR.z, sensor);
    changed(&mut s, SENSOR);
    run(&mut s, &mut Vec::new());
    let new_image = opaque(&s, SENSOR).unwrap().clone();
    assert_ne!(old_image, new_image);
    s.world.fold_overlay();
    assert_eq!(opaque(&s, SENSOR), Some(&new_image));
    let entries = journal_writes(&mut s, &template, &[(SENSOR, sensor)]);
    let removal = entries.iter().position(|e| matches!(e, crate::journal::JournalEntry::RemoveBlockEntity { pos, .. } if *pos == [SENSOR.x, SENSOR.y, SENSOR.z])).unwrap();
    let image = entries.iter().position(|e| matches!(e, crate::journal::JournalEntry::OpaqueBlockEntity { pos, nbt: Some(_) } if *pos == [SENSOR.x, SENSOR.y, SENSOR.z])).unwrap();
    assert!(removal < image);
    let replay = crate::journal::replay_batches(Default::default(), &[(s.tick, s.tick, entries)]);
    assert_eq!(
        replay
            .opaque_block_entities
            .get(&(SENSOR.x, SENSOR.y, SENSOR.z)),
        Some(&Some(new_image))
    );
    assert!(vibration::pending(&s, SENSOR).is_none());
}

#[test]
fn unloading_evicts_listener_and_catalyst_indexes_then_rehydrates_pending_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = state(Some(dir.path().to_owned()));
    place(&mut s, SENSOR, "sculk_sensor", &[]);
    place(&mut s, CATALYST, "sculk_catalyst", &[]);
    s.tick = 100;
    emit(&mut s, [7.5, 201.5, 3.5], GameEvent::Resonate(7), None);
    assert!(absorb(&mut s, [7.5, 201.0, 3.5], 5));
    let cursor = s.sculk.entries[&CATALYST].clone();
    checkpoint(&mut s);
    s.world.flush_dirty(&BTreeMap::new()).unwrap();
    assert_eq!(
        s.world.evict_outside(&Default::default()),
        vec![ChunkPos { x: 0, z: 0 }]
    );
    hydrate(&mut s);
    assert!(s.sculk.entries.is_empty());
    assert!(s.sculk.loaded.is_empty());
    assert_eq!(vibration::listener_count(&s), 0);
    s.world
        .ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    s.world
        .set_active_chunks([(ChunkPos { x: 0, z: 0 })].into_iter().collect(), s.tick);
    hydrate(&mut s);
    assert_eq!(vibration::listener_count(&s), 1);
    assert_eq!(s.sculk.entries[&CATALYST], cursor);
    assert_eq!(vibration::pending(&s, SENSOR).unwrap().arrival, 104);
    advance(&mut s, 4);
    assert_eq!(comparator(&s, SENSOR), Some(7));
    advance(&mut s, 40);
    assert_eq!(comparator(&s, SENSOR), Some(0));
    assert!(vibration::pending(&s, SENSOR).is_none());
}

fn remaining_delay(s: &TickState) -> i32 {
    let mut bytes = opaque(s, SENSOR).unwrap().as_slice();
    let Tag::Compound(root) = mc_nbt::read_network(&mut bytes).unwrap() else {
        panic!("sensor compound")
    };
    let Some(Tag::Compound(listener)) = field(&root, "listener") else {
        panic!("listener compound")
    };
    let Some(Tag::Int(delay)) = field(listener, "event_delay") else {
        panic!("event delay int")
    };
    *delay
}

#[test]
fn pending_vibration_writes_start_and_resolution_only_and_checkpoint_refreshes_delay() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = state(Some(dir.path().to_owned()));
    let (template, _, _) = crate::tick::run_tick(&mut s, &[]);
    place(&mut s, SENSOR, "sculk_sensor", &[]);
    s.tick = 100;
    advance(&mut s, 1);
    s.opaque_changes.clear();
    emit(&mut s, [7.5, 201.5, 3.5], GameEvent::Resonate(7), None);
    let start = journal_writes(&mut s, &template, &[]);
    assert_eq!(
        start
            .iter()
            .filter(|e| matches!(e, crate::journal::JournalEntry::OpaqueBlockEntity { .. }))
            .count(),
        1
    );
    let image = opaque(&s, SENSOR).unwrap().clone();
    assert_eq!(remaining_delay(&s), 4);
    for _ in 0..3 {
        advance(&mut s, 1);
        assert_eq!(opaque(&s, SENSOR), Some(&image));
        assert!(
            journal_writes(&mut s, &template, &[])
                .iter()
                .all(|e| !matches!(e, crate::journal::JournalEntry::OpaqueBlockEntity { .. }))
        );
    }
    checkpoint(&mut s);
    assert_eq!(remaining_delay(&s), 1);
    assert!(
        s.opaque_changes.is_empty(),
        "native checkpoint refresh is not a WAL mutation"
    );
    s.world.flush_dirty(&BTreeMap::new()).unwrap();
    drop(s);
    let mut restored = state(Some(dir.path().to_owned()));
    restored.tick = 104;
    hydrate(&mut restored);
    assert_eq!(vibration::pending(&restored, SENSOR).unwrap().arrival, 105);
    advance(&mut restored, 1);
    assert_eq!(comparator(&restored, SENSOR), Some(7));
    let resolved = journal_writes(&mut restored, &template, &[]);
    assert_eq!(
        resolved
            .iter()
            .filter(|e| matches!(e, crate::journal::JournalEntry::OpaqueBlockEntity { .. }))
            .count(),
        1
    );
    assert!(vibration::pending(&restored, SENSOR).is_none());
    advance(&mut restored, 40);
    assert_eq!(comparator(&restored, SENSOR), Some(0));
    assert!(vibration::pending(&restored, SENSOR).is_none());
}

#[test]
fn non_overworld_checkpoint_capture_refreshes_inflight_delay() {
    let dir = tempfile::tempdir().unwrap();
    let mut root = state(None);
    let mut other = state_in(
        Some(dir.path().to_owned()),
        crate::dimensions::Dimension::End,
    );
    place(&mut other, SENSOR, "sculk_sensor", &[]);
    other.tick = 100;
    emit(&mut other, [7.5, 201.5, 3.5], GameEvent::Resonate(7), None);
    advance(&mut other, 2);
    assert_eq!(remaining_delay(&other), 4);
    root.other_dimensions
        .insert(crate::dimensions::Dimension::End, Box::new(other));
    let captures = crate::dimensions::checkpoint::capture(&mut root, 102);
    assert_eq!(
        remaining_delay(&root.other_dimensions[&crate::dimensions::Dimension::End]),
        2
    );
    captures[&crate::dimensions::Dimension::End]
        .regions
        .write()
        .unwrap();
    let mut restored = state_in(
        Some(dir.path().to_owned()),
        crate::dimensions::Dimension::End,
    );
    restored.tick = 102;
    hydrate(&mut restored);
    assert_eq!(remaining_delay(&restored), 2);
    assert_eq!(vibration::pending(&restored, SENSOR).unwrap().arrival, 104);
    advance(&mut restored, 2);
    assert_eq!(comparator(&restored, SENSOR), Some(7));
}

#[test]
fn property_only_block_replay_preserves_the_prior_durable_opaque_sensor_image() {
    let mut s = state(None);
    place(&mut s, SENSOR, "sculk_sensor", &[]);
    emit(&mut s, [7.5, 201.5, 3.5], GameEvent::Resonate(7), None);
    let image = opaque(&s, SENSOR).unwrap().clone();
    place(&mut s, SENSOR, "sculk_sensor", &[("waterlogged", "true")]);
    let block = s.world.block_at(SENSOR.x, SENSOR.y, SENSOR.z).unwrap();
    let initial = vec![crate::journal::JournalEntry::OpaqueBlockEntity {
        pos: [SENSOR.x, SENSOR.y, SENSOR.z],
        nbt: Some(image.clone()),
    }];
    let changed = vec![crate::journal::JournalEntry::Block {
        pos: [SENSOR.x, SENSOR.y, SENSOR.z],
        state: block,
    }];
    let batches = [(1, 1, initial), (2, 2, changed)];
    let replay = crate::journal::replay_batches(Default::default(), &batches);
    assert_eq!(
        replay
            .opaque_block_entities
            .get(&(SENSOR.x, SENSOR.y, SENSOR.z)),
        Some(&Some(image))
    );
    let mut restored = state(None);
    restored.tick = 2;
    apply_replay(&mut restored, replay);
    assert_eq!(prop(&restored, SENSOR, "waterlogged"), "true");
    assert_eq!(vibration::pending(&restored, SENSOR).unwrap().arrival, 4);
}

#[test]
fn coalesced_catalyst_recreation_cannot_keep_old_transient_charge() {
    let mut s = state(None);
    place(&mut s, CATALYST, "sculk_catalyst", &[]);
    assert!(absorb(&mut s, [7.5, 201.0, 3.5], 5));
    let catalyst = s
        .world
        .block_at(CATALYST.x, CATALYST.y, CATALYST.z)
        .unwrap();
    let air = s.registry.states.air;
    s.world.set_block(CATALYST.x, CATALYST.y, CATALYST.z, air);
    s.world
        .set_block(CATALYST.x, CATALYST.y, CATALYST.z, catalyst);
    changed(&mut s, CATALYST);
    assert!(
        matches!(&s.sculk.entries[&CATALYST], Entry::Catalyst { cursors, bloom_until: 0 } if cursors.is_empty())
    );
    run(&mut s, &mut Vec::new());
    s.world.fold_overlay();
    let (entry, pending) = decode(opaque(&s, CATALYST).unwrap(), s.tick).unwrap();
    assert!(matches!(entry, Entry::Catalyst { cursors, bloom_until: 0 } if cursors.is_empty()));
    assert!(pending.is_none());
}
