#[path = "piston_tests.rs"]
mod piston_tests;

use super::*;
use crate::registry::CoreRegistry;
use crate::tick::{BlockEntityState, TickState};
use crate::world::ChunkWorld;
use std::sync::Arc;

struct Empty {
    air: BlockStateId,
}
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            self.air,
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}
fn state() -> TickState {
    state_in(None)
}
fn state_in(directory: Option<std::path::PathBuf>) -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let air = BlockStateId(registry.states.air);
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(Empty { air });
    let mut world = if let Some(directory) = directory {
        let config = crate::CoreConfig::new(directory, 1);
        ChunkWorld::new_native(
            1,
            crate::native_checkpoint::tests::fixture_store(&config).unwrap(),
            crate::native_checkpoint::dimension_id(crate::dimensions::Dimension::Overworld),
            generator,
            &registry,
            air,
            [0, 200, 0],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            1,
            generator,
            &registry,
            air,
            [0, 200, 0],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    world.set_opaque_block_entity(BlockPos { x: 0, y: 200, z: 0 }, None);
    let mut state = TickState::new(registry, world, 2, 512);
    state.redstone.loaded.insert(ChunkPos { x: 0, z: 0 });
    state
        .tickets
        .insert("redstone-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    state
}
fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
    let base = state.registry.blocks.block(&id).unwrap().default;
    let mut values = state
        .registry
        .blocks
        .by_id(base)
        .unwrap()
        .properties
        .clone();
    for (key, value) in props {
        values.iter_mut().find(|(k, _)| k == key).unwrap().1 = (*value).into();
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &values)
        .unwrap()
        .0
}
fn place(
    state: &mut TickState,
    x: i32,
    y: i32,
    z: i32,
    name: &str,
    props: &[(&str, &str)],
) -> BlockPos {
    let pos = BlockPos { x, y, z };
    let id = block(state, name, props);
    state.world.set_block(x, y, z, id);
    state.world.fold_overlay();
    pos
}
fn value<'a>(state: &'a TickState, pos: BlockPos, key: &str) -> &'a str {
    prop(at(state, pos).unwrap(), key).unwrap()
}
fn advance(state: &mut TickState, n: u64) {
    for _ in 0..n {
        let mut writes = Vec::new();
        let mut spawned = Vec::new();
        run(state, &mut writes, &mut spawned);
        state.world.fold_overlay();
        state.tick += 1;
    }
}
fn wake(state: &mut TickState, pos: BlockPos) {
    schedule_cell(state, pos, 0, 0);
}

#[test]
fn large_wire_network_obeys_one_tick_budget_and_eventually_sleeps() {
    let mut s = state();
    for x in 0..3 {
        for z in 0..3 {
            let chunk = ChunkPos { x, z };
            s.world.set_opaque_block_entity(
                BlockPos {
                    x: x * 16,
                    y: 200,
                    z: z * 16,
                },
                None,
            );
            s.redstone.loaded.insert(chunk);
        }
    }
    s.tickets
        .insert("redstone-test".into(), (ChunkPos { x: 0, z: 0 }, 2));
    s.refresh_active_chunks(s.tick);
    let wire = block(&s, "redstone_wire", &[]);
    for x in 0..33 {
        for z in 0..33 {
            s.world.set_block(x, 200, z, wire);
        }
    }
    s.world.fold_overlay();
    for x in 0..33 {
        for z in 0..33 {
            wake(&mut s, BlockPos { x, y: 200, z });
        }
    }
    advance(&mut s, 1);
    assert_eq!(s.redstone.settled_wires.len(), 1024);
    for _ in 0..16 {
        advance(&mut s, 1);
        assert!(s.redstone.settled_wires.len() <= 1024);
    }
    assert!(s.redstone.settled_wires.is_empty());
    assert_eq!(
        value(
            &s,
            BlockPos {
                x: 32,
                y: 200,
                z: 32
            },
            "west"
        ),
        "side"
    );
    assert_eq!(
        value(
            &s,
            BlockPos {
                x: 32,
                y: 200,
                z: 32
            },
            "power"
        ),
        "0"
    );
}

