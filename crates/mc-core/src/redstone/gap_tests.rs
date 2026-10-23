use super::*;

#[test]
fn gap_strong_power_conducts_one_solid_and_not_two() {
    let mut s = state();
    let lever = place(
        &mut s,
        2,
        200,
        2,
        "lever",
        &[("face", "wall"), ("facing", "west"), ("powered", "true")],
    );
    let stone = place(&mut s, 3, 200, 2, "stone", &[]);
    let lamp = place(&mut s, 4, 200, 2, "redstone_lamp", &[]);
    assert_eq!(output(&s, stone, lamp), 15);
    wake(&mut s, lamp);
    advance(&mut s, 2);
    assert_eq!(value(&s, lamp, "lit"), "true");
    place(&mut s, 4, 200, 2, "stone", &[]);
    assert_eq!(output(&s, lamp, BlockPos { x: 5, ..lamp }), 0);
    // The lever's other neighbors are weakly powered, not conductors.
    let weak = place(&mut s, 2, 200, 3, "stone", &[]);
    assert_eq!(power_at(&s, weak), 15);
    assert_eq!(output(&s, weak, BlockPos { z: 4, ..weak }), 0);
    assert_eq!(output(&s, lever, weak), 15);
}

#[test]
fn gap_dust_direction_and_weak_conductor_do_not_feed_back() {
    let mut s = state();
    let wire = place(
        &mut s,
        3,
        200,
        3,
        "redstone_wire",
        &[("power", "12"), ("east", "side"), ("west", "side")],
    );
    assert_eq!(output(&s, wire, BlockPos { y: 201, ..wire }), 0);
    assert_eq!(output(&s, wire, BlockPos { z: 4, ..wire }), 0);
    assert_eq!(output(&s, wire, BlockPos { x: 4, ..wire }), 12);
    let floor = place(&mut s, 3, 199, 3, "stone", &[]);
    assert_eq!(output(&s, floor, BlockPos { x: 4, ..floor }), 12);
    wake(&mut s, wire);
    advance(&mut s, 3);
    assert_eq!(value(&s, wire, "power"), "0");
}

#[test]
fn gap_conductor_neighbor_updates_wake_lamp() {
    let mut s = state();
    let lever = place(
        &mut s,
        2,
        200,
        2,
        "lever",
        &[("face", "wall"), ("facing", "west"), ("powered", "true")],
    );
    place(&mut s, 3, 200, 2, "stone", &[]);
    let lamp = place(&mut s, 4, 200, 2, "redstone_lamp", &[]);
    super::super::wake(&mut s, lever);
    advance(&mut s, 3);
    assert_eq!(value(&s, lamp, "lit"), "true");
    place(
        &mut s,
        2,
        200,
        2,
        "lever",
        &[("face", "wall"), ("facing", "west"), ("powered", "false")],
    );
    super::super::wake(&mut s, lever);
    advance(&mut s, 8);
    assert_eq!(value(&s, lamp, "lit"), "false");
}

#[test]
fn gap_repeater_extends_an_admitted_short_pulse() {
    let mut s = state();
    let input = place(&mut s, 3, 200, 2, "redstone_block", &[]);
    let diode = place(
        &mut s,
        3,
        200,
        3,
        "repeater",
        &[("facing", "north"), ("delay", "4")],
    );
    wake(&mut s, diode);
    advance(&mut s, 1);
    place(&mut s, input.x, input.y, input.z, "air", &[]);
    advance(&mut s, 8);
    assert_eq!(value(&s, diode, "powered"), "true");
    advance(&mut s, 7);
    assert_eq!(value(&s, diode, "powered"), "true");
    advance(&mut s, 1);
    assert_eq!(value(&s, diode, "powered"), "false");
}

