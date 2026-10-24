use super::*;
use crate::journal::{
    CheckpointManifest, JournalEntry, JournalWriter, Replay, read_complete_batches_after,
    replay_batches,
};
use crate::native_checkpoint::tests::{read_manifest, write_manifest};
fn named(state: &TickState, name: &str, overrides: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
    let block = state.registry.blocks.block(&id).unwrap();
    let mut props = state
        .registry
        .blocks
        .by_id(block.default)
        .unwrap()
        .properties
        .clone();
    for (key, value) in overrides {
        props.iter_mut().find(|(name, _)| name == key).unwrap().1 = (*value).into();
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &props)
        .unwrap()
        .0
}

#[test]
fn physics_lava_preserves_waterlogged_block_below() {
    let (mut state, _, pos) = setup();
    let wet = named(&state, "oak_fence", &[("waterlogged", "true")]);
    let lava = state.registry.states.lava_levels[0];
    place(&mut state, pos, wet);
    place(
        &mut state,
        BlockPos {
            y: pos.y + 1,
            ..pos
        },
        lava,
    );
    run(&mut state, 30);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(wet));
}

#[test]
fn physics_waterlogged_slab_seals_its_bottom_face() {
    let (mut state, _, pos) = setup();
    let wet = named(
        &state,
        "oak_slab",
        &[("type", "bottom"), ("waterlogged", "true")],
    );
    let air = state.registry.states.air;
    place(
        &mut state,
        BlockPos {
            y: pos.y - 1,
            ..pos
        },
        air,
    );
    place(&mut state, pos, wet);
    run(&mut state, 5);
    assert_eq!(state.world.block_at(pos.x, pos.y - 1, pos.z), Some(air));
}

