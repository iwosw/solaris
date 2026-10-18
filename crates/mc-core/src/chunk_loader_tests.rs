use super::*;
use crate::registry::CoreRegistry;
use crate::world::ChunkWorld;
use mc_nbt::Tag;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingEmpty {
    air: u32,
    geometry: mc_world::ChunkGeometry,
    generated: Arc<AtomicUsize>,
}

impl ChunkGenerator for CountingEmpty {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        self.generated.fetch_add(1, Ordering::Relaxed);
        Chunk::empty_with_geometry(
            pos,
            mc_world::BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
            self.geometry,
        )
    }
}

fn source(
    store: Arc<NativeStore>,
    dimension: u32,
    geometry: mc_world::ChunkGeometry,
    registry: &CoreRegistry,
    generated: &Arc<AtomicUsize>,
) -> Arc<ChunkSource> {
    Arc::new(ChunkSource {
        native: Some(store),
        dimension,
        generator: Arc::new(CountingEmpty {
            air: registry.states.air,
            geometry,
            generated: Arc::clone(generated),
        }),
        registry: Arc::clone(&registry.blocks),
        items: Arc::clone(&registry.items),
        chest_loot: Arc::clone(&registry.chest_loot),
        light: Arc::clone(&registry.light),
        geometry,
    })
}