#[test]
fn wire_attenuates_and_unpowers_without_self_sustaining() {
    let mut s = state();
    let source = place(&mut s, 0, 200, 0, "redstone_block", &[]);
    let mut wire = Vec::new();
    for x in 1..8 {
        wire.push(place(&mut s, x, 200, 0, "redstone_wire", &[]));
    }
    wake(&mut s, wire[0]);
    advance(&mut s, 2);
    for (i, p) in wire.iter().enumerate() {
        assert_eq!(value(&s, *p, "power").parse::<u8>().unwrap(), 15 - i as u8);
    }
    assert_eq!(value(&s, wire[1], "east"), "side");
    assert_eq!(value(&s, wire[1], "west"), "side");
    let air = s.registry.states.air;
    s.world.set_block(source.x, source.y, source.z, air);
    wake(&mut s, wire[0]);
    advance(&mut s, 3);
    assert!(wire.iter().all(|p| value(&s, *p, "power") == "0"));
}
#[test]
fn wire_climbs_a_supported_step_and_sets_up_shape() {
    let mut s = state();
    place(&mut s, 0, 200, 0, "redstone_block", &[]);
    let wire = place(&mut s, 1, 200, 0, "redstone_wire", &[]);
    place(&mut s, 2, 200, 0, "stone", &[]);
    let upper = place(&mut s, 2, 201, 0, "redstone_wire", &[]);
    wake(&mut s, wire);
    advance(&mut s, 3);
    assert_eq!(value(&s, wire, "east"), "up");
    assert_eq!(value(&s, upper, "power"), "14");
}
#[test]
fn lever_hand_toggle_propagates_to_iron_trapdoor() {
    let mut s = state();
    let lever = place(&mut s, 0, 200, 0, "lever", &[("powered", "false")]);
    let door = place(&mut s, 1, 200, 0, "iron_trapdoor", &[]);
    let cells = crate::placement::use_block(&s.registry.blocks, lever, 0.0, &|p| {
        s.world.block_at(p.x, p.y, p.z)
    })
    .unwrap();
    for (p, id) in &cells {
        s.world.set_block(p.x, p.y, p.z, *id);
    }
    neighbours(&mut s, &cells);
    advance(&mut s, 3);
    assert_eq!(value(&s, door, "open"), "true");
    assert_eq!(power_at(&s, door), 15);
}
#[test]
fn torch_inverts_only_after_two_game_ticks() {
    let mut s = state();
    let support = place(&mut s, 2, 200, 2, "stone", &[]);
    let torch = place(&mut s, 2, 201, 2, "redstone_torch", &[("lit", "true")]);
    place(&mut s, 3, 200, 2, "redstone_block", &[]);
    wake(&mut s, torch);
    advance(&mut s, 2);
    assert_eq!(value(&s, torch, "lit"), "true");
    advance(&mut s, 1);
    assert_eq!(value(&s, torch, "lit"), "false");
    assert_eq!(power_at(&s, support), 15);
}
#[test]
fn repeater_delay_and_side_lock_are_authoritative() {
    let mut s = state();
    let diode = place(
        &mut s,
        4,
        200,
        4,
        "repeater",
        &[("facing", "east"), ("delay", "3"), ("powered", "false")],
    );
    place(&mut s, 5, 200, 4, "redstone_block", &[]);
    wake(&mut s, diode);
    advance(&mut s, 6);
    assert_eq!(value(&s, diode, "powered"), "false");
    advance(&mut s, 1);
    assert_eq!(value(&s, diode, "powered"), "true");
    let side = place(
        &mut s,
        4,
        200,
        3,
        "repeater",
        &[("facing", "north"), ("powered", "true")],
    );
    // Vanilla 26.1.2 DiodeBlock::tick turns an unlocked, un-fed diode off;
    // RepeaterBlock::isLocked needs a live side signal, not a frozen state bit.
    let side_source = place(&mut s, 4, 200, 2, "redstone_block", &[]);
    let air = s.registry.states.air;
    s.world.set_block(5, 200, 4, air);
    wake(&mut s, diode);
    advance(&mut s, 9);
    assert_eq!(value(&s, diode, "locked"), "true");
    assert_eq!(value(&s, diode, "powered"), "true");
    assert_eq!(output(&s, side, diode), 15);
    assert_eq!(value(&s, side, "powered"), "true");
    s.world
        .set_block(side_source.x, side_source.y, side_source.z, air);
    wake(&mut s, side);
    advance(&mut s, 12);
    assert_eq!(value(&s, side, "powered"), "false");
    assert_eq!(output(&s, side, diode), 0);
    assert_eq!(value(&s, diode, "locked"), "false");
    assert_eq!(value(&s, diode, "powered"), "false");
}
#[test]
fn observer_watches_front_pulses_back_once() {
    let mut s = state();
    let observer = place(
        &mut s,
        5,
        200,
        5,
        "observer",
        &[("facing", "east"), ("powered", "false")],
    );
    let input = place(&mut s, 6, 200, 5, "stone", &[]);
    let output_wire = place(&mut s, 4, 200, 5, "redstone_wire", &[]);
    let id = s.world.block_at(input.x, input.y, input.z).unwrap();
    neighbours(&mut s, &[(input, id)]);
    advance(&mut s, 2);
    assert_eq!(value(&s, observer, "powered"), "false");
    advance(&mut s, 1);
    assert_eq!(value(&s, observer, "powered"), "true");
    advance(&mut s, 2);
    assert_eq!(value(&s, observer, "powered"), "false");
    advance(&mut s, 4);
    assert_eq!(value(&s, output_wire, "power"), "0");
}
#[test]
fn chest_comparator_keeps_analog_signal_and_subtraction() {
    let mut s = state();
    let chest = place(&mut s, 6, 200, 6, "chest", &[]);
    let comparator = place(
        &mut s,
        5,
        200,
        6,
        "comparator",
        &[("facing", "east"), ("mode", "compare")],
    );
    let item = s
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    s.block_entities.insert(
        (chest.x, chest.y, chest.z),
        BlockEntityState {
            slots: vec![mc_data::ItemStack::new(item, 64); 27],
            furnace: None,
        },
    );
    wake(&mut s, comparator);
    advance(&mut s, 4);
    assert_eq!(comparator_output(&s, comparator), 15);
    assert_eq!(value(&s, comparator, "powered"), "true");
    place(&mut s, 5, 200, 5, "redstone_wire", &[("power", "7")]);
    let new = block(
        &s,
        "comparator",
        &[
            ("facing", "east"),
            ("mode", "subtract"),
            ("powered", "true"),
        ],
    );
    s.world.set_block(5, 200, 6, new);
    comparator_tick(&mut s, &mut Vec::new(), comparator, 1);
    s.world.fold_overlay();
    assert_eq!(comparator_output(&s, comparator), 8);
}
#[test]
fn sticky_piston_door_pushes_and_pulls_without_duplication() {
    let mut s = state();
    let piston = place(&mut s, 5, 200, 5, "sticky_piston", &[("facing", "east")]);
    place(&mut s, 6, 200, 5, "stone", &[]);
    let lever = place(&mut s, 5, 200, 6, "lever", &[("powered", "true")]);
    wake(&mut s, piston);
    advance(&mut s, 2);
    assert_eq!(
        at(&s, BlockPos { x: 6, y: 200, z: 5 })
            .unwrap()
            .block
            .id
            .path(),
        "piston_head"
    );
    assert_eq!(
        at(&s, BlockPos { x: 7, y: 200, z: 5 })
            .unwrap()
            .block
            .id
            .path(),
        "stone"
    );
    let off = block(&s, "lever", &[("powered", "false")]);
    s.world.set_block(lever.x, lever.y, lever.z, off);
    wake(&mut s, piston);
    advance(&mut s, 2);
    assert_eq!(value(&s, piston, "extended"), "false");
    assert_eq!(
        at(&s, BlockPos { x: 6, y: 200, z: 5 })
            .unwrap()
            .block
            .id
            .path(),
        "stone"
    );
    assert_eq!(s.world.block_at(7, 200, 5), Some(s.registry.states.air));
}
#[test]
fn piston_refuses_thirteen_blocks_and_item_containers_atomically() {
    let mut s = state();
    let piston = place(&mut s, 0, 200, 2, "piston", &[("facing", "east")]);
    place(&mut s, 0, 200, 3, "redstone_block", &[]);
    for x in 1..14 {
        place(&mut s, x, 200, 2, "stone", &[]);
    }
    wake(&mut s, piston);
    advance(&mut s, 2);
    assert_eq!(value(&s, piston, "extended"), "false");
    place(&mut s, 1, 200, 2, "chest", &[]);
    wake(&mut s, piston);
    advance(&mut s, 2);
    assert_eq!(value(&s, piston, "extended"), "false");
    assert_eq!(
        at(&s, BlockPos { x: 1, y: 200, z: 2 })
            .unwrap()
            .block
            .id
            .path(),
        "chest"
    );
}
#[test]
fn lamp_turns_off_after_four_ticks() {
    let mut s = state();
    let lamp = place(&mut s, 4, 200, 4, "redstone_lamp", &[("lit", "false")]);
    let source = place(&mut s, 4, 200, 5, "redstone_block", &[]);
    wake(&mut s, lamp);
    advance(&mut s, 1);
    assert_eq!(value(&s, lamp, "lit"), "true");
    let air = s.registry.states.air;
    s.world.set_block(source.x, source.y, source.z, air);
    wake(&mut s, lamp);
    advance(&mut s, 4);
    assert_eq!(value(&s, lamp, "lit"), "true");
    advance(&mut s, 1);
    assert_eq!(value(&s, lamp, "lit"), "false");
}
#[test]
fn button_press_releases_and_note_tuning_wraps() {
    let mut s = state();
    let button = place(&mut s, 4, 200, 4, "stone_button", &[("powered", "false")]);
    let cells = crate::placement::use_block(&s.registry.blocks, button, 0.0, &|p| {
        s.world.block_at(p.x, p.y, p.z)
    })
    .unwrap();
    for (p, id) in &cells {
        s.world.set_block(p.x, p.y, p.z, *id);
    }
    neighbours(&mut s, &cells);
    advance(&mut s, 20);
    assert_eq!(value(&s, button, "powered"), "true");
    advance(&mut s, 1);
    assert_eq!(value(&s, button, "powered"), "false");
    let note = place(&mut s, 6, 200, 6, "note_block", &[("note", "24")]);
    let cells = crate::placement::use_block(&s.registry.blocks, note, 0.0, &|p| {
        s.world.block_at(p.x, p.y, p.z)
    })
    .unwrap();
    assert_eq!(
        prop(
            s.registry.blocks.by_id(BlockStateId(cells[0].1)).unwrap(),
            "note"
        ),
        Some("0")
    );
}
#[test]
fn pressure_plate_detects_body_and_releases() {
    let mut s = state();
    let plate = place(&mut s, 5, 200, 5, "oak_pressure_plate", &[]);
    let id =
        crate::tick::spawn_item_entity(&mut s, [5.5, 200.0, 5.5], mc_data::ItemStack::new(1, 1));
    wake(&mut s, plate);
    advance(&mut s, 1);
    assert_eq!(value(&s, plate, "powered"), "true");
    let slot = s.entities.slot_of(id).unwrap();
    s.entities.set_pos(slot, [8.0, 200.0, 8.0]);
    advance(&mut s, 22);
    assert_eq!(value(&s, plate, "powered"), "false");
}
#[test]
fn powered_rail_chain_is_limited_to_eight_steps() {
    let mut s = state();
    place(&mut s, 0, 200, 1, "redstone_block", &[]);
    let mut rails = Vec::new();
    for x in 0..11 {
        rails.push(place(
            &mut s,
            x,
            200,
            0,
            "powered_rail",
            &[("shape", "east_west")],
        ));
    }
    for p in &rails {
        wake(&mut s, *p);
    }
    advance(&mut s, 2);
    assert_eq!(value(&s, rails[8], "powered"), "true");
    assert_eq!(value(&s, rails[9], "powered"), "false");
}
#[test]
fn daylight_detector_periodically_resamples() {
    let mut s = state();
    let detector = place(&mut s, 4, 200, 4, "daylight_detector", &[]);
    s.day_time = 6000;
    wake(&mut s, detector);
    advance(&mut s, 1);
    assert_eq!(value(&s, detector, "power"), "15");
    s.day_time = 18000;
    advance(&mut s, 20);
    assert_eq!(value(&s, detector, "power"), "0");
}
#[test]
fn torch_feedback_clock_has_multiple_edges() {
    let mut s = state();
    place(&mut s, 4, 200, 4, "stone", &[]);
    let torch = place(
        &mut s,
        5,
        200,
        4,
        "redstone_wall_torch",
        &[("facing", "east"), ("lit", "true")],
    );
    let a = place(&mut s, 5, 201, 4, "redstone_wire", &[]);
    place(&mut s, 4, 201, 4, "redstone_wire", &[]);
    wake(&mut s, a);
    wake(&mut s, torch);
    let mut last = value(&s, torch, "lit").to_owned();
    let mut edges = 0;
    for _ in 0..40 {
        advance(&mut s, 1);
        let lit = value(&s, torch, "lit");
        if lit != last {
            edges += 1;
            last = lit.into();
        }
    }
    assert!(edges >= 4, "clock edges: {edges}");
}