#[test]
fn physics_water_selects_nearest_descent() {
    let (mut state, _, pos) = setup();
    let air = state.registry.states.air;
    let water = state.registry.states.water;
    place(
        &mut state,
        BlockPos {
            x: pos.x + 1,
            y: pos.y - 1,
            ..pos
        },
        air,
    );
    place(&mut state, pos, water);
    // Water delay is 5 ticks; the out-of-band test placement stamps its
    // deadline on the already-processed tick, so the 5-tick delay lands on
    // the 6th harness run — the descent selection itself is the behavior.
    run(&mut state, 6);
    assert_eq!(at(&state, pos, -1), air);
    assert_eq!(at(&state, pos, 1), state.registry.states.water_levels[1]);
}
fn item(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
}
fn near(state: &mut TickState, player: EntityId, pos: BlockPos) {
    let slot = state.entities.slot_of(player).unwrap();
    state
        .entities
        .set_pos(slot, [pos.x as f64 + 0.5, pos.y as f64, pos.z as f64 + 2.5]);
}
fn click(pos: BlockPos, sequence: i32) -> PlayerInput {
    PlayerInput::UseItemOn {
        connection: ConnectionId(1),
        position: pos,
        hand: Hand::Main,
        face: 1,
        cursor: [0.5; 3],
        sequence,
    }
}
#[test]
fn waterlogged_bucket_fill_drain_and_break_preserve_one_source_and_one_drop() {
    let (mut state, player, pos) = setup();
    near(&mut state, player, pos);
    let dry = named(
        &state,
        "oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let wet = named(
        &state,
        "oak_slab",
        &[("type", "bottom"), ("waterlogged", "true")],
    );
    let water_bucket = item(&state, "water_bucket");
    let bucket = item(&state, "bucket");
    place(&mut state, pos, dry);
    state.players.get_mut(&player).unwrap().inventory.slots[0] =
        mc_data::ItemStack::new(water_bucket, 1);
    let (out, _, _) = run_tick(&mut state, &[click(pos, 1)]);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(wet));
    assert_eq!(state.players[&player].inventory.slots[0].item_id, bucket);
    assert!(
        out.changes.entities_spawned.is_empty(),
        "waterlogging must not drop its block"
    );
    run_tick(&mut state, &[click(pos, 2)]);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(dry));
    assert_eq!(
        state.players[&player].inventory.slots[0].item_id,
        water_bucket
    );
    run_tick(&mut state, &[click(pos, 3)]);
    crate::tick::mining_tests::ready_to_finish(&mut state, ConnectionId(1), pos);
    let (out, _, _) = run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: pos,
            action: crate::boundary::DigAction::Finish,
            sequence: 4,
        }],
    );
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(state.registry.states.water)
    );
    let drops: Vec<_> = out
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .collect();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].item_id, item(&state, "oak_slab"));
}
#[test]
fn source_ray_respects_torch_outline_and_rejects_existing_source_pour() {
    let (mut state, _, pos) = setup();
    let torch = named(&state, "torch", &[]);
    let water = state.registry.states.water;
    let target = BlockPos {
        z: pos.z + 1,
        ..pos
    };
    place(&mut state, pos, torch);
    place(&mut state, target, water);
    let eye = [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 - 1.5];
    assert_eq!(
        crate::fluids::bucket_target(&state, eye, 0.0, 0.0, None),
        None
    );
    let eye = [pos.x as f64 + 0.2, eye[1], eye[2]];
    assert_eq!(
        crate::fluids::bucket_target(&state, eye, 0.0, 0.0, None),
        Some((target, state.registry.states.air))
    );
    let held = mc_data::ItemStack::new(item(&state, "water_bucket"), 1);
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    let stone = named(&state, "stone", &[]);
    let wall = BlockPos {
        z: pos.z + 2,
        ..pos
    };
    place(&mut state, wall, stone);
    assert!(
        crate::fluids::use_on(
            &mut state,
            player,
            Hand::Main,
            wall,
            crate::placement::Face::North,
            &held
        )
        .is_none()
    );
}
#[test]
fn cauldron_partial_water_refuses_pickup_and_full_roundtrip_has_no_block_loot() {
    let (mut state, player, pos) = setup();
    near(&mut state, player, pos);
    let partial = named(&state, "water_cauldron", &[("level", "2")]);
    let bucket = item(&state, "bucket");
    place(&mut state, pos, partial);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(bucket, 1);
    run_tick(&mut state, &[click(pos, 1)]);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(partial));
    let full = named(&state, "water_cauldron", &[("level", "3")]);
    place(&mut state, pos, full);
    let (out, _, _) = run_tick(&mut state, &[click(pos, 2)]);
    assert!(out.changes.entities_spawned.is_empty());
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(named(&state, "cauldron", &[]))
    );
    assert_eq!(
        state.players[&player].inventory.slots[0].item_id,
        item(&state, "water_bucket")
    );
    run_tick(&mut state, &[click(pos, 3)]);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(full));
}
#[test]
fn fluid_deadlines_and_empty_tombstone_survive_wal_and_manifest_restart() {
    let (mut state, _, pos) = setup();
    let water = state.registry.states.water;
    place(&mut state, pos, water);
    for (dx, id) in [
        (2, "minecraft:flowing_water"),
        (4, "minecraft:flowing_lava"),
    ] {
        assert!(state.fluid_ticks.schedule(
            BlockPos {
                x: pos.x + dx,
                ..pos
            },
            &mc_data::Identifier::parse(id).unwrap(),
            state.tick + 100,
            -1,
        ));
    }
    let image = state.fluid_ticks.image();
    assert!(
        image
            .due
            .iter()
            .any(|tick| tick.pos == [pos.x, pos.y, pos.z])
    );
    for id in ["minecraft:flowing_water", "minecraft:flowing_lava"] {
        assert!(image.due.iter().any(|tick| tick.fluid.as_str() == id));
    }
    let dir = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
    drop(crate::native_checkpoint::tests::fixture_store(&config).unwrap());
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    let mut entries = Vec::new();
    state.fluid_ticks.journal(&mut entries);
    writer.append(state.tick, state.tick, &entries).unwrap();
    writer.flush().unwrap();
    let recovered = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&dir.path().join("journal"), 0).unwrap(),
    );
    assert_eq!(recovered.fluid_ticks, image);
    write_manifest(
        &config,
        &CheckpointManifest {
            tick: state.tick,
            fluid_ticks: recovered.fluid_ticks,
            ..read_manifest(&config).unwrap()
        },
    )
    .unwrap();
    state.fluid_ticks =
        crate::fluids::FluidTicks::from_image(read_manifest(&config).unwrap().fluid_ticks, None);
    assert_eq!(state.fluid_ticks.image(), image);
    state.tick += 5;
    crate::fluids::run(&mut state, &mut Vec::new(), &mut Vec::new());
    assert_ne!(
        state.world.block_at(pos.x + 1, pos.y, pos.z),
        Some(state.registry.states.air)
    );
    let cleared = crate::fluids::FluidTicksImage {
        due: Vec::new(),
        covered: image.covered.clone(),
        next_sequence: image.next_sequence,
    };
    let recovered = replay_batches(
        Replay {
            fluid_ticks: image,
            ..Default::default()
        },
        &[(
            state.tick,
            state.tick,
            vec![JournalEntry::FluidTicks {
                image: cleared.clone(),
            }],
        )],
    );
    assert_eq!(recovered.fluid_ticks, cleared);
}
#[test]
fn placed_sign_editor_enforces_owner_range_and_journals_chunk_nbt() {
    let (mut state, player, pos) = setup();
    near(&mut state, player, pos);
    let sign = item(&state, "oak_sign");
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(sign, 1);
    let floor = BlockPos {
        y: pos.y - 1,
        ..pos
    };
    let (_, outbox, _) = run_tick(&mut state, &[click(floor, 1)]);
    assert!(outbox.iter().flat_map(|(_, out)| out).any(
        |change| matches!(change,PlayerChange::SignEditorOpened{position,..} if *position==pos)
    ));
    let lines = vec!["one".into(), "two".into(), "three".into(), "four".into()];
    run_tick(
        &mut state,
        &[PlayerInput::SignUpdate {
            connection: ConnectionId(2),
            position: pos,
            front: true,
            lines: lines.clone(),
        }],
    );
    assert!(state.sign_editors.contains_key(&ConnectionId(1)));
    let (out, _, _) = run_tick(
        &mut state,
        &[PlayerInput::SignUpdate {
            connection: ConnectionId(1),
            position: pos,
            front: true,
            lines,
        }],
    );
    assert!(state.sign_editors.is_empty());
    let entries = crate::core::journal_entries_for_tick(&mut state, &out);
    let dir = tempfile::tempdir().unwrap();
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    writer.append(1, out.tick, &entries).unwrap();
    writer.flush().unwrap();
    let replay = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&dir.path().join("journal"), 0).unwrap(),
    );
    let bytes = replay.opaque_block_entities[&(pos.x, pos.y, pos.z)]
        .as_ref()
        .unwrap();
    assert_eq!(
        state
            .world
            .chunk(crate::boundary::ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16)
            })
            .unwrap()
            .block_entities
            .get(&pos),
        Some(bytes)
    );
    let parsed = mc_nbt::read_network(&mut bytes.as_slice()).unwrap();
    assert!(
        matches!(&parsed,mc_nbt::Tag::Compound(fields) if fields.iter().any(|(key,_)|key=="front_text"))
    );
    state.sign_editors.insert(
        ConnectionId(1),
        crate::signs::Editor {
            position: pos,
            block: state.world.block_at(pos.x, pos.y, pos.z).unwrap(),
            opened_tick: state.tick,
            front: true,
        },
    );
    let slot = state.entities.slot_of(player).unwrap();
    state
        .entities
        .set_pos(slot, [pos.x as f64 + 100.0, pos.y as f64, pos.z as f64]);
    crate::signs::update(
        &mut state,
        ConnectionId(1),
        pos,
        true,
        &vec!["bad".into(); 4],
    );
    assert!(state.sign_editors.is_empty());
    assert!(
        state.opaque_changes.is_empty(),
        "out-of-range write must not persist"
    );
}
#[test]
fn later_same_state_replacement_revokes_placed_sign_authority() {
    let (mut state, _, pos) = setup();
    let sign = named(&state, "oak_sign", &[]);
    state.sign_editors.insert(
        ConnectionId(1),
        crate::signs::Editor {
            position: pos,
            block: sign,
            opened_tick: state.tick,
            front: true,
        },
    );
    state.tick += 1;
    crate::signs::invalidate(&mut state, &[(pos, sign)]);
    assert!(state.sign_editors.is_empty());
}

