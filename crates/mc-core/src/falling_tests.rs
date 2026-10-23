use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;
fn setup() -> (TickState, BlockPos) {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let registry = Arc::new(CoreRegistry::load(Some(&dir)).unwrap());
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().unwrap();
    let world = ChunkWorld::new(
        42,
        Arc::new(terrain),
        &registry,
        BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "falling".into(),
            permission_level: 0,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let resident = state.world.resident_positions();
    for pos in &resident {
        state
            .tickets
            .insert(format!("falling-test:{}:{}", pos.x, pos.z), (*pos, 0));
    }
    state.refresh_active_chunks(state.tick);
    let id = state.connections.entity_of(ConnectionId(1)).unwrap();
    let p = state.entities.pos(state.entities.slot_of(id).unwrap());
    let pos = BlockPos {
        x: p[0].floor() as i32,
        y: 205,
        z: p[2].floor() as i32,
    };
    let air = state.registry.states.air;
    for y in 199..215 {
        state.world.set_block(pos.x, y, pos.z, air);
    }
    let stone = state.registry.states.stone;
    state.world.set_block(pos.x, 200, pos.z, stone);
    (state, pos)
}

#[test]
fn physics_torch_does_not_support_sand() {
    let (mut state, pos) = setup();
    let torch = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default
        .0;
    let sand = state.registry.states.sand;
    state.world.set_block(pos.x, pos.y - 1, pos.z, torch);
    state.world.set_block(pos.x, pos.y, pos.z, sand);
    let mut spawned = Vec::new();
    start(&mut state, &mut vec![(pos, sand)], &mut spawned);
    assert_eq!(spawned.len(), 1);
}

#[test]
fn physics_powder_hardens_at_water_entry_not_pool_bottom() {
    let (mut state, pos) = setup();
    let powder = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:white_concrete_powder").unwrap())
        .unwrap()
        .default
        .0;
    let water = state.registry.states.water;
    for y in 201..pos.y {
        state.world.set_block(pos.x, y, pos.z, water);
    }
    state.world.set_block(pos.x, pos.y, pos.z, powder);
    let mut writes = vec![(pos, powder)];
    let mut spawned = Vec::new();
    let mut removed = Vec::new();
    start(&mut state, &mut writes, &mut spawned);
    run(&mut state, &mut writes, &mut spawned, &mut removed);
    assert_eq!(removed.len(), 1);
    assert_eq!(
        state.world.block_at(pos.x, pos.y - 1, pos.z),
        concrete(&state.registry.blocks, powder)
    );
}

#[test]
fn physics_falling_hazards_queue_bounded_impact_damage() {
    for (name, multiplier) in [("anvil", 2.0), ("pointed_dripstone", 6.0)] {
        let (mut state, pos) = setup();
        let block = state
            .registry
            .blocks
            .block(&Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap()
            .default
            .0;
        let falling = state.spawn_mob(
            FALLING_BLOCK_TYPE.unwrap(),
            [pos.x as f64 + 0.5, 201.01, pos.z as f64 + 0.5],
            1.0,
        );
        let slot = state.entities.slot_of(falling).unwrap();
        state.entities.row_mut(slot).falling = Some(FallingState {
            block_state: block,
            age: 20,
            touched_water: false,
        });
        state.entities.row_mut(slot).controls.fall_distance = 5.0;
        let player = state.connections.entity_of(ConnectionId(1)).unwrap();
        let player_slot = state.entities.slot_of(player).unwrap();
        state
            .entities
            .set_pos(player_slot, [pos.x as f64 + 0.5, 201.0, pos.z as f64 + 0.5]);
        let mut removed = Vec::new();
        run(&mut state, &mut Vec::new(), &mut Vec::new(), &mut removed);
        assert_eq!(removed, vec![falling]);
        assert!(
            state
                .merge_damage
                .iter()
                .any(|(slot, damage)| state.entities.body(*slot).id == player
                    && damage.amount == 5.0 * multiplier)
        );
    }
}
#[test]
fn source_becomes_retained_entity_then_lands_once() {
    let (mut state, pos) = setup();
    let sand = state.registry.states.sand;
    state.world.set_block(pos.x, pos.y, pos.z, sand);
    let mut writes = vec![(pos, sand)];
    let mut spawned = Vec::new();
    start(&mut state, &mut writes, &mut spawned);
    assert_eq!(spawned.len(), 1);
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(state.registry.states.air)
    );
    let id = spawned[0];
    let slot = state.entities.slot_of(id).unwrap();
    let image = serde_json::to_vec(&state.entities.row(slot).falling.unwrap()).unwrap();
    let restored: FallingState = serde_json::from_slice(&image).unwrap();
    state.entities.row_mut(slot).falling = Some(restored);
    let mut removed = Vec::new();
    for _ in 0..40 {
        run(&mut state, &mut writes, &mut spawned, &mut removed);
    }
    assert_eq!(removed, vec![id]);
    assert!(state.entities.slot_of(id).is_none());
    assert_eq!(state.world.block_at(pos.x, 201, pos.z), Some(sand));
    start(&mut state, &mut writes, &mut spawned);
    assert_eq!(spawned.len(), 1, "landing remains supported");
}
#[test]
fn blocked_landing_drops_once_and_shattering_states_leave_nothing() {
    let (mut state, pos) = setup();
    let sand = state.registry.states.sand;
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:falling_block").unwrap())
        .unwrap();
    let stone = state.registry.states.stone;
    state.world.set_block(pos.x, 201, pos.z, stone);
    let id = state.spawn_mob(
        type_id,
        [pos.x as f64 + 0.5, 201.01, pos.z as f64 + 0.5],
        1.0,
    );
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).falling = Some(FallingState {
        block_state: sand,
        age: 1,
        touched_water: false,
    });
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    let mut removed = Vec::new();
    run(&mut state, &mut writes, &mut spawned, &mut removed);
    assert_eq!(removed, vec![id]);
    assert_eq!(spawned.len(), 1);
    run(&mut state, &mut writes, &mut spawned, &mut removed);
    assert_eq!(spawned.len(), 1);
    let air = state.registry.states.air;
    state.world.set_block(pos.x, 201, pos.z, air);
    let suspicious = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:suspicious_sand").unwrap())
        .unwrap()
        .default
        .0;
    let id = state.spawn_mob(
        type_id,
        [pos.x as f64 + 0.5, 201.01, pos.z as f64 + 0.5],
        1.0,
    );
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).falling = Some(FallingState {
        block_state: suspicious,
        age: 1,
        touched_water: false,
    });
    run(&mut state, &mut writes, &mut spawned, &mut removed);
    assert!(removed.contains(&id));
    assert_eq!(spawned.len(), 1);
    assert_eq!(
        state.world.block_at(pos.x, 201, pos.z),
        Some(state.registry.states.air)
    );
}
#[test]
fn water_contact_survives_retained_image_and_hardens_powder() {
    let (mut state, pos) = setup();
    let powder = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:white_concrete_powder").unwrap())
        .unwrap()
        .default
        .0;
    let mut image = FallingState {
        block_state: powder,
        age: 15,
        touched_water: true,
    };
    image = serde_json::from_slice(&serde_json::to_vec(&image).unwrap()).unwrap();
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:falling_block").unwrap())
        .unwrap();
    let id = state.spawn_mob(
        type_id,
        [pos.x as f64 + 0.5, 201.01, pos.z as f64 + 0.5],
        1.0,
    );
    state
        .entities
        .row_mut(state.entities.slot_of(id).unwrap())
        .falling = Some(image);
    run(
        &mut state,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
    );
    assert_eq!(
        state.world.block_at(pos.x, 201, pos.z),
        concrete(&state.registry.blocks, powder)
    );
}