#[test]
fn iron_door_halves_share_power_and_close_together() {
    let mut s = state();
    let lower = place(&mut s, 5, 200, 5, "iron_door", &[("half", "lower")]);
    let upper = place(&mut s, 5, 201, 5, "iron_door", &[("half", "upper")]);
    let source = place(&mut s, 6, 201, 5, "redstone_block", &[]);
    wake(&mut s, upper);
    advance(&mut s, 2);
    assert_eq!(value(&s, lower, "open"), "true");
    assert_eq!(value(&s, upper, "open"), "true");
    let air = s.registry.states.air;
    s.world.set_block(source.x, source.y, source.z, air);
    wake(&mut s, upper);
    advance(&mut s, 2);
    assert_eq!(value(&s, lower, "open"), "false");
    assert_eq!(value(&s, upper, "open"), "false");
}

#[test]
fn full_trapped_chest_lid_changes_wake_wire_independent_of_inventory_signal() {
    let mut s = state();
    let chest = place(&mut s, 5, 200, 5, "trapped_chest", &[]);
    let wire = place(&mut s, 6, 200, 5, "redstone_wire", &[]);
    s.block_entities.insert(
        (5, 200, 5),
        BlockEntityState {
            slots: vec![mc_data::ItemStack::new(1, 64); 27],
            furnace: None,
        },
    );
    advance(&mut s, 3);
    assert_eq!(value(&s, wire, "power"), "0");
    s.open_containers
        .insert(EntityId(90), (chest.x, chest.y, chest.z));
    advance(&mut s, 3);
    assert_eq!(value(&s, wire, "power"), "1");
    s.open_containers.clear();
    advance(&mut s, 3);
    assert_eq!(value(&s, wire, "power"), "0");
}