#[test]
fn unsupported_attachment_and_occupied_second_cell_refuse_without_debit() {
    let (mut state, player, pos) = setup();
    near(&mut state, player, pos);
    let torch = item(&state, "torch");
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(torch, 2);
    let unsupported = BlockPos {
        y: pos.y + 3,
        ..pos
    };
    run_tick(&mut state, &[click(unsupported, 1)]);
    assert_eq!(
        state
            .world
            .block_at(unsupported.x, unsupported.y, unsupported.z),
        Some(state.registry.states.air)
    );
    assert_eq!(state.players[&player].inventory.slots[0].count, 2);
    let door = item(&state, "oak_door");
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(door, 2);
    let upper = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    let stone = state.registry.states.stone;
    place(&mut state, upper, stone);
    run_tick(
        &mut state,
        &[click(
            BlockPos {
                y: pos.y - 1,
                ..pos
            },
            2,
        )],
    );
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(state.registry.states.air)
    );
    assert_eq!(state.players[&player].inventory.slots[0].count, 2);
    assert_eq!(state.world.block_at(upper.x, upper.y, upper.z), Some(stone));
}

#[test]
fn double_plant_replaces_clicked_vegetation_and_upper_break_drops_one_lower_loot() {
    let (mut state, player, pos) = setup();
    near(&mut state, player, pos);
    let dirt = state.registry.states.dirt;
    place(
        &mut state,
        BlockPos {
            y: pos.y - 1,
            ..pos
        },
        dirt,
    );
    let grass = named(&state, "short_grass", &[]);
    place(&mut state, pos, grass);
    let flower = item(&state, "sunflower");
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(flower, 2);
    run_tick(&mut state, &[click(pos, 1)]);
    let lower = named(&state, "sunflower", &[("half", "lower")]);
    let upper = named(&state, "sunflower", &[("half", "upper")]);
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(lower));
    assert_eq!(state.world.block_at(pos.x, pos.y + 1, pos.z), Some(upper));
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
    crate::tick::mining_tests::ready_to_finish(
        &mut state,
        ConnectionId(1),
        BlockPos {
            y: pos.y + 1,
            ..pos
        },
    );
    let (out, _, _) = run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: BlockPos {
                y: pos.y + 1,
                ..pos
            },
            action: crate::boundary::DigAction::Finish,
            sequence: 2,
        }],
    );
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(state.registry.states.air)
    );
    assert_eq!(
        state.world.block_at(pos.x, pos.y + 1, pos.z),
        Some(state.registry.states.air)
    );
    let drops: Vec<_> = out
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .collect();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].item_id, flower);
    assert_eq!(drops[0].count, 1);
}

#[test]
fn supported_sign_popping_tombstones_old_text_in_same_block_tick() {
    let (mut state, _, pos) = setup();
    let sign = named(&state, "oak_sign", &[]);
    place(&mut state, pos, sign);
    let mut bytes = Vec::new();
    mc_nbt::write_network(
        &mut bytes,
        &crate::signs::text_nbt(&vec!["retained".into(); 4]),
    )
    .unwrap();
    state.world.set_opaque_block_entity(pos, Some(bytes));
    let air = state.registry.states.air;
    state.world.set_block(pos.x, pos.y - 1, pos.z, air);
    let mut writes = vec![(
        BlockPos {
            y: pos.y - 1,
            ..pos
        },
        air,
    )];
    crate::blocks::run(&mut state, &mut writes, &mut Vec::new(), &mut Vec::new());
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(air));
    assert!(state.world.take_block_entity_removals().contains(&pos));
}

