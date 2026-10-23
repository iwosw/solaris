use super::*;

#[test]
fn chunk_rectangle_for_world_bounds_handles_edges_negatives_and_caps() {
    assert_eq!(
        chunk_rectangle_for_world_bounds(0.0, 15.999, 0.0, 15.999, 4),
        Some(vec![ChunkPos { x: 0, z: 0 }])
    );
    assert_eq!(
        chunk_rectangle_for_world_bounds(-0.001, 16.0, -16.0, 0.0, 9),
        Some(vec![
            ChunkPos { x: -1, z: -1 },
            ChunkPos { x: -1, z: 0 },
            ChunkPos { x: 0, z: -1 },
            ChunkPos { x: 0, z: 0 },
            ChunkPos { x: 1, z: -1 },
            ChunkPos { x: 1, z: 0 },
        ])
    );
    assert_eq!(
        chunk_rectangle_for_world_bounds(-0.001, 16.0, -16.0, 0.0, 5),
        None
    );
    assert_eq!(
        chunk_rectangle_for_world_bounds(f64::NAN, 0.0, 0.0, 0.0, 1),
        None
    );
    assert_eq!(
        chunk_rectangle_for_world_bounds(1.0, 0.0, 0.0, 0.0, 1),
        None
    );
}

fn air() -> BlockStateId {
    BlockStateId(0)
}
fn stone() -> BlockStateId {
    BlockStateId(1)
}
fn plains() -> Identifier {
    Identifier::parse("minecraft:plains").unwrap()
}
fn wheat() -> Identifier {
    Identifier::parse("minecraft:wheat").unwrap()
}
fn water() -> Identifier {
    Identifier::parse("minecraft:water").unwrap()
}

#[test]
fn y_mapping_covers_full_range() {
    assert_eq!(OVERWORLD_GEOMETRY.world_y_to_section(MIN_Y), Some((0, 0)));
    assert_eq!(
        OVERWORLD_GEOMETRY.world_y_to_section(MAX_Y - 1),
        Some((SECTION_COUNT - 1, 15))
    );
    assert_eq!(OVERWORLD_GEOMETRY.world_y_to_section(0), Some((4, 0)));
    assert_eq!(OVERWORLD_GEOMETRY.world_y_to_section(MIN_Y - 1), None);
    assert_eq!(OVERWORLD_GEOMETRY.world_y_to_section(MAX_Y), None);
}

#[test]
fn empty_chunk_is_all_air() {
    let c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    assert_eq!(c.sections.len(), SECTION_COUNT);
    assert_eq!(c.biomes.len(), SECTION_COUNT);
    assert_eq!(c.get_block(0, MIN_Y, 0), Some(air()));
    assert_eq!(c.get_block(15, MAX_Y - 1, 15), Some(air()));
    assert_eq!(c.get_block(0, MIN_Y - 1, 0), None);
    assert_eq!(c.get_block(0, MAX_Y, 0), None);
}

#[test]
fn generated_entity_markers_survive_anvil_nbt_persistence() {
    let mut chunk = Chunk::empty(ChunkPos { x: 4, z: 0 }, air(), plains());
    let marker = GeneratedEntityMarker {
        claim: "minecraft:village/plains/toolsmith@72:66:8#0".to_owned(),
        entity_type: "minecraft:villager".to_owned(),
        position: [72.5, 66.0, 8.5],
        persistent: true,
        villager_kind: "plains".to_owned(),
        profession: "toolsmith".to_owned(),
        level: 1,
        age: -21_359,
        yaw: 0.0,
        pitch: -25.827_711,
    };

    chunk.set_generated_entities(std::slice::from_ref(&marker));
    let registry = crate::BlockRegistry::from_report(&[mc_data::blocks::BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: BTreeMap::new(),
        }],
    }])
    .unwrap();
    let root = crate::anvil::chunk_nbt::chunk_to_nbt(&chunk, &registry).unwrap();
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &root).unwrap();
    let (_, reloaded_root) = mc_nbt::read_named(&mut bytes.as_slice()).unwrap();
    let reloaded = crate::anvil::chunk_nbt::chunk_from_nbt(
        &reloaded_root,
        &registry,
        crate::OVERWORLD_GEOMETRY,
    )
    .unwrap();

    assert_eq!(reloaded.generated_entities(), vec![marker]);
}