#[test]
fn detector_rail_does_not_accept_item_occupancy() {
    let mut s = state();
    let rail = place(&mut s, 5, 200, 5, "detector_rail", &[]);
    crate::tick::spawn_item_entity(&mut s, [5.5, 200.0, 5.5], mc_data::ItemStack::new(1, 1));
    wake(&mut s, rail);
    advance(&mut s, 2);
    assert_eq!(value(&s, rail, "powered"), "false");
    let minecart = s
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:minecart").unwrap())
        .unwrap();
    s.spawn_mob(minecart, [5.5, 200.0, 5.5], 1.0);
    wake(&mut s, rail);
    advance(&mut s, 1);
    assert_eq!(value(&s, rail, "powered"), "true");
}

#[test]
fn comparator_nbt_and_pending_observer_trailing_edge_survive_native_restart() {
    let directory = tempfile::tempdir().unwrap();
    let mut s = state_in(Some(directory.path().to_owned()));
    let chest = place(&mut s, 6, 200, 6, "chest", &[]);
    let comparator = place(&mut s, 5, 200, 6, "comparator", &[("facing", "east")]);
    let item = s
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    s.block_entities.insert(
        (chest.x, chest.y, chest.z),
        BlockEntityState {
            slots: vec![mc_data::ItemStack::new(item, 64); 27],
            furnace: None,
        },
    );
    comparator_tick(&mut s, &mut Vec::new(), comparator, 1);
    s.world.fold_overlay();
    let observer = place(&mut s, 10, 200, 10, "observer", &[("facing", "east")]);
    let watched = place(&mut s, 11, 200, 10, "stone", &[]);
    let id = s.world.block_at(watched.x, watched.y, watched.z).unwrap();
    neighbours(&mut s, &[(watched, id)]);
    advance(&mut s, 3);
    assert_eq!(value(&s, observer, "powered"), "true");
    assert_eq!(comparator_output(&s, comparator), 15);
    let records = s
        .block_entities
        .iter()
        .map(|(pos, record)| {
            (
                *pos,
                crate::journal::ReplayBlockEntity {
                    slots: record.slots.clone(),
                    furnace: record.furnace_image(),
                    tick: s.tick,
                },
            )
        })
        .collect();
    let absent = s.block_ticks.checkpoint(&mut s.world, s.tick);
    let mut capture = s.world.capture_dirty(&records);
    s.world.capture_scheduled_block_ticks(&mut capture, absent);
    capture.write().unwrap();
    drop(s);
    let mut restarted = state_in(Some(directory.path().to_owned()));
    assert_eq!(value(&restarted, observer, "powered"), "true");
    assert_eq!(comparator_output(&restarted, comparator), 15);
    advance(&mut restarted, 2);
    assert_eq!(value(&restarted, observer, "powered"), "false");
}