#[test]
fn falling_motion_restarts_from_fsynced_absolute_wal_without_recreating_source() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let (mut original, pos) = setup();
    let sand = original.registry.states.sand;
    original.world.set_block(pos.x, pos.y, pos.z, sand);
    start(&mut original, &mut vec![(pos, sand)], &mut Vec::new());
    let (output, _, _) = run_tick(&mut original, &[]);
    let id = original
        .entities
        .slots()
        .find(|slot| original.entities.row(*slot).falling.is_some())
        .map(|slot| original.entities.body(slot).id)
        .unwrap();
    let entries = crate::core::journal_entries_for_tick(&mut original, &output);
    let dir = tempfile::tempdir().unwrap();
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    writer.append(1, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    let replay = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&dir.path().join("journal"), 0).unwrap(),
    );
    let recovered = &replay.entities[&id.0];
    assert!(recovered.state.falling.is_some());
    assert!(recovered.pos[1] < pos.y as f64);
    assert!(recovered.vel[1] < 0.0);
    let (mut restarted, restart_pos) = setup();
    assert_eq!(pos, restart_pos);
    let restored = restarted.spawn_mob(recovered.type_id, recovered.pos, 1.0);
    let slot = restarted.entities.slot_of(restored).unwrap();
    restarted.entities.set_vel(slot, recovered.vel);
    restarted.entities.row_mut(slot).falling = recovered.state.falling;
    configure(&mut restarted, restored);
    for _ in 0..40 {
        run(
            &mut original,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        );
        run(
            &mut restarted,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
        );
    }
    assert_eq!(original.world.block_at(pos.x, 201, pos.z), Some(sand));
    assert_eq!(restarted.world.block_at(pos.x, 201, pos.z), Some(sand));
    assert_eq!(
        restarted.world.block_at(pos.x, pos.y, pos.z),
        Some(restarted.registry.states.air)
    );
    assert!(
        restarted
            .entities
            .slots()
            .all(|slot| restarted.entities.row(slot).falling.is_none())
    );
}