#[test]
fn native_fluid_checkpoint_retains_identity_priority_and_absolute_deadline() {
    let (mut state, _, pos) = setup();
    let water = state.registry.states.water;
    place(&mut state, pos, water);
    state.fluid_ticks = crate::fluids::FluidTicks::default();
    let ids = [
        "minecraft:water",
        "minecraft:flowing_water",
        "minecraft:lava",
        "minecraft:flowing_lava",
    ];
    for id in ids {
        let id = mc_data::Identifier::parse(id).unwrap();
        assert!(state.fluid_ticks.schedule(pos, &id, 100, -2));
        assert!(!state.fluid_ticks.schedule(pos, &id, 100, -2));
    }
    let before = state.fluid_ticks.image();
    before.validate().unwrap();
    let absent = state.fluid_ticks.checkpoint(&mut state.world, 40);
    assert!(absent.is_empty());
    let chunk = state
        .world
        .chunk(crate::boundary::ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .unwrap();
    assert_eq!(chunk.scheduled_fluid_ticks().len(), ids.len());
    for ((tick, image), id) in chunk
        .scheduled_fluid_ticks()
        .iter()
        .zip(&before.due)
        .zip(ids)
    {
        assert_eq!(tick.fluid.as_str(), id);
        assert_eq!(tick.priority, -2);
        assert_eq!(tick.trigger_tick, 60);
        assert_eq!(tick.sequence(), image.sequence);
    }
    let mut hydrated = crate::fluids::FluidTicks::default();
    hydrated.hydrate(chunk, 40);
    assert_eq!(hydrated.image(), before);
    let restored = crate::fluids::FluidTicks::from_image(before.clone(), None);
    assert_eq!(restored.image(), before);
}

#[test]
fn runtime_fluid_scheduling_uses_exact_source_flowing_and_waterlogged_ids() {
    let (mut state, _, pos) = setup();
    let states = state.registry.states;
    let wet = named(
        &state,
        "oak_slab",
        &[("type", "bottom"), ("waterlogged", "true")],
    );
    for (block, id, delay) in [
        (states.water_levels[0], "minecraft:water", 5),
        (states.water_levels[1], "minecraft:flowing_water", 5),
        (states.water_levels[8], "minecraft:flowing_water", 5),
        (states.lava_levels[0], "minecraft:lava", 30),
        (states.lava_levels[1], "minecraft:flowing_lava", 30),
        (states.lava_levels[8], "minecraft:flowing_lava", 30),
        (wet, "minecraft:water", 5),
    ] {
        state.fluid_ticks = crate::fluids::FluidTicks::default();
        place(&mut state, pos, block);
        let image = state.fluid_ticks.image();
        image.validate().unwrap();
        assert_eq!(image.due.len(), 1, "{id}");
        assert_eq!(image.due[0].fluid.as_str(), id);
        assert_eq!(image.due[0].deadline, state.tick + delay);
        assert_eq!(image.due[0].pos, [pos.x, pos.y, pos.z]);
        assert!(!state.fluid_ticks.schedule(
            pos,
            &mc_data::Identifier::parse("minecraft:milk").unwrap(),
            state.tick,
            0,
        ));
        assert_eq!(state.fluid_ticks.image(), image);
    }
}

#[test]
fn runtime_fluid_ticks_only_consume_matching_source_or_flowing_state() {
    for (family, level) in [
        ("water", 0),
        ("water", 1),
        ("lava", 0),
        ("lava", 1),
        ("waterlogged", 0),
    ] {
        let (mut state, _, pos) = setup();
        let block = match family {
            "water" => state.registry.states.water_levels[level],
            "lava" => state.registry.states.lava_levels[level],
            "waterlogged" => named(
                &state,
                "oak_slab",
                &[("type", "bottom"), ("waterlogged", "true")],
            ),
            _ => unreachable!(),
        };
        place(&mut state, pos, block);
        state.fluid_ticks = crate::fluids::FluidTicks::default();
        let family = if family == "waterlogged" {
            "water"
        } else {
            family
        };
        let source = mc_data::Identifier::parse(format!("minecraft:{family}")).unwrap();
        let flowing = mc_data::Identifier::parse(format!("minecraft:flowing_{family}")).unwrap();
        let (exact, mismatched) = if level == 0 {
            (&source, &flowing)
        } else {
            (&flowing, &source)
        };
        assert!(state.fluid_ticks.schedule(pos, mismatched, state.tick, 0));
        let mut writes = Vec::new();
        let mut spawned = Vec::new();
        crate::fluids::run(&mut state, &mut writes, &mut spawned);
        assert!(writes.is_empty(), "{mismatched} must not run on {exact}");
        assert!(spawned.is_empty());
        assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(block));
        assert!(state.fluid_ticks.image().due.is_empty());
        assert!(state.fluid_ticks.schedule(pos, exact, state.tick, 0));
        crate::fluids::run(&mut state, &mut writes, &mut spawned);
        assert!(!writes.is_empty(), "{exact} must run on its matching state");
        assert!(state.fluid_ticks.image().due.is_empty());
    }
}

#[test]
fn stable_lava_with_fuel_requeues_but_isolated_source_does_not_poll_forever() {
    let (mut state, _, pos) = setup();
    let lava = state.registry.states.lava_levels[0];
    let log = named(&state, "oak_log", &[]);
    place(
        &mut state,
        BlockPos {
            x: pos.x + 1,
            y: pos.y + 1,
            ..pos
        },
        log,
    );
    place(&mut state, pos, lava);
    state.tick += 30;
    crate::fluids::run(&mut state, &mut Vec::new(), &mut Vec::new());
    assert!(
        state
            .fluid_ticks
            .image()
            .due
            .iter()
            .any(|tick| tick.pos == [pos.x, pos.y, pos.z] && tick.deadline > state.tick)
    );
    let (mut isolated, _, isolated_pos) = setup();
    place(&mut isolated, isolated_pos, lava);
    isolated.tick += 30;
    crate::fluids::run(&mut isolated, &mut Vec::new(), &mut Vec::new());
    assert!(
        !isolated
            .fluid_ticks
            .image()
            .due
            .iter()
            .any(|tick| tick.pos == [isolated_pos.x, isolated_pos.y, isolated_pos.z])
    );
}