#[test]
fn gap_comparator_reads_analog_blocks_through_stone() {
    let mut s = state();
    let diode = place(&mut s, 4, 200, 4, "comparator", &[("facing", "north")]);
    place(&mut s, 4, 200, 3, "stone", &[]);
    for (name, props, expected) in [
        ("water_cauldron", vec![("level", "3")], 3),
        ("composter", vec![("level", "8")], 8),
        ("cake", vec![("bites", "0")], 14),
        ("beehive", vec![("honey_level", "5")], 5),
    ] {
        let source = place(&mut s, 4, 200, 2, name, &props);
        super::super::wake(&mut s, source);
        advance(&mut s, 6);
        assert_eq!(comparator_output(&s, diode), expected, "{name}");
    }
}

#[test]
fn gap_comparator_container_changes_wake_through_solid() {
    let mut s = state();
    let diode = place(&mut s, 4, 200, 4, "comparator", &[("facing", "north")]);
    place(&mut s, 4, 200, 3, "stone", &[]);
    let chest = place(&mut s, 4, 200, 2, "chest", &[]);
    let item = s
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let record = BlockEntityState {
        slots: vec![mc_data::ItemStack::new(item, 64); 27],
        furnace: None,
    };
    s.block_entities.insert((chest.x, chest.y, chest.z), record);
    advance(&mut s, 6);
    assert_eq!(comparator_output(&s, diode), 15);
    s.block_entities
        .get_mut(&(chest.x, chest.y, chest.z))
        .unwrap()
        .slots
        .fill(Default::default());
    advance(&mut s, 6);
    assert_eq!(comparator_output(&s, diode), 0);
}

#[test]
fn gap_daylight_detector_requires_local_sky_and_overworld() {
    let mut s = state();
    s.day_time = 6000;
    let detector = place(&mut s, 4, 200, 4, "daylight_detector", &[]);
    wake(&mut s, detector);
    advance(&mut s, 1);
    assert_eq!(value(&s, detector, "power"), "15");
    s.weather = (crate::commands::Weather::Thunder, 1000);
    s.thundering = true;
    s.thunder_remaining_ticks = 1000;
    s.rain_level = 1.0;
    s.thunder_level = 1.0;
    wake(&mut s, detector);
    advance(&mut s, 1);
    assert_eq!(value(&s, detector, "power"), "10");
    s.weather = (crate::commands::Weather::Clear, 1000);
    s.thundering = false;
    s.rain_level = 0.0;
    s.thunder_level = 0.0;
    for d in NEIGHBOURS {
        let p = offset(detector, d).unwrap();
        place(&mut s, p.x, p.y, p.z, "stone", &[]);
    }
    assert_eq!(s.world.light_at(4, 200, 4).unwrap().0, 0);
    wake(&mut s, detector);
    advance(&mut s, 1);
    assert_eq!(value(&s, detector, "power"), "0");
    s.dimension = crate::dimensions::Dimension::Nether;
    place(
        &mut s,
        4,
        200,
        4,
        "daylight_detector",
        &[("inverted", "true")],
    );
    wake(&mut s, detector);
    advance(&mut s, 1);
    assert_eq!(value(&s, detector, "power"), "15");
}

#[test]
fn gap_rail_shapes_follow_neighbors_and_slopes() {
    let mut s = state();
    let west = place(&mut s, 3, 200, 4, "rail", &[]);
    let middle = place(&mut s, 4, 200, 4, "rail", &[]);
    let east = place(&mut s, 5, 200, 4, "rail", &[]);
    for p in [west, middle, east] {
        wake(&mut s, p);
    }
    advance(&mut s, 6);
    assert_eq!(value(&s, middle, "shape"), "east_west");
    place(&mut s, 5, 200, 4, "air", &[]);
    let south = place(&mut s, 4, 200, 5, "rail", &[]);
    for p in [middle, south] {
        wake(&mut s, p);
    }
    advance(&mut s, 6);
    assert_eq!(value(&s, middle, "shape"), "south_west");
    place(&mut s, 4, 200, 5, "air", &[]);
    place(&mut s, 5, 200, 4, "stone", &[]);
    let upper = place(&mut s, 5, 201, 4, "rail", &[]);
    for p in [middle, upper] {
        wake(&mut s, p);
    }
    advance(&mut s, 6);
    assert_eq!(value(&s, middle, "shape"), "ascending_east");
}

