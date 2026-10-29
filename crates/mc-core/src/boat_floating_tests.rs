use super::*;
use crate::boundary::PlayerChange;

fn mounted_boat(water: bool) -> (TickState, EntityId, EntityId) {
    let mut state = fixture();
    let actor = player(&mut state, [0.5, 65.8, -0.5]);
    if water {
        let source = state.registry.states.water;
        for x in -2..=2 {
            for z in -2..=2 {
                state.world.set_block(x, 65, z, source);
            }
        }
    }
    let boat = mob(&mut state, "oak_boat", [0.5, 65.8, 0.5]);
    run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: boat,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    let slot = state.entities.slot_of(boat).unwrap();
    assert_eq!(
        crate::mounts::rider(&state, slot),
        state.entities.slot_of(actor)
    );
    (state, actor, boat)
}

fn packet(y: f64, on_ground: bool) -> PlayerInput {
    PlayerInput::MoveVehicle {
        connection: ConnectionId(1),
        position: [0.5, y, 0.5],
        yaw: 0.0,
        pitch: 0.0,
        on_ground,
    }
}

fn flying_kick(changes: &[(ConnectionId, Vec<PlayerChange>)]) -> bool {
    changes.iter().flat_map(|(_, changes)| changes).any(|change| {
        matches!(change, PlayerChange::Disconnect { reason } if reason == "Flying is not enabled on this server")
    })
}

#[test]
fn boat_floating_hover_sequence_kicks_after_gravity_scaled_allowance() {
    for claimed_ground in [false, true] {
        let (mut state, _, boat) = mounted_boat(false);
        state.max_airtime_ticks = 80;
        let slot = state.entities.slot_of(boat).unwrap();
        state.entities.set_pos(slot, [0.5, 70.0, 0.5]);
        state.entities.set_vel(slot, [0.0; 3]);
        // Vanilla scales 80 ticks by 0.08 / AbstractBoat's 0.04 gravity.
        for tick in 1..=160 {
            let (_, changes, _) = run_tick(&mut state, &[packet(70.0, claimed_ground)]);
            assert!(
                !flying_kick(&changes),
                "boat allowance ends after tick 160, not {tick}"
            );
        }
        let (_, changes, _) = run_tick(&mut state, &[packet(70.0, claimed_ground)]);
        assert!(
            flying_kick(&changes),
            "repeated accepted hover packets must not permit indefinite boat flight (ground claim {claimed_ground})"
        );
        assert!(
            changes
                .iter()
                .flat_map(|(_, changes)| changes)
                .any(|change| { matches!(change, PlayerChange::VehicleCorrected { .. }) })
        );
    }
}

#[test]
fn boat_floating_accepted_air_packet_does_not_skip_native_gravity() {
    let (mut state, _, boat) = mounted_boat(false);
    let slot = state.entities.slot_of(boat).unwrap();
    state.entities.set_pos(slot, [0.5, 70.0, 0.5]);
    state.entities.set_vel(slot, [0.0; 3]);
    run_tick(&mut state, &[packet(70.0, true)]);
    assert!(
        state.entities.pos(slot)[1] < 70.0,
        "an unsupported accepted packet cannot consume native gravity, even with on_ground=true"
    );
}

#[test]
fn boat_floating_water_and_descent_do_not_accumulate_hover_time() {
    for water in [false, true] {
        let (mut state, _, boat) = mounted_boat(water);
        state.max_airtime_ticks = 2;
        let slot = state.entities.slot_of(boat).unwrap();
        if !water {
            state.entities.set_pos(slot, [0.5, 70.0, 0.5]);
        }
        for tick in 1..=12 {
            let y = if water {
                65.8
            } else {
                70.0 - f64::from(tick) * 0.1
            };
            let (_, changes, _) = run_tick(&mut state, &[packet(y, false)]);
            assert!(!flying_kick(&changes), "water={water}, tick={tick}");
            if water {
                assert_eq!(state.entities.pos(slot), [0.5, y, 0.5]);
            }
        }
    }
}

#[test]
fn boat_floating_real_ground_support_does_not_need_a_ground_claim() {
    for (name, properties, y) in [
        ("stone", &[][..], 65.0),
        (
            "stone_slab",
            &[("type", "bottom"), ("waterlogged", "false")][..],
            64.5,
        ),
    ] {
        let (mut state, _, boat) = mounted_boat(false);
        state.max_airtime_ticks = 2;
        let floor = block(&state, name, properties);
        for x in -2..=2 {
            for z in -2..=2 {
                state.world.set_block(x, 64, z, floor);
            }
        }
        let slot = state.entities.slot_of(boat).unwrap();
        state.entities.set_pos(slot, [0.5, y, 0.5]);
        for _ in 0..12 {
            let (_, changes, _) = run_tick(&mut state, &[packet(y, false)]);
            assert!(!flying_kick(&changes), "{name} supports the boat");
            assert_eq!(state.entities.pos(slot), [0.5, y, 0.5]);
        }
    }
}

#[test]
fn boat_floating_dismount_resets_connection_local_hover_count() {
    let (mut state, actor, boat) = mounted_boat(false);
    let slot = state.entities.slot_of(boat).unwrap();
    state.entities.set_pos(slot, [0.5, 70.0, 0.5]);
    for _ in 0..3 {
        run_tick(&mut state, &[packet(70.0, false)]);
    }
    assert_eq!(state.players[&actor].vehicle_airtime_ticks, 3);
    run_tick(
        &mut state,
        &[PlayerInput::Sneak {
            connection: ConnectionId(1),
            sneaking: true,
        }],
    );
    assert_eq!(state.players[&actor].vehicle_airtime_ticks, 0);
}

#[test]
fn boat_floating_accepted_air_packets_preserve_accumulating_gravity() {
    let (mut driven, _, boat) = mounted_boat(false);
    let (mut native, _, reference) = mounted_boat(false);
    let driven_slot = driven.entities.slot_of(boat).unwrap();
    let native_slot = native.entities.slot_of(reference).unwrap();
    driven.entities.set_pos(driven_slot, [0.5, 70.0, 0.5]);
    driven.entities.set_vel(driven_slot, [0.0; 3]);
    native.entities.set_pos(native_slot, [0.5, 70.0, 0.5]);
    native.entities.set_vel(native_slot, [0.0; 3]);
    for _ in 0..4 {
        let y = driven.entities.pos(driven_slot)[1];
        run_tick(&mut driven, &[packet(y, false)]);
        run_tick(&mut native, &[]);
        assert_eq!(
            driven.entities.vel(driven_slot)[1],
            native.entities.vel(native_slot)[1],
            "accepted packets must not erase vertical momentum before native gravity",
        );
        assert_eq!(
            driven.entities.pos(driven_slot),
            native.entities.pos(native_slot)
        );
    }
    let water = driven.registry.states.water;
    driven.world.set_block(0, 65, 0, water);
    run_tick(&mut driven, &[packet(65.8, false)]);
    assert_eq!(driven.entities.pos(driven_slot), [0.5, 65.8, 0.5]);
    assert_eq!(
        driven.entities.vel(driven_slot)[1],
        0.0,
        "water ends the native fall"
    );
}