#[test]
fn absent_chunk_checkpoint_composes_block_and_fluid_native_queues_without_admission() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
    let make_world = || {
        let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
        ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&config).unwrap(),
            0,
            Arc::new(terrain),
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [0, 200, 0],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    let cpos = crate::boundary::ChunkPos { x: 0, z: 0 };
    let want = [(cpos, 0)].into_iter().collect();
    let mut world = make_world();
    world.ensure_blocking(&want);
    let mut fluids = crate::fluids::FluidTicks::default();
    fluids.schedule(
        BlockPos { x: 1, y: 200, z: 1 },
        &mc_data::Identifier::parse("minecraft:water").unwrap(),
        120,
        -1,
    );
    for (x, id) in [
        (3, "minecraft:flowing_water"),
        (4, "minecraft:flowing_lava"),
    ] {
        assert!(fluids.schedule(
            BlockPos { x, y: 200, z: 1 },
            &mc_data::Identifier::parse(id).unwrap(),
            120,
            -1,
        ));
    }
    let mut blocks = crate::block_ticks::BlockTicks::default();
    blocks.schedule(mc_world::ScheduledBlockTick::new(
        BlockPos { x: 2, y: 200, z: 1 },
        mc_data::Identifier::parse("minecraft:stone").unwrap(),
        130,
        2,
    ));
    world.evict_outside(&Default::default());
    assert!(!world.has_chunk(0, 0));
    let absent_blocks = blocks.checkpoint(&mut world, 100);
    let absent_fluids = fluids.checkpoint(&mut world, 100);
    let mut capture = world.capture_dirty(&Default::default());
    world.capture_scheduled_block_ticks(&mut capture, absent_blocks);
    world.capture_scheduled_fluid_ticks(&mut capture, absent_fluids);
    assert!(
        !world.has_chunk(0, 0),
        "checkpoint must not activate overdue work"
    );
    crate::world::world_tests::write_capture(&capture).unwrap();
    drop(capture);
    drop(world);
    write_manifest(
        &config,
        &CheckpointManifest {
            tick: 100,
            fluid_ticks: fluids.image(),
            block_ticks: blocks.image(),
            ..read_manifest(&config).unwrap()
        },
    )
    .unwrap();
    let manifest = read_manifest(&config).unwrap();
    let mut restarted = make_world();
    restarted.ensure_blocking(&want);
    let loaded = restarted.chunk(cpos).unwrap();
    assert_eq!(loaded.scheduled_fluid_ticks().len(), 3);
    for (tick, id) in loaded.scheduled_fluid_ticks().iter().zip([
        "minecraft:water",
        "minecraft:flowing_water",
        "minecraft:flowing_lava",
    ]) {
        assert_eq!(tick.fluid.as_str(), id);
        assert_eq!(tick.trigger_tick, 20);
        assert_eq!(tick.priority, -1);
    }
    assert_eq!(loaded.scheduled_block_ticks().len(), 1);
    assert_eq!(loaded.scheduled_block_ticks()[0].trigger_tick, 30);
    let mut restored = crate::fluids::FluidTicks::from_image(manifest.fluid_ticks, None);
    restored.hydrate(loaded, 100);
    assert_eq!(restored.image(), fluids.image());
    assert_eq!(
        restored.image().due[0].deadline,
        120,
        "no rebasing the manifest twice"
    );
}

#[test]
fn ceiling_and_wall_hanging_signs_open_editor_publish_type_eight_and_keep_text_when_waterlogged() {
    for wall in [false, true] {
        let (mut state, player, pos) = setup();
        near(&mut state, player, pos);
        let support = if wall {
            BlockPos {
                x: pos.x - 1,
                ..pos
            }
        } else {
            BlockPos {
                y: pos.y + 1,
                ..pos
            }
        };
        let stone = state.registry.states.stone;
        place(&mut state, support, stone);
        let hanging = item(&state, "oak_hanging_sign");
        state.players.get_mut(&player).unwrap().inventory.slots[0] =
            mc_data::ItemStack::new(hanging, 2);
        let (_, outbox, _) = run_tick(
            &mut state,
            &[PlayerInput::UseItemOn {
                connection: ConnectionId(1),
                position: support,
                hand: Hand::Main,
                face: if wall { 5 } else { 0 },
                cursor: [0.5; 3],
                sequence: 1,
            }],
        );
        assert!(outbox.iter().flat_map(|(_, changes)| changes).any(|change| matches!(change, PlayerChange::SignEditorOpened { position, .. } if *position == pos)));
        let current = state.world.block_at(pos.x, pos.y, pos.z).unwrap();
        let block = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(current))
            .unwrap();
        assert_eq!(
            block.block.id.path(),
            if wall {
                "oak_wall_hanging_sign"
            } else {
                "oak_hanging_sign"
            }
        );
        assert_eq!(state.players[&player].inventory.slots[0].count, 1);
        let (out, outbox, _) = run_tick(
            &mut state,
            &[PlayerInput::SignUpdate {
                connection: ConnectionId(1),
                position: pos,
                front: true,
                lines: vec!["hanging text".into(); 4],
            }],
        );
        assert!(outbox.iter().flat_map(|(_, changes)| changes).any(|change| matches!(change, PlayerChange::BlockEntityData { block_entity_type: 8, position, .. } if *position == pos)));
        let entries = crate::core::journal_entries_for_tick(&mut state, &out);
        let bytes = entries
            .iter()
            .find_map(|entry| match entry {
                JournalEntry::OpaqueBlockEntity {
                    pos: at,
                    nbt: Some(bytes),
                } if *at == [pos.x, pos.y, pos.z] => Some(bytes.clone()),
                _ => None,
            })
            .unwrap();
        let parsed = mc_nbt::read_network(&mut bytes.as_slice()).unwrap();
        assert!(
            matches!(parsed, mc_nbt::Tag::Compound(fields) if fields.iter().any(|(key, value)| key == "id" && *value == mc_nbt::Tag::String("minecraft:hanging_sign".into())))
        );
        state.players.get_mut(&player).unwrap().inventory.slots[0] =
            mc_data::ItemStack::new(item(&state, "water_bucket"), 1);
        run_tick(&mut state, &[click(pos, 2)]);
        let wet = state.world.block_at(pos.x, pos.y, pos.z).unwrap();
        assert_ne!(wet, current);
        let block = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(wet))
            .unwrap();
        assert!(
            block
                .properties
                .iter()
                .any(|(key, value)| key == "waterlogged" && value == "true")
        );
        let chunk = state
            .world
            .chunk(crate::boundary::ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .unwrap();
        assert_eq!(chunk.block_entities.get(&pos), Some(&bytes));
        run_tick(&mut state, &[click(pos, 3)]);
        let chunk = state
            .world
            .chunk(crate::boundary::ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .unwrap();
        assert_eq!(
            chunk.block_entities.get(&pos),
            Some(&bytes),
            "draining also preserves sign NBT"
        );
    }
}