#[test]
fn gap_powered_rails_reject_disconnected_shapes_and_heights() {
    let mut s = state();
    let rail = place(&mut s, 4, 200, 4, "powered_rail", &[("shape", "east_west")]);
    place(
        &mut s,
        5,
        200,
        4,
        "powered_rail",
        &[("shape", "north_south")],
    );
    place(&mut s, 6, 200, 4, "redstone_block", &[]);
    assert!(!rails::powered(&s, rail));
    place(&mut s, 5, 200, 4, "air", &[]);
    place(&mut s, 5, 201, 4, "powered_rail", &[("shape", "east_west")]);
    place(&mut s, 6, 201, 4, "redstone_block", &[]);
    assert!(!rails::powered(&s, rail));
    place(
        &mut s,
        4,
        200,
        4,
        "powered_rail",
        &[("shape", "ascending_east")],
    );
    assert!(rails::powered(&s, rail));
}

#[test]
fn gap_conductors_and_insulators_use_block_shape() {
    let mut s = state();
    for name in [
        "stone",
        "packed_ice",
        "blue_ice",
        "slime_block",
        "soul_sand",
        "mud",
    ] {
        let pos = place(&mut s, 4, 200, 4, name, &[]);
        assert!(conducts(at(&s, pos).unwrap()), "{name}");
    }
    for name in [
        "glass",
        "oak_leaves",
        "ice",
        "redstone_block",
        "observer",
        "piston",
        "tnt",
        "oak_stairs",
        "oak_slab",
    ] {
        let pos = place(&mut s, 4, 200, 4, name, &[]);
        assert!(!conducts(at(&s, pos).unwrap()), "{name}");
    }
}

#[test]
fn gap_comparator_ignores_non_diode_side_sources() {
    let mut s = state();
    let diode = place(&mut s, 4, 200, 4, "comparator", &[("facing", "north")]);
    place(&mut s, 4, 200, 3, "water_cauldron", &[("level", "3")]);
    place(&mut s, 3, 200, 4, "lever", &[("powered", "true")]);
    wake(&mut s, diode);
    advance(&mut s, 5);
    assert_eq!(comparator_output(&s, diode), 3);
    place(&mut s, 3, 200, 4, "redstone_block", &[]);
    wake(&mut s, diode);
    advance(&mut s, 5);
    assert_eq!(comparator_output(&s, diode), 0);
}

#[test]
fn gap_lightning_rod_strongly_powers_only_its_base_and_expires() {
    let mut s = state();
    let rod = place(
        &mut s,
        3,
        200,
        3,
        "lightning_rod",
        &[("facing", "up"), ("powered", "true")],
    );
    let below = place(&mut s, 3, 199, 3, "stone", &[]);
    let side = place(&mut s, 4, 200, 3, "stone", &[]);
    assert_eq!(output(&s, below, BlockPos { x: 4, ..below }), 15);
    assert_eq!(output(&s, side, BlockPos { x: 5, ..side }), 0);
    schedule_cell(&mut s, rod, 8, 4);
    advance(&mut s, 9);
    assert_eq!(value(&s, rod, "powered"), "false");
}

#[test]
fn gap_rail_power_range_is_eight_connected_hops() {
    let mut s = state();
    for x in 1..=10 {
        place(&mut s, x, 200, 4, "powered_rail", &[("shape", "east_west")]);
    }
    place(&mut s, 0, 200, 4, "redstone_block", &[]);
    assert!(rails::powered(&s, BlockPos { x: 9, y: 200, z: 4 }));
    assert!(!rails::powered(
        &s,
        BlockPos {
            x: 10,
            y: 200,
            z: 4
        }
    ));
}