#[test]
fn corrupt_native_block_entity_never_generates_or_overwrites_at_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let registry = CoreRegistry::load(Some(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
    ))
    .unwrap();
    let store = crate::world::world_tests::native_store(directory.path(), &registry);
    let pos = ChunkPos { x: 3, z: -2 };
    let block = mc_world::BlockPos {
        x: 49,
        y: 100,
        z: -31,
    };
    let mut chunk = Chunk::empty(
        pos,
        mc_world::BlockStateId(registry.states.air),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    let mut corrupt_nbt = Vec::new();
    mc_nbt::write_network(
        &mut corrupt_nbt,
        &Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:conduit".into())),
            ("x".into(), Tag::Int(block.x)),
            ("y".into(), Tag::Int(block.y)),
            ("z".into(), Tag::Int(block.z)),
            ("Target".into(), Tag::IntArray(vec![1, 2, 3])),
        ]),
    )
    .unwrap();
    chunk.block_entities.insert(block, corrupt_nbt);
    store
        .import_chunk(0, &chunk, &registry.blocks, &registry.items)
        .unwrap();
    let original_counts = store.record_counts().unwrap();
    let generated = Arc::new(AtomicUsize::new(0));
    let source = source(
        Arc::clone(&store),
        0,
        mc_world::OVERWORLD_GEOMETRY,
        &registry,
        &generated,
    );
    let error = source.load_or_generate(pos).unwrap_err();
    assert_eq!(error.dimension, 0);
    assert_eq!(error.pos, pos);
    assert!(
        error
            .reason
            .contains("conduit Target: expected TAG_Int_Array[4]"),
        "{error}"
    );
    assert_eq!(generated.load(Ordering::Relaxed), 0);

    // Unseen corruption is discovered on journal preparation, not capture.
    // Failed preparation must keep the absent interval pending for retry.
    let mut deferred_world = ChunkWorld::new_native(
        42,
        Arc::clone(&store),
        0,
        Arc::clone(&source.generator),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [block.x, block.y, block.z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut replay = crate::journal::Replay {
        tick: 20,
        ..Default::default()
    };
    replay.inhabited_time.insert(
        [pos.x, pos.z],
        crate::journal::InhabitedTimeImage {
            chunk: [pos.x, pos.z],
            value: 400,
            tick: 20,
            active: false,
        },
    );
    deferred_world.restore_inhabited_time(&replay, None);
    for _ in 0..2 {
        let capture = deferred_world.capture_dirty(&Default::default());
        assert!(capture.load_error().is_none());
        assert_eq!(capture.written().len(), 1, "failed capture remains pending");
        assert!(
            capture
                .visit_native(|_| Ok(()))
                .err()
                .unwrap()
                .to_string()
                .contains(&error.to_string())
        );
        assert!(!deferred_world.has_chunk(pos.x, pos.z));
        assert_eq!(store.record_counts().unwrap(), original_counts);
        assert_eq!(store.read_checkpoint().unwrap().tick, 0);
    }
    drop(deferred_world);

    let mut world = ChunkWorld::new_native(
        42,
        Arc::clone(&store),
        0,
        Arc::clone(&source.generator),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [block.x, block.y, block.z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let want = [(pos, 0)].into_iter().collect();
    assert!(world.ensure_blocking(&want).is_empty());
    assert!(!world.has_chunk(pos.x, pos.z));
    assert_eq!(
        world.load_error(pos).unwrap().to_string(),
        error.to_string()
    );
    assert!(world.ensure_blocking(&want).is_empty());
    assert_eq!(world.load_errors().count(), 1);
    world.set_block(block.x, block.y, block.z, registry.states.dirt);
    assert_eq!(world.block_at(block.x, block.y, block.z), None);
    world.set_opaque_block_entity(block, None);
    assert_eq!(world.flush_dirty(&Default::default()).unwrap(), 0);
    assert!(world.take_generated(0).is_none());

    let mut scheduled = world.capture_dirty(&Default::default());
    world.capture_scheduled_block_ticks(
        &mut scheduled,
        vec![(
            pos,
            vec![mc_world::ScheduledBlockTick::new(
                block,
                mc_data::Identifier::parse("minecraft:stone").unwrap(),
                1,
                0,
            )],
        )],
    );
    assert_eq!(
        scheduled.load_error().unwrap().to_string(),
        error.to_string()
    );
    assert!(scheduled.visit_native(|_| Ok(())).is_err());
    assert!(scheduled.write().is_err());
    let mut scheduled_fluid = world.capture_dirty(&Default::default());
    world.capture_scheduled_fluid_ticks(
        &mut scheduled_fluid,
        vec![(
            pos,
            vec![mc_world::ScheduledFluidTick::new(
                block,
                mc_data::Identifier::parse("minecraft:water").unwrap(),
                1,
                0,
            )],
        )],
    );
    assert_eq!(
        scheduled_fluid.load_error().unwrap().to_string(),
        error.to_string()
    );
    assert!(scheduled_fluid.visit_native(|_| Ok(())).is_err());
    assert!(scheduled_fluid.write().is_err());

    // Failed captures retain both absent custody and the pending block overlay.
    world.mark_block_entity_dirty(block);
    for _ in 0..2 {
        let checkpoint = world.flush_dirty(&Default::default()).unwrap_err();
        assert!(
            checkpoint.to_string().contains(&error.to_string()),
            "{checkpoint}"
        );
        assert_eq!(store.record_counts().unwrap(), original_counts);
        assert_eq!(store.read_checkpoint().unwrap().tick, 0);
    }
    drop(world);
    let mut world = ChunkWorld::new_native(
        42,
        Arc::clone(&store),
        0,
        Arc::clone(&source.generator),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [block.x, block.y, block.z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.set_block(block.x, block.y, block.z, registry.states.dirt);
    assert!(world.ensure_blocking(&want).is_empty());
    for _ in 0..2 {
        assert!(world.flush_dirty(&Default::default()).is_err());
        assert!(!world.overlay_is_empty());
        assert_eq!(store.record_counts().unwrap(), original_counts);
    }
    assert_eq!(generated.load(Ordering::Relaxed), 0);
    assert!(!directory.path().join("region").exists());
}

#[test]
fn native_imported_air_wins_and_only_absence_generates_in_each_dimension() {
    let directory = tempfile::tempdir().unwrap();
    let registry = CoreRegistry::load(Some(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
    ))
    .unwrap();
    let store = crate::world::world_tests::native_store(directory.path(), &registry);
    let imported_pos = ChunkPos { x: -3, z: 2 };
    let absent_pos = ChunkPos { x: -4, z: 2 };
    for dimension in 0..3 {
        let geometry = if dimension == 0 {
            mc_world::OVERWORLD_GEOMETRY
        } else {
            mc_world::ChunkGeometry::new(0, 256).unwrap()
        };
        let generated = Arc::new(AtomicUsize::new(0));
        let source = source(
            Arc::clone(&store),
            dimension,
            geometry,
            &registry,
            &generated,
        );
        let chunk = Chunk::empty_with_geometry(
            imported_pos,
            mc_world::BlockStateId(registry.states.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
            geometry,
        );
        store
            .import_chunk(dimension, &chunk, &registry.blocks, &registry.items)
            .unwrap();
        let (stored, was_generated) = source.load_or_generate(imported_pos).unwrap();
        assert!(!was_generated);
        assert_eq!(stored.geometry(), geometry);
        assert_eq!(generated.load(Ordering::Relaxed), 0);
        let (fresh, was_generated) = source.load_or_generate(absent_pos).unwrap();
        assert!(was_generated);
        assert_eq!(fresh.geometry(), geometry);
        assert_eq!(generated.load(Ordering::Relaxed), 1);
    }
    let generated = Arc::new(AtomicUsize::new(0));
    let unknown = source(
        store,
        99,
        mc_world::OVERWORLD_GEOMETRY,
        &registry,
        &generated,
    );
    assert!(unknown.load_or_generate(imported_pos).is_err());
    assert_eq!(
        generated.load(Ordering::Relaxed),
        0,
        "unknown native dimension is not absence"
    );
}

#[test]
fn memory_constructor_has_no_persistence_source_or_generated_saves() {
    let registry = CoreRegistry::load(Some(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
    ))
    .unwrap();
    let generated = Arc::new(AtomicUsize::new(0));
    let mut world = ChunkWorld::new(
        42,
        Arc::new(CountingEmpty {
            air: registry.states.air,
            geometry: mc_world::OVERWORLD_GEOMETRY,
            generated: Arc::clone(&generated),
        }),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let pos = ChunkPos { x: 0, z: 0 };
    assert_eq!(
        world.ensure_blocking(&[(pos, 0)].into_iter().collect()),
        vec![pos]
    );
    assert_eq!(generated.load(Ordering::Relaxed), 1);
    assert!(
        world.take_generated(0).is_none(),
        "memory worlds never enqueue durable generated bases"
    );
}

#[test]
fn checkpoint_bases_are_immutable_and_persisted_bases_are_not_regenerated() {
    let directory = tempfile::tempdir().unwrap();
    let registry = CoreRegistry::load(Some(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
    ))
    .unwrap();
    let store = crate::world::world_tests::native_store(directory.path(), &registry);
    let generated = Arc::new(AtomicUsize::new(0));
    let source = source(
        Arc::clone(&store),
        0,
        mc_world::OVERWORLD_GEOMETRY,
        &registry,
        &generated,
    );
    let pos = ChunkPos { x: 0, z: 0 };
    let mut world = ChunkWorld::new_native(
        42,
        Arc::clone(&store),
        0,
        Arc::clone(&source.generator),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(&[(pos, 0)].into_iter().collect());
    let immutable_generated = world.take_generated(0).unwrap();
    world.set_block(4, 100, 4, registry.states.dirt);
    world.fold_overlay();
    let capture = world.capture_dirty(&Default::default());
    let mut visits = 0;
    capture
        .visit_native(|snapshot| {
            visits += 1;
            assert_eq!(snapshot.dimension, 0);
            assert_eq!(
                snapshot.base.get_block(4, 100, 4).unwrap().0,
                registry.states.air
            );
            assert_eq!(
                snapshot.effective.get_block(4, 100, 4).unwrap().0,
                registry.states.dirt
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(visits, 1);
    assert!(Arc::ptr_eq(capture.store().unwrap(), &store));
    assert!(Arc::ptr_eq(capture.registry(), &registry.blocks));
    let written = capture.write().unwrap();
    world.mark_flushed(&written);
    let after_first_checkpoint = generated.load(Ordering::Relaxed);
    immutable_generated.fill_generated().unwrap();
    assert_eq!(
        source
            .load_native(pos)
            .unwrap()
            .unwrap()
            .get_block(4, 100, 4)
            .unwrap()
            .0,
        registry.states.dirt,
        "delayed generated save preserves effective gameplay",
    );
    world.set_block(5, 100, 4, registry.states.stone);
    world.fold_overlay();
    world.flush_dirty(&Default::default()).unwrap();
    assert_eq!(
        generated.load(Ordering::Relaxed),
        after_first_checkpoint,
        "the first checkpoint stored the immutable base",
    );
    let capture = world.capture_dirty(&Default::default());
    assert!(
        capture.fill_generated().is_err(),
        "effective captures are not generation bases"
    );
}

#[test]
fn absent_checkpoint_metadata_is_frozen_and_prepared_off_tick_in_each_dimension() {
    let directory = tempfile::tempdir().unwrap();
    let registry = CoreRegistry::load(Some(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
    ))
    .unwrap();
    let store = crate::world::world_tests::native_store(directory.path(), &registry);
    for dimension in 0..3 {
        let geometry = if dimension == 0 {
            mc_world::OVERWORLD_GEOMETRY
        } else {
            mc_world::ChunkGeometry::new(0, 256).unwrap()
        };
        let generated = Arc::new(AtomicUsize::new(0));
        let source = source(
            Arc::clone(&store),
            dimension,
            geometry,
            &registry,
            &generated,
        );
        let pos = ChunkPos { x: 3, z: -2 };
        let block = mc_world::BlockPos {
            x: 49,
            y: 100,
            z: -31,
        };
        let mut world = ChunkWorld::new_native(
            42,
            Arc::clone(&store),
            dimension,
            Arc::clone(&source.generator),
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [block.x, block.y, block.z],
            geometry,
        );
        let mut replay = crate::journal::Replay {
            tick: 20,
            ..Default::default()
        };
        replay.inhabited_time.insert(
            [pos.x, pos.z],
            crate::journal::InhabitedTimeImage {
                chunk: [pos.x, pos.z],
                value: 70,
                tick: 10,
                active: true,
            },
        );
        world.restore_inhabited_time(&replay, None);
        let blocks = vec![mc_world::ScheduledBlockTick::new(
            block,
            mc_data::Identifier::parse("minecraft:stone").unwrap(),
            3,
            -1,
        )];
        let fluids = vec![mc_world::ScheduledFluidTick::new(
            block,
            mc_data::Identifier::parse("minecraft:water").unwrap(),
            4,
            1,
        )];
        let original_counts = store.record_counts().unwrap();
        let mut capture = world.capture_dirty(&Default::default());
        world.capture_scheduled_block_ticks(&mut capture, vec![(pos, blocks.clone())]);
        world.capture_scheduled_fluid_ticks(&mut capture, vec![(pos, fluids.clone())]);
        assert_eq!(
            generated.load(Ordering::Relaxed),
            0,
            "capture must not load, generate, or bake an absent base"
        );
        assert!(!world.has_chunk(pos.x, pos.z));
        assert!(
            world.snapshot().is_empty(),
            "no incidental simulation admission"
        );
        assert_eq!(store.record_counts().unwrap(), original_counts);
        assert_eq!(capture.written().len(), 1, "domains compose into one image");

        // Gameplay admission before journal preparation restores the live
        // interval, without changing the capture's frozen counter or queues.
        world.ensure_blocking(&[(pos, 0)].into_iter().collect());
        assert_eq!(world.chunk(pos).unwrap().inhabited_time(), 80);
        world.tick_inhabited_time(pos, 21);
        world.finish_inhabited_tick(21);
        world.set_scheduled_block_ticks(pos, Vec::new());
        world.set_scheduled_fluid_ticks(pos, Vec::new());
        let mut visits = 0;
        capture
            .visit_native(|snapshot| {
                visits += 1;
                assert_eq!(snapshot.dimension, dimension);
                assert_eq!(snapshot.base.inhabited_time(), 0);
                assert!(snapshot.base.scheduled_block_ticks().is_empty());
                assert!(snapshot.base.scheduled_fluid_ticks().is_empty());
                assert_eq!(snapshot.effective.inhabited_time(), 80);
                assert_eq!(snapshot.effective.scheduled_block_ticks(), blocks);
                assert_eq!(snapshot.effective.scheduled_fluid_ticks(), fluids);
                Ok(())
            })
            .unwrap();
        assert_eq!(visits, 1);
        world.mark_flushed(&capture.write().unwrap());
        assert!(
            !world.evict_outside(&Default::default()).contains(&pos),
            "admission and a newer interval fence the old absent ACK"
        );
        assert_eq!(world.inhabited_time(pos), 81);
        world.flush_dirty(&Default::default()).unwrap();
        let durable = source.load_native(pos).unwrap().unwrap();
        assert_eq!(durable.inhabited_time(), 81);
        assert!(durable.scheduled_block_ticks().is_empty());
        assert!(durable.scheduled_fluid_ticks().is_empty());
    }
}

#[test]
fn deferred_native_metadata_rebakes_untrusted_imported_light_without_admission() {
    let directory = tempfile::tempdir().unwrap();
    let registry = CoreRegistry::load(Some(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
    ))
    .unwrap();
    let store = crate::world::world_tests::native_store(directory.path(), &registry);
    let generated = Arc::new(AtomicUsize::new(0));
    let source = source(
        Arc::clone(&store),
        0,
        mc_world::OVERWORLD_GEOMETRY,
        &registry,
        &generated,
    );
    let pos = ChunkPos { x: 0, z: 0 };
    let mut imported = source.generator.generate(pos);
    imported.set_block(8, 100, 8, mc_world::BlockStateId(registry.states.torch));
    // No baked arrays: importing metadata must never certify derived lighting.
    assert!(mc_world::light::ChunkLight::from_chunk(&imported).is_none());
    store
        .import_chunk(0, &imported, &registry.blocks, &registry.items)
        .unwrap();
    let mut oracle = imported.clone();
    crate::light::bake_missing(&mut oracle, &registry.light);
    generated.store(0, Ordering::Relaxed);
    let mut world = ChunkWorld::new_native(
        42,
        Arc::clone(&store),
        0,
        Arc::clone(&source.generator),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 100, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let block = mc_world::BlockPos { x: 8, y: 100, z: 8 };
    let mut capture = world.capture_dirty(&Default::default());
    world.capture_scheduled_block_ticks(
        &mut capture,
        vec![(
            pos,
            vec![mc_world::ScheduledBlockTick::new(
                block,
                mc_data::Identifier::parse("minecraft:torch").unwrap(),
                1,
                0,
            )],
        )],
    );
    let mut visits = 0;
    capture
        .visit_native(|snapshot| {
            visits += 1;
            assert_eq!(snapshot.effective.section_lights, oracle.section_lights);
            assert_eq!(
                snapshot.effective.boundary_section_lights,
                oracle.boundary_section_lights
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(visits, 1);
    assert_eq!(
        generated.load(Ordering::Relaxed),
        0,
        "imported coverage wins"
    );
    assert!(!world.has_chunk(pos.x, pos.z));
}

#[test]
fn absent_metadata_visitor_streams_one_chunk_and_late_failure_never_acks() {
    let directory = tempfile::tempdir().unwrap();
    let registry = CoreRegistry::load(Some(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
    ))
    .unwrap();
    let store = crate::world::world_tests::native_store(directory.path(), &registry);
    let generated = Arc::new(AtomicUsize::new(0));
    let source = source(
        Arc::clone(&store),
        0,
        mc_world::OVERWORLD_GEOMETRY,
        &registry,
        &generated,
    );
    let mut world = ChunkWorld::new_native(
        42,
        Arc::clone(&store),
        0,
        Arc::clone(&source.generator),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 100, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    // Three absent metadata-only captures: nothing resident, nothing generated.
    let mut replay = crate::journal::Replay {
        tick: 20,
        ..Default::default()
    };
    for (x, z) in [(0, 0), (1, 0), (2, 0)] {
        replay.inhabited_time.insert(
            [x, z],
            crate::journal::InhabitedTimeImage {
                chunk: [x, z],
                value: 40,
                tick: 20,
                active: false,
            },
        );
    }
    world.restore_inhabited_time(&replay, None);
    let capture = world.capture_dirty(&Default::default());
    assert_eq!(capture.written().len(), 3);
    assert_eq!(generated.load(Ordering::Relaxed), 0);
    assert!(world.snapshot().is_empty());
    // The visitor resolves one absent base/effective pair at a time: a late
    // preparation error aborts the stream without ACK or partial publish.
    let mut visited = 0;
    let late = capture
        .visit_native(|snapshot| {
            visited += 1;
            assert_eq!(snapshot.effective.inhabited_time(), 40);
            if visited == 2 {
                return Err(crate::config::CoreError::Storage(
                    "late chunk preparation failure".into(),
                ));
            }
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(late.contains("late chunk preparation failure"), "{late}");
    assert_eq!(visited, 2, "stream stops at the failing chunk");
    assert_eq!(store.read_checkpoint().unwrap().tick, 0);
    assert_eq!(capture.written().len(), 3, "failed capture remains pending");
    assert_eq!(generated.load(Ordering::Relaxed), 2);
    assert!(world.snapshot().is_empty(), "visiting never admits");
    // Retry succeeds end to end through the streaming fixture commit.
    let retry = world.capture_dirty(&Default::default());
    assert_eq!(retry.written(), capture.written());
    let mut retried = 0;
    retry
        .visit_native(|_| {
            retried += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(retried, 3);
    world.mark_flushed(&retry.write().unwrap());
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty(),
        "only a successful revision-matching ACK clears the absent images"
    );
}