#[test]
fn accepted_same_state_sign_replacement_clears_old_opaque_text() {
    for (name, item_name, face) in [
        ("oak_sign", "oak_sign", 1),
        ("oak_hanging_sign", "oak_hanging_sign", 0),
        ("oak_wall_hanging_sign", "oak_hanging_sign", 5),
    ] {
        let (mut state, player, pos) = setup();
        near(&mut state, player, pos);
        let support = match face {
            0 => BlockPos {
                y: pos.y + 1,
                ..pos
            },
            5 => BlockPos {
                x: pos.x - 1,
                ..pos
            },
            _ => BlockPos {
                y: pos.y - 1,
                ..pos
            },
        };
        let stone = state.registry.states.stone;
        place(&mut state, support, stone);
        let sign = named(
            &state,
            name,
            if face == 5 {
                &[("facing", "north")]
            } else {
                &[("rotation", "8")]
            },
        );
        place(&mut state, pos, sign);
        let mut bytes = Vec::new();
        mc_nbt::write_network(
            &mut bytes,
            &crate::signs::text_nbt(&vec!["old text".into(); 4]),
        )
        .unwrap();
        state
            .world
            .set_opaque_block_entity(pos, Some(bytes.clone()));
        // A pending break exposes air to placement, while the resident base
        // still owns the old sign and NBT. Break/re-place ends at the SAME id.
        state
            .world
            .set_block(pos.x, pos.y, pos.z, state.registry.states.air);
        let held = item(&state, item_name);
        state.players.get_mut(&player).unwrap().inventory.slots[0] =
            mc_data::ItemStack::new(held, 1);
        let (out, outbox, _) = run_tick(
            &mut state,
            &[PlayerInput::UseItemOn {
                connection: ConnectionId(1),
                position: support,
                hand: Hand::Main,
                face,
                cursor: [0.5; 3],
                sequence: 1,
            }],
        );
        assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(sign));
        assert!(outbox.iter().flat_map(|(_, changes)| changes).any(|change| matches!(change, PlayerChange::SignEditorOpened { position, .. } if *position == pos)));
        let chunk = state
            .world
            .chunk(crate::boundary::ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .unwrap();
        let replacement = chunk.block_entities[&pos].clone();
        let fields = mc_nbt::read_network(&mut replacement.as_slice()).unwrap();
        let kind = mc_data::block_entity_kinds::BlockEntityKind::for_block(name).unwrap();
        assert_eq!(
            fields,
            mc_nbt::Tag::Compound(vec![
                ("id".into(), mc_nbt::Tag::String(kind.vanilla_id().into())),
                ("x".into(), mc_nbt::Tag::Int(pos.x)),
                ("y".into(), mc_nbt::Tag::Int(pos.y)),
                ("z".into(), mc_nbt::Tag::Int(pos.z)),
            ]),
            "fresh default owner contains no old sign text"
        );
        let entries = crate::core::journal_entries_for_tick(&mut state, &out);
        let tombstone = entries
            .iter()
            .position(|entry| {
                matches!(entry,
                    JournalEntry::RemoveBlockEntity { pos: at, .. } if *at == [pos.x, pos.y, pos.z]
                )
            })
            .unwrap();
        let post_image = entries
            .iter()
            .position(|entry| {
                matches!(entry,
                    JournalEntry::OpaqueBlockEntity { pos: at, nbt: Some(nbt) }
                        if *at == [pos.x, pos.y, pos.z] && nbt == &replacement
                )
            })
            .unwrap();
        assert!(
            tombstone < post_image,
            "old owner removal precedes new custody"
        );
        let mut old = Replay::default();
        old.opaque_block_entities
            .insert((pos.x, pos.y, pos.z), Some(bytes));
        let replayed = replay_batches(old, &[(state.tick, state.tick, entries)]);
        assert_eq!(
            replayed.opaque_block_entities[&(pos.x, pos.y, pos.z)],
            Some(replacement)
        );
    }
}

#[test]
fn wall_hanging_sign_survives_either_bar_end_then_pops_after_both_end_updates() {
    let (mut state, _, pos) = setup();
    let sign = named(&state, "oak_wall_hanging_sign", &[("facing", "north")]);
    let west = BlockPos {
        x: pos.x - 1,
        ..pos
    };
    let east = BlockPos {
        x: pos.x + 1,
        ..pos
    };
    let stone = state.registry.states.stone;
    place(&mut state, west, stone);
    place(&mut state, east, stone);
    place(&mut state, pos, sign);
    let air = state.registry.states.air;
    state.world.set_block(west.x, west.y, west.z, air);
    let mut writes = vec![(west, air)];
    crate::blocks::run(&mut state, &mut writes, &mut Vec::new(), &mut Vec::new());
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(sign));
    state.world.fold_overlay();
    state.world.set_block(east.x, east.y, east.z, air);
    let mut writes = vec![(east, air)];
    let mut spawned = Vec::new();
    crate::blocks::run(&mut state, &mut writes, &mut spawned, &mut Vec::new());
    assert_eq!(state.world.block_at(pos.x, pos.y, pos.z), Some(air));
    let drops: Vec<_> = spawned
        .iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .collect();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].item_id, item(&state, "oak_hanging_sign"));
}