#[test]
fn custom_geometry_controls_chunk_sections_and_block_bounds() {
    assert!(ChunkGeometry::new(0, 512).is_none());
    let geometry = ChunkGeometry::new(0, 256).unwrap();
    let mut chunk = Chunk::empty_with_geometry(ChunkPos { x: 2, z: -3 }, air(), plains(), geometry);

    assert_eq!(chunk.geometry(), geometry);
    assert_eq!(chunk.sections.len(), 16);
    assert_eq!(chunk.biomes.len(), 16);
    assert_eq!(chunk.get_block(0, 0, 0), Some(air()));
    assert_eq!(chunk.set_block(15, 255, 15, stone()), Some(air()));
    assert_eq!(chunk.get_block(15, 255, 15), Some(stone()));
    assert_eq!(chunk.get_block(0, -1, 0), None);
    assert_eq!(chunk.get_block(0, 256, 0), None);

    chunk
        .heightmaps
        .insert("WORLD_SURFACE".into(), Heightmap::zeroed());
    assert_eq!(
        chunk.set_block_and_update(3, 200, 7, stone(), air()),
        Some(air())
    );
    assert_eq!(chunk.heightmaps["WORLD_SURFACE"].get(3, 7), 201);
    chunk.highest_opaque.set(3, 7, 201);
    assert_eq!(chunk.highest_opaque_y(3, 7), Some(200));

    assert!(chunk.contains_block_pos(BlockPos {
        x: 32,
        y: 0,
        z: -48,
    }));
    assert!(!chunk.contains_block_pos(BlockPos {
        x: 32,
        y: -1,
        z: -48,
    }));
}

#[test]
fn set_block_round_trips_across_sections() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    // Set at the bottom, the build-height boundary, and the top.
    let probes = [MIN_Y, -1, 0, 63, 319];
    for &y in &probes {
        assert_eq!(c.set_block(3, y, 7, stone()), Some(air()));
        assert_eq!(c.get_block(3, y, 7), Some(stone()));
        assert_eq!(c.get_block(2, y, 7), Some(air()));
    }
    // Non-probed columns still air.
    assert_eq!(c.get_block(0, 0, 0), Some(air()));
}

#[test]
fn biome_section_default_is_single() {
    let b = BiomeSection::filled(plains());
    for y in 0..4 {
        for z in 0..4 {
            for x in 0..4 {
                assert_eq!(b.get(x, y, z).as_str(), "minecraft:plains");
            }
        }
    }
}

#[test]
fn heightmap_round_trip() {
    let mut h = Heightmap::zeroed();
    h.set(0, 0, 64);
    h.set(15, 15, 320);
    h.set(7, 3, 0);
    assert_eq!(h.get(0, 0), 64);
    assert_eq!(h.get(15, 15), 320);
    assert_eq!(h.get(7, 3), 0);
    assert_eq!(h.get(1, 1), 0);
}

#[test]
fn min_section_y_constant_is_correct() {
    assert_eq!(MIN_SECTION_Y, -4);
    assert_eq!(MIN_SECTION_Y + SECTION_COUNT as i32, 20);
}

#[test]
fn set_block_and_update_recomputes_present_heightmaps() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    // Pre-existing heightmap (e.g. surface-Y from generation):
    // empty column → value 0. Insert it explicitly so the helper
    // has something to recompute.
    c.heightmaps
        .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
    c.heightmaps
        .insert("WORLD_SURFACE".into(), Heightmap::zeroed());

    // Drop a stone at world Y=64. The heightmap value is
    // (Y - MIN_Y + 1) = 64 - (-64) + 1 = 129.
    let prev = c.set_block_and_update(3, 64, 7, stone(), air()).unwrap();
    assert_eq!(prev, air());
    assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(3, 7), 129);
    assert_eq!(c.heightmaps.get("WORLD_SURFACE").unwrap().get(3, 7), 129);

    // Other columns stay at 0.
    assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(0, 0), 0);

    // Break it back to air — heightmap returns to 0.
    c.set_block_and_update(3, 64, 7, air(), air()).unwrap();
    assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(3, 7), 0);
}