#[test]
fn wooden_button_holds_an_arrow_while_stone_button_ignores_it() {
    let mut s = state();
    let wooden = place(&mut s, 5, 200, 5, "oak_button", &[]);
    let stone = place(&mut s, 7, 200, 5, "stone_button", &[]);
    let type_id = s
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:arrow").unwrap())
        .unwrap();
    let arrow = s.spawn_mob(type_id, [5.5, 200.5, 5.5], 1.0);
    s.spawn_mob(type_id, [7.5, 200.5, 5.5], 1.0);
    wake(&mut s, wooden);
    wake(&mut s, stone);
    advance(&mut s, 35);
    assert_eq!(value(&s, wooden, "powered"), "true");
    assert_eq!(value(&s, stone, "powered"), "false");
    let slot = s.entities.slot_of(arrow).unwrap();
    s.entities.set_pos(slot, [9.0, 200.5, 5.5]);
    advance(&mut s, 31);
    assert_eq!(value(&s, wooden, "powered"), "false");
}

#[test]
fn piston_retraction_waits_for_unloaded_head_and_retains_pending_work() {
    let mut s = state();
    let piston = place(
        &mut s,
        15,
        200,
        5,
        "piston",
        &[("facing", "east"), ("extended", "true")],
    );
    wake(&mut s, piston);
    advance(&mut s, 2);
    assert_eq!(value(&s, piston, "extended"), "true");
    let id = at(&s, piston).unwrap().block.id.clone();
    assert!(s.block_ticks.contains(piston, &id, 0));
    let mut wanted = rustc_hash::FxHashMap::default();
    wanted.insert(ChunkPos { x: 1, z: 0 }, 1);
    s.world.ensure_blocking(&wanted);
    place(
        &mut s,
        16,
        200,
        5,
        "piston_head",
        &[("facing", "east"), ("type", "normal")],
    );
    advance(&mut s, 2);
    assert_eq!(value(&s, piston, "extended"), "false");
    assert_eq!(s.world.block_at(16, 200, 5), Some(s.registry.states.air));
}