#[test]
fn bucket_previous_cell_waterlogs_before_source_fallback_and_lava_cannot_waterlog() {
    let (mut state, _, pos) = setup();
    let dry = named(&state, "oak_hanging_sign", &[("waterlogged", "false")]);
    let wet = named(&state, "oak_hanging_sign", &[("waterlogged", "true")]);
    place(&mut state, pos, dry);
    assert_eq!(
        crate::fluids::bucket_previous(&state, pos, crate::fluids::Fluid::Water),
        Some((pos, wet))
    );
    assert_eq!(
        crate::fluids::bucket_previous(&state, pos, crate::fluids::Fluid::Lava),
        None
    );
    let air = state.registry.states.air;
    place(&mut state, pos, air);
    assert_eq!(
        crate::fluids::bucket_previous(&state, pos, crate::fluids::Fluid::Water),
        Some((pos, state.registry.states.water))
    );
    let source = state.registry.states.water;
    place(&mut state, pos, source);
    assert_eq!(
        crate::fluids::bucket_previous(&state, pos, crate::fluids::Fluid::Water),
        None,
        "existing sources refuse duplicate pour"
    );
    let flowing = state.registry.states.water_levels[1];
    place(&mut state, pos, flowing);
    assert_eq!(
        crate::fluids::bucket_previous(&state, pos, crate::fluids::Fluid::Water),
        Some((pos, source))
    );
}

#[test]
fn fluid_coverage_waits_for_captured_empty_flush_ack_and_retains_new_pending_work() {
    let (state, _, pos) = setup();
    let chunk = [pos.x.div_euclid(16), pos.z.div_euclid(16)];
    let mut queue = crate::fluids::FluidTicks::default();
    let water = mc_data::Identifier::parse("minecraft:water").unwrap();
    queue.schedule(pos, &water, 10, 0);
    let nonempty = queue.image();
    queue.drain(10, &state.world);
    let empty = queue.image();
    queue.evict_flushed(|_, _| true);
    assert!(queue.image().covered.contains(&chunk));
    let mut entries = Vec::new();
    queue.journal(&mut entries);
    queue.evict_flushed(|_, _| true);
    assert!(
        queue.image().covered.contains(&chunk),
        "WAL clear cannot evict a tombstone"
    );
    queue.acknowledge_flushed(&nonempty);
    queue.evict_flushed(|_, _| true);
    assert!(
        queue.image().covered.contains(&chunk),
        "old nonempty checkpoint remains insufficient"
    );
    queue.acknowledge_flushed(&empty);
    queue.evict_flushed(|_, _| false);
    assert!(
        queue.image().covered.contains(&chunk),
        "loaded/dirty columns remain authoritative"
    );
    queue.evict_flushed(|_, _| true);
    assert!(!queue.image().covered.contains(&chunk));
    queue.journal(&mut entries);
    assert!(
        matches!(entries.last(), Some(JournalEntry::FluidTicks { image }) if !image.covered.contains(&chunk))
    );
    queue.schedule(pos, &water, 20, 0);
    queue.acknowledge_flushed(&empty);
    queue.journal(&mut entries);
    queue.evict_flushed(|_, _| true);
    assert_eq!(queue.image().due.len(), 1);
    assert!(queue.image().covered.contains(&chunk));
}

fn drainable(state: &TickState, pos: BlockPos) -> bool {
    state
        .world
        .is_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
        && state.world.block_at(pos.x, pos.y, pos.z).is_some()
}

fn fluid_ids(names: &[&str]) -> Vec<mc_data::Identifier> {
    names
        .iter()
        .map(|name| mc_data::Identifier::parse(*name).unwrap())
        .collect()
}

fn shuffled(image: &crate::fluids::FluidTicksImage) -> crate::fluids::FluidTicksImage {
    let mut due = image.due.clone();
    // `validate` accepts any unique order, so a durable image may legitimately
    // arrive with rows and coverage reversed.
    due.reverse();
    let mut covered = image.covered.clone();
    covered.reverse();
    crate::fluids::FluidTicksImage {
        due,
        covered,
        next_sequence: image.next_sequence,
    }
}

/// Check the backlog without touching resident queue state: flush fixture
/// edits, evict the chunks, then count the pure absent payload that
/// `checkpoint` projects for the dirty chunks.
fn projected(queue: &crate::fluids::FluidTicks, state: &mut TickState, now: u64) -> usize {
    // `setup` leaves block, debt and inhabited pins behind, and pinned chunks
    // survive eviction. Fold the inhabited pins and fence the storage
    // revisions first (this fixture world is memory-only, so the capture
    // itself writes nothing), then evict and count the pure absent payload.
    // Activity membership is left alone: residency never gates it here, and
    // the next drain still needs the active set `readmit` restores.
    let _ = state.world.capture_dirty(&Default::default());
    let flushed: Vec<(crate::boundary::ChunkPos, u64)> = state
        .world
        .resident_positions()
        .into_iter()
        .map(|pos| (pos, state.world.storage_revision(pos)))
        .collect();
    state.world.mark_flushed(&flushed);
    state.world.evict_outside(&Default::default());
    queue.checkpoint(&mut state.world, now).len()
}
/// Re-admit the evicted fixture chunks for the next drain/schedule step.
fn readmit(state: &mut TickState) {
    let tick = state.tick;
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state.refresh_active_chunks(tick);
}