#[test]
fn set_block_and_update_picks_next_lower_top_after_break() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    c.heightmaps
        .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
    // Stack two blocks at Y=63 and Y=64; heightmap should track
    // the higher one and fall back to the lower on break.
    c.set_block_and_update(3, 63, 7, stone(), air()).unwrap();
    c.set_block_and_update(3, 64, 7, stone(), air()).unwrap();
    assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(3, 7), 129);

    c.set_block_and_update(3, 64, 7, air(), air()).unwrap();
    assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(3, 7), 128);
}

#[test]
fn set_block_and_update_with_no_heightmaps_is_a_noop_for_them() {
    // Some chunks (Status: empty / structure_starts in our test
    // world) ship no heightmaps. The helper must not panic and
    // must still mutate the underlying block.
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    assert!(c.heightmaps.is_empty());
    let prev = c.set_block_and_update(0, 0, 0, stone(), air()).unwrap();
    assert_eq!(prev, air());
    assert_eq!(c.get_block(0, 0, 0), Some(stone()));
    assert!(c.heightmaps.is_empty());
}

#[test]
fn set_block_and_update_clears_stale_baked_light_layers() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    c.section_lights[0].sky = Some(LightSection::uniform(0xFF));
    c.section_lights[0].block = Some(LightSection::uniform(0x11));
    c.section_lights[3].sky = Some(LightSection::uniform(0x22));

    let prev = c.set_block_and_update(0, 0, 0, stone(), air()).unwrap();

    assert_eq!(prev, air());
    assert!(
        c.section_lights
            .iter()
            .all(|section| section.sky.is_none() && section.block.is_none()),
        "block mutation must invalidate baked light arrays before they can be reused"
    );
}

#[test]
fn highest_opaque_uses_light_table_sky_predicate() {
    let table = BlockLightTable::from_arrays(
        "test",
        vec![0, 0, 0],
        vec![0, 15, 0],
        vec![true, false, true],
    );
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    let glass = BlockStateId(2);

    c.set_block(3, 80, 7, glass);
    c.set_block(3, 64, 7, stone());
    c.rebuild_highest_opaque(&table);
    assert_eq!(c.highest_opaque_y(3, 7), Some(64));

    c.set_block(3, 64, 7, air());
    c.update_highest_opaque_column(3, 7, &table);
    assert_eq!(c.highest_opaque_y(3, 7), None);
}

#[test]
fn scheduled_block_ticks_drain_in_deterministic_order() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    let pos_a = BlockPos { x: 1, y: 64, z: 1 };
    let pos_b = BlockPos { x: 2, y: 64, z: 1 };
    let pos_c = BlockPos { x: 3, y: 64, z: 1 };

    assert!(c.schedule_block_tick(ScheduledBlockTick::new(pos_a, wheat(), 10, 0)));
    assert!(c.schedule_block_tick(ScheduledBlockTick::new(pos_b, wheat(), 5, 0)));
    assert!(c.schedule_block_tick(ScheduledBlockTick::new(pos_c, wheat(), 10, -1)));

    c.dirty = false;
    let due = c.drain_due_block_ticks(10, usize::MAX);

    assert_eq!(
        due.iter().map(|tick| tick.pos).collect::<Vec<_>>(),
        vec![pos_b, pos_c, pos_a]
    );
    assert!(c.scheduled_block_ticks().is_empty());
    assert!(c.dirty);
}

#[test]
fn scheduled_block_ticks_skip_duplicates_and_mark_dirty_only_on_change() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    let tick = ScheduledBlockTick::new(BlockPos { x: 1, y: 64, z: 1 }, wheat(), 10, 0);

    assert!(c.schedule_block_tick(tick.clone()));
    c.dirty = false;
    assert!(!c.schedule_block_tick(tick));
    assert_eq!(c.scheduled_block_ticks().len(), 1);
    assert!(!c.dirty);

    assert!(c.drain_due_block_ticks(9, usize::MAX).is_empty());
    assert!(!c.dirty);
    assert!(c.drain_due_block_ticks(10, 0).is_empty());
    assert!(!c.dirty);
}