#[test]
fn weighted_pressure_plates_count_bodies_not_stack_size() {
    let mut s = state();
    let light = place(&mut s, 4, 200, 4, "light_weighted_pressure_plate", &[]);
    let heavy = place(&mut s, 8, 200, 4, "heavy_weighted_pressure_plate", &[]);
    for x in [4.5, 8.5] {
        for index in 0..11 {
            let stack = mc_data::ItemStack::new(1, 64).with_custom_name(format!("body-{index}"));
            crate::tick::spawn_item_entity(&mut s, [x, 200.0, 4.5], stack);
        }
    }
    wake(&mut s, light);
    wake(&mut s, heavy);
    advance(&mut s, 1);
    assert_eq!(value(&s, light, "power"), "11");
    assert_eq!(value(&s, heavy, "power"), "2");
}

#[test]
fn spectator_body_never_triggers_a_pressure_plate() {
    let mut s = state();
    let plate = place(&mut s, 5, 200, 5, "stone_pressure_plate", &[]);
    let type_id = s
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:pig").unwrap())
        .unwrap();
    let id = s.spawn_mob(type_id, [5.5, 200.0, 5.5], 20.0);
    let slot = s.entities.slot_of(id).unwrap();
    let connection = crate::boundary::ConnectionId(9);
    s.entities
        .set_kind(slot, crate::entities::EntityKind::Player { connection });
    s.players.insert(
        id,
        crate::player::PlayerState::new(connection, crate::commands::GameMode::Spectator),
    );
    wake(&mut s, plate);
    advance(&mut s, 1);
    assert_eq!(value(&s, plate, "powered"), "false");
    s.players.get_mut(&id).unwrap().game_mode = crate::commands::GameMode::Survival;
    wake(&mut s, plate);
    advance(&mut s, 1);
    assert_eq!(value(&s, plate, "powered"), "true");
}

