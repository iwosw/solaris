use super::*;

#[test]
fn root_capture_refreshes_remaining_vibration_delay_without_a_wal_image() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let config = CoreConfig::new(dir.path().to_owned(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let mut world = ChunkWorld::new_native(
        42,
        Arc::clone(&store),
        0,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [3, 201, 3],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    let mut state = TickState::new(Arc::clone(&registry), world, 0, 512);
    state.tickets.insert(
        "sculk-checkpoint-test".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.refresh_active_chunks(state.tick);
    let pos = crate::boundary::BlockPos { x: 3, y: 201, z: 3 };
    let sensor = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:sculk_sensor").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(pos.x, pos.y, pos.z, sensor);
    state.world.fold_overlay();
    crate::sculk::changed(&mut state, pos);
    state.tick = 100;
    crate::vibration::emit(
        &mut state,
        [7.5, 201.5, 3.5],
        crate::vibration::GameEvent::Resonate(7),
        None,
    );
    state.opaque_changes.clear();
    state.tick = 102;
    crate::sculk::run(&mut state, &mut Vec::new());
    assert!(state.opaque_changes.is_empty());
    let (work, received) = crate::core::journal_work_channel();
    let (done, _acks) = std::sync::mpsc::channel();
    assert!(start_checkpoint_with_reply(
        &mut state, &work, &done, 102, None
    ));
    let JournalWork::Checkpoint(job) = received.try_recv().unwrap() else {
        panic!("checkpoint work")
    };
    assert!(state.opaque_changes.is_empty());
    job.regions.write().unwrap();
    drop(state);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let mut world = ChunkWorld::new_native(
        42,
        store,
        0,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [3, 201, 3],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    let mut restored = TickState::new(registry, world, 0, 512);
    restored.tick = 102;
    restored.tickets.insert(
        "sculk-checkpoint-test".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    restored.refresh_active_chunks(restored.tick);
    crate::sculk::hydrate(&mut restored);
    let bytes = &restored
        .world
        .chunk(crate::boundary::ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities[&pos];
    let mc_nbt::Tag::Compound(fields) = mc_nbt::read_network(&mut bytes.as_slice()).unwrap() else {
        panic!("root compound")
    };
    let mc_nbt::Tag::Compound(listener) = &fields
        .iter()
        .find(|(name, _)| name == "listener")
        .unwrap()
        .1
    else {
        panic!("listener")
    };
    assert!(
        listener
            .iter()
            .any(|(name, value)| name == "event_delay" && *value == mc_nbt::Tag::Int(2))
    );
    assert_eq!(
        crate::vibration::pending(&restored, pos).unwrap().arrival,
        104
    );
    restored.tick = 104;
    crate::sculk::run(&mut restored, &mut Vec::new());
    assert_eq!(crate::sculk::comparator(&restored, pos), Some(7));
    assert!(crate::vibration::pending(&restored, pos).is_none());
}

#[test]
fn replayed_replacement_has_no_removal_queued_for_the_first_quiet_live_tick() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let config = CoreConfig::new(dir.path().to_owned(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let mut world = ChunkWorld::new_native(
        42,
        Arc::clone(&store),
        0,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [3, 201, 3],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    let pos = crate::boundary::BlockPos { x: 3, y: 201, z: 3 };
    world.ensure_blocking(&[(chunk, 0)].into_iter().collect());
    let old = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:oak_sign").unwrap())
        .unwrap()
        .default
        .0;
    world.set_block(pos.x, pos.y, pos.z, old);
    world.fold_overlay();
    let mut bytes = Vec::new();
    mc_nbt::write_network(
        &mut bytes,
        &mc_nbt::Tag::Compound(vec![
            ("id".into(), mc_nbt::Tag::String("minecraft:sign".into())),
            ("x".into(), mc_nbt::Tag::Int(pos.x)),
            ("y".into(), mc_nbt::Tag::Int(pos.y)),
            ("z".into(), mc_nbt::Tag::Int(pos.z)),
        ]),
    )
    .unwrap();
    world.set_opaque_block_entity(pos, Some(bytes));
    world.flush_dirty(&Default::default()).unwrap();
    let mut state = TickState::new(Arc::clone(&registry), world, 0, 512);
    state.tick = 100;
    state
        .tickets
        .insert("sculk-replay-fixture".into(), (chunk, 0));
    state.refresh_active_chunks(state.tick);
    let sensor = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:sculk_sensor").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(pos.x, pos.y, pos.z, sensor);
    crate::sculk::changed(&mut state, pos);
    crate::vibration::emit(
        &mut state,
        [7.5, 201.5, 3.5],
        crate::vibration::GameEvent::Resonate(7),
        None,
    );
    let image = state.world.chunk(chunk).unwrap().block_entities[&pos].clone();
    let mut writer = crate::journal::JournalWriter::open(dir.path(), 1).unwrap();
    writer
        .append(
            2,
            100,
            &[
                JournalEntry::RemoveBlockEntity {
                    pos: [pos.x, pos.y, pos.z],
                    tick: 100,
                },
                JournalEntry::Block {
                    pos: [pos.x, pos.y, pos.z],
                    state: sensor,
                },
                JournalEntry::OpaqueBlockEntity {
                    pos: [pos.x, pos.y, pos.z],
                    nbt: Some(image.clone()),
                },
            ],
        )
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    drop(state);
    let mut restored =
        crate::core::recover_dimension_state(&config, crate::dimensions::Dimension::Overworld)
            .unwrap();
    assert_eq!(
        restored.world.chunk(chunk).unwrap().block_entities[&pos],
        image
    );
    crate::sculk::hydrate(&mut restored);
    assert_eq!(
        crate::vibration::pending(&restored, pos).unwrap().arrival,
        104
    );
    assert!(
        restored.world.take_block_entity_removals().is_empty(),
        "recovery must not enqueue a new durable mutation"
    );
    restored.tickets.insert("replay-test".into(), (chunk, 0));
    let (output, _, _) = crate::tick::run_tick(&mut restored, &[]);
    let entries = journal_entries_for_tick(&mut restored, &output);
    assert!(!entries.iter().any(|entry| matches!(entry, JournalEntry::RemoveBlockEntity { pos: cell, .. } if *cell == [pos.x, pos.y, pos.z])));
    assert!(!entries.iter().any(|entry| matches!(entry, JournalEntry::OpaqueBlockEntity { pos: cell, .. } if *cell == [pos.x, pos.y, pos.z])));
    assert_eq!(
        restored.world.chunk(chunk).unwrap().block_entities[&pos],
        image
    );
}