#[test]
fn scheduled_block_ticks_reject_positions_outside_chunk() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());

    assert!(!c.schedule_block_tick(ScheduledBlockTick::new(
        BlockPos { x: 16, y: 64, z: 1 },
        wheat(),
        10,
        0,
    )));
    assert!(!c.schedule_block_tick(ScheduledBlockTick::new(
        BlockPos {
            x: 1,
            y: MAX_Y,
            z: 1
        },
        wheat(),
        10,
        0,
    )));
    assert!(c.scheduled_block_ticks().is_empty());
    assert!(!c.dirty);
}

#[test]
fn scheduled_block_ticks_remove_by_position() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    let pos = BlockPos { x: 1, y: 64, z: 1 };
    let other = BlockPos { x: 2, y: 64, z: 1 };
    assert!(c.schedule_block_tick(ScheduledBlockTick::new(pos, wheat(), 10, 0)));
    assert!(c.schedule_block_tick(ScheduledBlockTick::new(other, wheat(), 10, 0)));

    c.dirty = false;
    let removed = c.remove_scheduled_block_ticks_at(pos);

    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].pos, pos);
    assert_eq!(c.scheduled_block_ticks().len(), 1);
    assert_eq!(c.scheduled_block_ticks()[0].pos, other);
    assert!(c.dirty);

    c.dirty = false;
    assert!(c.remove_scheduled_block_ticks_at(pos).is_empty());
    assert!(!c.dirty);
}

#[test]
fn scheduled_fluid_ticks_drain_in_deterministic_order() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    let pos_a = BlockPos { x: 1, y: 64, z: 1 };
    let pos_b = BlockPos { x: 2, y: 64, z: 1 };
    let pos_c = BlockPos { x: 3, y: 64, z: 1 };

    assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(pos_a, water(), 10, 0)));
    assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(pos_b, water(), 5, 0)));
    assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(pos_c, water(), 10, -1)));

    c.dirty = false;
    let due = c.drain_due_fluid_ticks(10, usize::MAX);

    assert_eq!(
        due.iter().map(|tick| tick.pos).collect::<Vec<_>>(),
        vec![pos_b, pos_c, pos_a]
    );
    assert!(c.scheduled_fluid_ticks().is_empty());
    assert!(c.dirty);
}

#[test]
fn scheduled_fluid_ticks_skip_duplicates_and_mark_dirty_only_on_change() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    let tick = ScheduledFluidTick::new(BlockPos { x: 1, y: 64, z: 1 }, water(), 10, 0);

    assert!(c.schedule_fluid_tick(tick.clone()));
    c.dirty = false;
    assert!(!c.schedule_fluid_tick(tick));
    assert_eq!(c.scheduled_fluid_ticks().len(), 1);
    assert!(!c.dirty);

    assert!(c.drain_due_fluid_ticks(9, usize::MAX).is_empty());
    assert!(!c.dirty);
    assert!(c.drain_due_fluid_ticks(10, 0).is_empty());
    assert!(!c.dirty);
}

#[test]
fn scheduled_fluid_ticks_reject_positions_outside_chunk() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());

    assert!(!c.schedule_fluid_tick(ScheduledFluidTick::new(
        BlockPos { x: 16, y: 64, z: 1 },
        water(),
        10,
        0,
    )));
    assert!(!c.schedule_fluid_tick(ScheduledFluidTick::new(
        BlockPos {
            x: 1,
            y: MAX_Y,
            z: 1,
        },
        water(),
        10,
        0,
    )));
    assert!(c.scheduled_fluid_ticks().is_empty());
    assert!(!c.dirty);
}

#[test]
fn scheduled_fluid_ticks_remove_by_position() {
    let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
    let pos = BlockPos { x: 1, y: 64, z: 1 };
    let other = BlockPos { x: 2, y: 64, z: 1 };
    assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(pos, water(), 10, 0)));
    assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(other, water(), 10, 0)));

    c.dirty = false;
    let removed = c.remove_scheduled_fluid_ticks_at(pos);

    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].pos, pos);
    assert_eq!(c.scheduled_fluid_ticks().len(), 1);
    assert_eq!(c.scheduled_fluid_ticks()[0].pos, other);
    assert!(c.dirty);

    c.dirty = false;
    assert!(c.remove_scheduled_fluid_ticks_at(pos).is_empty());
    assert!(!c.dirty);
}