#[test]
fn idle_controls_back_off_and_current_pass_census_detects_new_bodies() {
    let mut s = state();
    let wooden = place(&mut s, 2, 200, 2, "oak_button", &[]);
    let plate = place(&mut s, 5, 200, 5, "oak_pressure_plate", &[]);
    let rail = place(&mut s, 8, 200, 8, "detector_rail", &[]);
    for pos in [wooden, plate, rail] {
        wake(&mut s, pos);
    }
    advance(&mut s, 1);
    let image = s.block_ticks.image();
    for pos in [wooden, plate, rail] {
        let pending: Vec<_> = image
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.ticks)
            .filter(|tick| tick.pos == [pos.x, pos.y, pos.z])
            .collect();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].trigger_tick, 10);
    }
    let type_id = |s: &TickState, name: &str| {
        s.registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap()
    };
    let arrow = type_id(&s, "arrow");
    let minecart = type_id(&s, "minecart");
    s.spawn_mob(arrow, [2.5, 200.5, 2.5], 1.0);
    s.spawn_mob(minecart, [8.5, 200.0, 8.5], 1.0);
    crate::tick::spawn_item_entity(&mut s, [5.5, 200.0, 5.5], mc_data::ItemStack::new(1, 1));
    // The tick-10 idle poll must see bodies born at tick 1, before the
    // unrelated 20-tick natural-spawn census would refresh.
    advance(&mut s, 10);
    for pos in [wooden, plate, rail] {
        assert_eq!(value(&s, pos, "powered"), "true");
    }
}

#[test]
fn local_entity_query_prunes_distant_population_and_keeps_giant_border_bodies() {
    let mut s = state();
    for x in 0..4096 {
        crate::tick::spawn_item_entity(
            &mut s,
            [1000.0 + x as f64 * 2.0, 200.0, 1000.0],
            mc_data::ItemStack::new(1, 1),
        );
    }
    let id =
        crate::tick::spawn_item_entity(&mut s, [-2.0, 199.0, -2.0], mc_data::ItemStack::new(1, 1));
    let slot = s.entities.slot_of(id).unwrap();
    s.entities.set_half_width(slot, 3.0);
    s.entities.body_mut(slot).height = 3.0;
    let mut query = entity_query::EntityQuery::default();
    query.rebuild(&s);
    assert_eq!(
        query.count([0.125, 200.0, 0.125], [0.875, 200.25, 0.875], 15, |_| true),
        1
    );
    assert!(
        query.visited.get() < 128,
        "distant rows must be spatially pruned"
    );
    s.entities.body_mut(slot).alive = false;
    query.rebuild(&s);
    assert_eq!(
        query.count([0.125, 200.0, 0.125], [0.875, 200.25, 0.875], 15, |_| true),
        0
    );
    assert!(query.visited.get() < 128);
}