#[test]
fn empty_collision_attachment_is_not_a_floor_and_slab_landing_drops_instead() {
    let (mut state, pos) = setup();
    let sand = state.registry.states.sand;
    let torch = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:torch").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(pos.x, 201, pos.z, torch);
    state.world.set_block(pos.x, pos.y, pos.z, sand);
    start(&mut state, &mut vec![(pos, sand)], &mut Vec::new());
    let mut spawned = Vec::new();
    let mut removed = Vec::new();
    for _ in 0..40 {
        run(&mut state, &mut Vec::new(), &mut spawned, &mut removed);
    }
    assert_eq!(
        state.world.block_at(pos.x, 201, pos.z),
        Some(sand),
        "empty collision torch must not suspend falling sand above it"
    );
    assert!(
        spawned.is_empty(),
        "OLD landing replacement does not separately wash loot"
    );
    let (mut slab_state, pos) = setup();
    let sand = slab_state.registry.states.sand;
    let slab = slab_state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:oak_slab").unwrap())
        .unwrap()
        .default
        .0;
    slab_state.world.set_block(pos.x, 201, pos.z, slab);
    slab_state.world.set_block(pos.x, pos.y, pos.z, sand);
    start(&mut slab_state, &mut vec![(pos, sand)], &mut Vec::new());
    let mut spawned = Vec::new();
    let mut removed = Vec::new();
    for _ in 0..40 {
        run(&mut slab_state, &mut Vec::new(), &mut spawned, &mut removed);
    }
    assert_eq!(slab_state.world.block_at(pos.x, 201, pos.z), Some(slab));
    assert_eq!(
        spawned.len(),
        1,
        "blocked partial-shape landing drops one item"
    );
    assert_eq!(removed.len(), 1);
}

#[test]
fn malformed_falling_fields_report_the_exact_field_and_nonfalling_is_not_an_error() {
    let registry = CoreRegistry::load(None).unwrap();
    assert_eq!(from_record(&Default::default(), &registry).unwrap(), None);
    let mut record = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:falling_block".into(),
        ..Default::default()
    };
    assert!(
        from_record(&record, &registry)
            .unwrap_err()
            .starts_with("BlockState:")
    );
    record.extra_fields = vec![(
        "BlockState".into(),
        Tag::Compound(vec![
            ("Name".into(), Tag::String("minecraft:sand".into())),
            (
                "Properties".into(),
                Tag::Compound(vec![("unknown".into(), Tag::String("bad".into()))]),
            ),
        ]),
    )];
    assert!(
        from_record(&record, &registry)
            .unwrap_err()
            .starts_with("BlockState.Properties.unknown:")
    );
    record.extra_fields = vec![(
        "BlockState".into(),
        Tag::Compound(vec![
            ("Name".into(), Tag::String("minecraft:sand".into())),
            ("Properties".into(), Tag::Int(1)),
        ]),
    )];
    assert!(
        from_record(&record, &registry)
            .unwrap_err()
            .starts_with("BlockState.Properties:")
    );
    record.extra_fields = vec![(
        "BlockState".into(),
        Tag::Compound(vec![("Name".into(), Tag::String("minecraft:stone".into()))]),
    )];
    assert_eq!(
        from_record(&record, &registry).unwrap_err(),
        "BlockState.Name: not a gravity block"
    );
    record.extra_fields = to_fields(
        &FallingState {
            block_state: registry.states.sand,
            age: 9,
            touched_water: true,
        },
        &registry.blocks,
    );
    assert_eq!(from_record(&record, &registry).unwrap().unwrap().age, 9);
    record
        .extra_fields
        .iter_mut()
        .find(|(key, _)| key == "Time")
        .unwrap()
        .1 = Tag::Int(-1);
    assert_eq!(
        from_record(&record, &registry).unwrap_err(),
        "Time: expected nonnegative int"
    );
    record.pos[1] = f64::INFINITY;
    assert_eq!(
        from_record(&record, &registry).unwrap_err(),
        "Pos[1]: non-finite number"
    );
}

#[test]
fn falling_entity_drops_are_independent_of_block_and_mob_drops() {
    let (mut state, pos) = setup();
    let sand = state.registry.states.sand;
    state.gamerules.insert("block_drops".into(), "false".into());
    state.gamerules.insert("mob_drops".into(), "false".into());
    let mut spawned = Vec::new();
    drop(&mut state, pos, sand, &mut spawned);
    assert_eq!(spawned.len(), 1);
    state
        .gamerules
        .insert("entity_drops".into(), "false".into());
    drop(&mut state, pos, sand, &mut spawned);
    assert_eq!(
        spawned.len(),
        1,
        "entity_drops disables only entity landing loot"
    );
}