/// Durable (even unordered) empty and nonempty coverage starts
/// checkpoint-clean and projects nothing; unknown durability stays pending.
#[test]
fn fluid_checkpoint_delta_orders_and_base_images() {
    let (mut state, _, pos) = setup();
    let chunk = [pos.x.div_euclid(16), pos.z.div_euclid(16)];
    let water = mc_data::Identifier::parse("minecraft:water").unwrap();
    let mut source = crate::fluids::FluidTicks::default();
    assert!(source.schedule(pos, &water, 10, 0));
    assert!(source.schedule(
        BlockPos {
            y: pos.y + 1,
            ..pos
        },
        &water,
        10,
        1
    ));
    assert!(drainable(&state, pos));
    let nonempty = source.image();
    nonempty.validate().unwrap();
    // Same durable content in canonical or reversed order starts clean.
    let reversed = shuffled(&nonempty);
    reversed.validate().unwrap();
    for base in [&nonempty, &reversed] {
        let queue = crate::fluids::FluidTicks::from_image(nonempty.clone(), Some(base));
        assert_eq!(queue.image(), nonempty);
        assert_eq!(
            projected(&queue, &mut state, 5),
            0,
            "durable nonempty coverage must not project"
        );
        // Re-admit the evicted chunk for the next iteration.
        readmit(&mut state);
    }
    // Coverage itself need not be sorted in a valid durable image.
    let empty_pair = crate::fluids::FluidTicksImage {
        due: Vec::new(),
        covered: vec![[0, 0], [2, 3]],
        next_sequence: 0,
    };
    let reversed_empty = shuffled(&empty_pair);
    reversed_empty.validate().unwrap();
    let clean = crate::fluids::FluidTicks::from_image(empty_pair, Some(&reversed_empty));
    assert_eq!(projected(&clean, &mut state, 5), 0);
    readmit(&mut state);
    // Unknown durability stays pending and projects its chunk once.
    let mut pending = crate::fluids::FluidTicks::from_image(nonempty.clone(), None);
    assert_eq!(projected(&pending, &mut state, 5), 1);
    // A failed capture keeps the chunk pending; the matching ACK clears it.
    readmit(&mut state);
    assert_eq!(
        projected(&pending, &mut state, 5),
        1,
        "checkpoint must stay retry-safe until ACK"
    );
    pending.acknowledge_flushed(&nonempty);
    assert_eq!(projected(&pending, &mut state, 5), 0);
    // Consuming the queue marks its chunk even though the live queue is empty.
    readmit(&mut state);
    let mut consumed = crate::fluids::FluidTicks::from_image(nonempty.clone(), None);
    consumed.drain(10, &state.world);
    assert!(consumed.image().due.is_empty());
    consumed.acknowledge_flushed(&nonempty);
    assert_eq!(
        projected(&consumed, &mut state, 5),
        1,
        "consumed empty clears keep their projection pending across stale captures"
    );
    // The matching empty capture clears the backlog.
    let empty = consumed.image();
    empty.validate().unwrap();
    consumed.acknowledge_flushed(&empty);
    assert_eq!(projected(&consumed, &mut state, 5), 0);
    // Genuinely durable empty coverage starts clean and may compact away.
    let mut clean = crate::fluids::FluidTicks::from_image(empty.clone(), Some(&empty));
    assert_eq!(projected(&clean, &mut state, 5), 0);
    clean.acknowledge_flushed(&empty);
    clean.evict_flushed(|_, _| true);
    assert!(
        !clean.image().covered.contains(&chunk),
        "genuinely durable empty coverage may compact away"
    );
}

/// Changed/new queues project once, retries stay pending, stale captures
/// never clear newer state, and journal emission is never an ACK.
#[test]
fn fluid_checkpoint_delta_changed_new_retry_and_stale_ack() {
    let (mut state, _, pos) = setup();
    let chunk = [pos.x.div_euclid(16), pos.z.div_euclid(16)];
    let other = BlockPos {
        x: (chunk[0] + 1) * 16,
        y: pos.y,
        z: pos.z,
    };
    let ids = fluid_ids(&["minecraft:water", "minecraft:flowing_water"]);
    let mut base = crate::fluids::FluidTicks::default();
    assert!(base.schedule(pos, &ids[0], 10, 0));
    let durable = base.image();
    durable.validate().unwrap();
    let mut queue = crate::fluids::FluidTicks::from_image(durable.clone(), Some(&durable));
    assert_eq!(projected(&queue, &mut state, 5), 0);
    // Re-admit, then change one row and add a brand-new chunk.
    readmit(&mut state);
    assert!(drainable(&state, pos));
    queue.drain(10, &state.world);
    assert!(queue.schedule(pos, &ids[1], 11, 0));
    assert!(queue.schedule(other, &ids[0], 12, 0));
    let changed = queue.image();
    changed.validate().unwrap();
    assert_eq!(changed.due.len(), 2);
    // Journal emission carries WAL authority but must not clear the backlog.
    let mut entries = Vec::new();
    queue.journal(&mut entries);
    assert!(matches!(
        entries.last(),
        Some(JournalEntry::FluidTicks { .. })
    ));
    assert_eq!(
        projected(&queue, &mut state, 5),
        2,
        "changed and new queues project once"
    );
    // A failed capture keeps both chunks pending; the matching ACK clears them.
    readmit(&mut state);
    assert_eq!(
        projected(&queue, &mut state, 5),
        2,
        "checkpoint must stay retry-safe until ACK"
    );
    queue.acknowledge_flushed(&changed);
    assert_eq!(projected(&queue, &mut state, 5), 0);
    // A stale capture after newer schedule/consume clears nothing newer: the
    // rescheduled chunk stays pending while the already-consumed empty chunk
    // legitimately acknowledges.
    readmit(&mut state);
    assert!(queue.schedule(pos, &ids[0], 13, 0));
    queue.drain(13, &state.world);
    let stale = queue.image();
    assert!(queue.schedule(other, &ids[1], 14, 0));
    queue.acknowledge_flushed(&changed);
    queue.acknowledge_flushed(&stale);
    assert_eq!(
        projected(&queue, &mut state, 5),
        1,
        "stale ACK after newer schedule/consume must not clear the newer chunk"
    );
    // A bare sequence step is manifest authority, not a content change.
    let before_step = queue.image();
    let mut stepped = before_step.clone();
    stepped.next_sequence += 1;
    stepped.validate().unwrap();
    let mut resequenced = crate::fluids::FluidTicks::from_image(stepped, Some(&before_step));
    assert_eq!(
        projected(&resequenced, &mut state, 5),
        0,
        "next_sequence alone must not mark chunks"
    );
    readmit(&mut state);
    assert!(resequenced.schedule(pos, &ids[0], 15, 0));
    assert_eq!(
        projected(&resequenced, &mut state, 5),
        1,
        "new schedule after a clean base still projects"
    );
}