#[test]
fn note_block_rising_edge_sends_native_event_with_instrument_and_note_state() {
    let mut s = state();
    let type_id = s
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:pig").unwrap())
        .unwrap();
    let id = s.spawn_mob(type_id, [6.0, 200.0, 8.0], 20.0);
    let slot = s.entities.slot_of(id).unwrap();
    let connection = crate::boundary::ConnectionId(9);
    s.entities
        .set_kind(slot, crate::entities::EntityKind::Player { connection });
    s.players.insert(
        id,
        crate::player::PlayerState::new(connection, crate::commands::GameMode::Survival),
    );
    let note = place(&mut s, 6, 200, 6, "note_block", &[("note", "24")]);
    place(&mut s, 6, 199, 6, "gold_block", &[]);
    place(&mut s, 7, 200, 6, "redstone_block", &[]);
    wake(&mut s, note);
    advance(&mut s, 1);
    let events: Vec<_> = s.outbox[&connection]
        .iter()
        .filter_map(|change| {
            if let crate::boundary::PlayerChange::BlockEvent {
                position,
                block,
                state,
                action,
                parameter,
                ..
            } = change
            {
                Some((*position, *block, *state, *action, *parameter))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, note);
    assert_eq!((events[0].3, events[0].4), (0, 0));
    let event_state = s.registry.blocks.by_id(BlockStateId(events[0].2)).unwrap();
    assert_eq!(
        events[0].1,
        u32::try_from(event_state.block.raw_id).unwrap()
    );
    assert_eq!(prop(event_state, "instrument"), Some("bell"));
    assert_eq!(prop(event_state, "note"), Some("24"));
    assert_eq!(prop(event_state, "powered"), Some("true"));
    s.outbox.clear();
    wake(&mut s, note);
    advance(&mut s, 1);
    assert!(s.outbox.is_empty(), "sustained power must not retrigger");
    let air = s.registry.states.air;
    s.world.set_block(7, 200, 6, air);
    wake(&mut s, note);
    advance(&mut s, 1);
    assert_eq!(value(&s, note, "powered"), "false");
    place(&mut s, 6, 201, 6, "stone", &[]);
    place(&mut s, 7, 200, 6, "redstone_block", &[]);
    wake(&mut s, note);
    advance(&mut s, 1);
    assert!(
        s.outbox.is_empty(),
        "ordinary instruments require air above"
    );
    s.world.set_block(7, 200, 6, air);
    wake(&mut s, note);
    advance(&mut s, 1);
    place(&mut s, 6, 201, 6, "skeleton_skull", &[]);
    place(&mut s, 7, 200, 6, "redstone_block", &[]);
    wake(&mut s, note);
    advance(&mut s, 1);
    assert_eq!(value(&s, note, "instrument"), "skeleton");
    assert!(
        s.outbox[&connection]
            .iter()
            .any(|change| matches!(change, crate::boundary::PlayerChange::BlockEvent { .. }))
    );
}

#[test]
fn obsolete_idle_poll_cannot_shorten_pressed_plate_tail() {
    let mut s = state();
    let plate = place(&mut s, 5, 200, 5, "oak_pressure_plate", &[]);
    wake(&mut s, plate);
    advance(&mut s, 1);
    let id =
        crate::tick::spawn_item_entity(&mut s, [5.5, 200.0, 5.5], mc_data::ItemStack::new(1, 1));
    wake(&mut s, plate);
    advance(&mut s, 1);
    assert_eq!(value(&s, plate, "powered"), "true");
    let slot = s.entities.slot_of(id).unwrap();
    s.entities.set_pos(slot, [50.0, 200.0, 50.0]);
    advance(&mut s, 10);
    assert_eq!(
        value(&s, plate, "powered"),
        "true",
        "the old tick10 idle poll must not release early"
    );
    advance(&mut s, 10);
    assert_eq!(
        value(&s, plate, "powered"),
        "false",
        "the tick21 pressed tail observes current bodies"
    );
}

#[test]
fn pistons_are_quasi_connected_but_only_react_to_a_neighbour_update() {
    let mut s = state();
    let piston = place(&mut s, 4, 200, 4, "piston", &[("facing", "east")]);
    let control = place(&mut s, 8, 200, 4, "piston", &[("facing", "east")]);
    // The source touches the cell ABOVE the piston, not the piston: vanilla's
    // quasi-connectivity powers it, yet nothing has updated it (BUD).
    place(&mut s, 4, 202, 4, "redstone_block", &[]);
    advance(&mut s, 6);
    assert_eq!(value(&s, piston, "extended"), "false");
    wake(&mut s, piston);
    wake(&mut s, control);
    advance(&mut s, 6);
    assert_eq!(value(&s, piston, "extended"), "true");
    assert_eq!(value(&s, control, "extended"), "false");
}

#[test]
fn pistons_ignore_power_arriving_through_their_own_face() {
    let mut s = state();
    let piston = place(&mut s, 4, 200, 4, "piston", &[("facing", "east")]);
    // PistonBaseBlock.getNeighborSignal skips the push direction.
    place(&mut s, 5, 200, 4, "redstone_block", &[]);
    wake(&mut s, piston);
    advance(&mut s, 6);
    assert_eq!(value(&s, piston, "extended"), "false");
}
