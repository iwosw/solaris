use super::*;

const BULBS: [&str; 8] = [
    "copper_bulb",
    "exposed_copper_bulb",
    "weathered_copper_bulb",
    "oxidized_copper_bulb",
    "waxed_copper_bulb",
    "waxed_exposed_copper_bulb",
    "waxed_weathered_copper_bulb",
    "waxed_oxidized_copper_bulb",
];

fn set_source(state: &mut TickState, powered: bool) {
    let pos = place(
        state,
        5,
        200,
        6,
        if powered { "redstone_block" } else { "air" },
        &[],
    );
    let id = state.world.block_at(pos.x, pos.y, pos.z).unwrap();
    neighbours(state, &[(pos, id)]);
    advance(state, 4);
}

#[test]
fn copper_bulb_rising_edges_toggle_once_and_falling_edges_keep_light() {
    for name in BULBS {
        let mut s = state();
        let bulb = place(
            &mut s,
            6,
            200,
            6,
            name,
            &[("lit", "false"), ("powered", "false")],
        );
        set_source(&mut s, true);
        assert_eq!(value(&s, bulb, "lit"), "true", "{name}: first rising edge");
        assert_eq!(value(&s, bulb, "powered"), "true", "{name}: powered");
        // An unrelated neighbour update while power stays high must not toggle again.
        set_source(&mut s, true);
        assert_eq!(value(&s, bulb, "lit"), "true", "{name}: sustained high");
        set_source(&mut s, false);
        assert_eq!(value(&s, bulb, "lit"), "true", "{name}: falling edge");
        assert_eq!(value(&s, bulb, "powered"), "false", "{name}: unpowered");
        set_source(&mut s, true);
        assert_eq!(
            value(&s, bulb, "lit"),
            "false",
            "{name}: second rising edge"
        );
        assert_eq!(value(&s, bulb, "powered"), "true", "{name}: powered again");
        set_source(&mut s, false);
        assert_eq!(
            value(&s, bulb, "lit"),
            "false",
            "{name}: second falling edge"
        );
        assert_eq!(
            value(&s, bulb, "powered"),
            "false",
            "{name}: unpowered again"
        );
    }
}

#[test]
fn copper_bulb_comparator_reads_lit_independently_of_powered() {
    for name in BULBS {
        for powered in ["false", "true"] {
            let mut s = state();
            let bulb = place(
                &mut s,
                6,
                200,
                6,
                name,
                &[("lit", "true"), ("powered", powered)],
            );
            let comparator = place(
                &mut s,
                5,
                200,
                6,
                "comparator",
                &[("facing", "east"), ("mode", "compare")],
            );
            assert_eq!(
                signal::analog(&s, bulb),
                Some(15),
                "{name}, powered={powered}"
            );
            wake(&mut s, comparator);
            advance(&mut s, 4);
            assert_eq!(
                comparator_output(&s, comparator),
                15,
                "{name}, powered={powered}"
            );
            assert_eq!(value(&s, comparator, "powered"), "true");

            let dark = place(
                &mut s,
                6,
                200,
                6,
                name,
                &[("lit", "false"), ("powered", powered)],
            );
            assert_eq!(
                signal::analog(&s, dark),
                Some(0),
                "{name}, powered={powered}"
            );
            let id = s.world.block_at(dark.x, dark.y, dark.z).unwrap();
            neighbours(&mut s, &[(dark, id)]);
            advance(&mut s, 4);
            assert_eq!(
                comparator_output(&s, comparator),
                0,
                "{name}, powered={powered}"
            );
            assert_eq!(value(&s, comparator, "powered"), "false");
        }
    }
}

#[test]
fn copper_bulb_edge_updates_comparator_and_dust() {
    let mut s = state();
    let bulb = place(
        &mut s,
        6,
        200,
        6,
        "copper_bulb",
        &[("lit", "false"), ("powered", "false")],
    );
    let comparator = place(&mut s, 5, 200, 6, "comparator", &[("facing", "east")]);
    place(&mut s, 4, 199, 6, "stone", &[]);
    let dust = place(&mut s, 4, 200, 6, "redstone_wire", &[]);
    let source = place(&mut s, 7, 200, 6, "redstone_block", &[]);
    let id = s.world.block_at(source.x, source.y, source.z).unwrap();
    neighbours(&mut s, &[(source, id)]);
    advance(&mut s, 8);
    assert_eq!(value(&s, bulb, "lit"), "true");
    assert_eq!(comparator_output(&s, comparator), 15);
    assert_eq!(value(&s, dust, "power"), "15");

    let air = place(&mut s, 7, 200, 6, "air", &[]);
    let id = s.world.block_at(air.x, air.y, air.z).unwrap();
    neighbours(&mut s, &[(air, id)]);
    advance(&mut s, 8);
    assert_eq!(value(&s, bulb, "powered"), "false");
    assert_eq!(value(&s, dust, "power"), "15");

    let source = place(&mut s, 7, 200, 6, "redstone_block", &[]);
    let id = s.world.block_at(source.x, source.y, source.z).unwrap();
    neighbours(&mut s, &[(source, id)]);
    advance(&mut s, 8);
    assert_eq!(value(&s, bulb, "lit"), "false");
    assert_eq!(comparator_output(&s, comparator), 0);
    assert_eq!(value(&s, dust, "power"), "0");
}
