use super::{flat_player_fixture, run_tick};
use crate::boundary::{ConnectionId, PlayerChange, PlayerInput};
use crate::effects::{ActiveEffect, EffectId};
use mc_data::{Identifier, ItemStack};

fn movement(position: [f64; 3]) -> PlayerInput {
    PlayerInput::Move {
        connection: ConnectionId(1),
        position,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
    }
}

fn kicked(changes: &[(ConnectionId, Vec<PlayerChange>)]) -> bool {
    changes.iter().flat_map(|(_, changes)| changes).any(|change| {
        matches!(change, PlayerChange::Disconnect { reason } if reason == "Flying is not enabled on this server")
    })
}

#[test]
fn physics_landing_blocks_reduce_damage_without_trusting_ground_claims() {
    for (name, expected) in [("hay_block", 4.0), ("slime_block", 0.0), ("stone", 20.0)] {
        let (mut state, player, pos) = flat_player_fixture();
        let block = state
            .registry
            .blocks
            .block(&Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap()
            .default
            .0;
        state.world.set_block(
            pos[0].floor() as i32,
            pos[1].floor() as i32 - 1,
            pos[2].floor() as i32,
            block,
        );
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.set_pos(slot, [pos[0], pos[1] + 0.5, pos[2]]);
        state.players.get_mut(&player).unwrap().fall_distance = 22.5;
        state.entities.row_mut(slot).protected_until = 0;
        run_tick(&mut state, &[movement([pos[0], pos[1] - 0.1, pos[2]])]);
        assert_eq!(
            state.players[&player].vitals.health,
            20.0 - expected,
            "{name}"
        );
        assert!(
            state.entities.ground(slot),
            "support, not the false client flag"
        );
    }
}

#[test]
fn physics_water_eye_sample_uses_pose_and_flow_height() {
    let (mut state, _, pos) = flat_player_fixture();
    let x = pos[0].floor() as i32;
    let y = pos[1].floor() as i32;
    let z = pos[2].floor() as i32;
    state
        .world
        .set_block(x, y + 1, z, state.registry.states.water);
    let feet = [pos[0], y as f64 + 0.5, pos[2]];
    let eye = |pose| {
        [
            feet[0],
            feet[1] + crate::tick::player_eye_height(pose),
            feet[2],
        ]
    };
    assert!(crate::fluids::water_at_point(&state, eye(5)));
    assert!(!crate::fluids::water_at_point(&state, eye(0)));
    state
        .world
        .set_block(x, y + 1, z, state.registry.states.water_levels[7]);
    assert!(!crate::fluids::water_at_point(&state, eye(5)));
}

#[test]
fn physics_flowing_water_cancels_landing_damage() {
    let (mut state, player, pos) = flat_player_fixture();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [pos[0], pos[1] + 0.5, pos[2]]);
    state.players.get_mut(&player).unwrap().fall_distance = 23.0;
    state.world.set_block(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
        state.registry.states.water_levels[3],
    );
    state.world.fold_overlay();
    state.entities.row_mut(slot).protected_until = 0;
    run_tick(&mut state, &[movement(pos)]);
    assert_eq!(state.players[&player].vitals.health, 20.0);
    assert_eq!(state.players[&player].fall_distance, 0.0);
}

#[test]
fn physics_swimming_charges_water_distance_not_land_sprint() {
    for sprinting in [false, true] {
        let (mut state, player, pos) = flat_player_fixture();
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.set_ground(slot, false);
        state.players.get_mut(&player).unwrap().sprinting = sprinting;
        let target = [pos[0] + 0.2, pos[1] + 0.2, pos[2]];
        state.world.set_block(
            target[0].floor() as i32,
            target[1].floor() as i32,
            target[2].floor() as i32,
            state.registry.states.water,
        );
        let before = state.players[&player].vitals.exhaustion;
        run_tick(&mut state, &[movement(target)]);
        let added = state.players[&player].vitals.exhaustion - before;
        assert!(
            (added - (0.08_f32).sqrt() * 0.01).abs() < 1e-5,
            "{sprinting}: {added}"
        );
    }
}

#[test]
fn unsupported_non_descending_airtime_corrects_then_kicks() {
    let (mut state, player, pos) = flat_player_fixture();
    state.max_airtime_ticks = 3;
    let mut changes = Vec::new();
    for y in [pos[1] + 0.25, pos[1] + 0.5, pos[1] + 0.75] {
        let (_, drained, _) = run_tick(&mut state, &[movement([pos[0], y, pos[2]])]);
        changes.extend(drained);
    }
    let slot = state.entities.slot_of(player).unwrap();
    assert_eq!(
        state.entities.pos(slot),
        pos,
        "set back to the supported position"
    );
    assert!(kicked(&changes), "sustained hover is kicked");
    assert!(changes.iter().flat_map(|(_, changes)| changes).any(
        |change| matches!(change, PlayerChange::PositionCorrected { position, .. } if *position == pos)
    ));
}

#[test]
fn ordinary_jump_and_fall_do_not_accumulate_to_a_kick() {
    let (mut state, _, pos) = flat_player_fixture();
    let mut changes = Vec::new();
    for y in [
        pos[1] + 0.25,
        pos[1] + 0.5,
        pos[1] + 0.75,
        pos[1] + 0.5,
        pos[1] + 0.25,
        pos[1],
    ] {
        let (_, drained, _) = run_tick(&mut state, &[movement([pos[0], y, pos[2]])]);
        changes.extend(drained);
    }
    assert!(!kicked(&changes));
}

fn grounded_movement(position: [f64; 3]) -> PlayerInput {
    PlayerInput::Move {
        connection: ConnectionId(1),
        position,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
    }
}

/// Client-integrated jump (vanilla gravity/drag): up 1.25 blocks, then down
/// until the client's own collision stops it exactly on the floor.
fn jump_positions(ground: f64) -> Vec<([f64; 3], bool)> {
    let mut y = ground;
    let mut velocity: f64 = 0.42;
    let mut moves = Vec::new();
    loop {
        y += velocity;
        velocity = (velocity - 0.08) * 0.98;
        let landed = y <= ground;
        if landed {
            y = ground;
        }
        moves.push((y, landed));
        if landed {
            return moves.into_iter().map(|(y, g)| ([0.0, y, 0.0], g)).collect();
        }
    }
}

#[test]
fn consecutive_jumps_on_flat_ground_deal_no_fall_damage() {
    let (mut state, player, pos) = flat_player_fixture();
    state.max_airtime_ticks = 1000;
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    for jump in 0..5 {
        for (offset, landed) in jump_positions(pos[1]) {
            let at = [pos[0], offset[1], pos[2]];
            let input = if landed {
                grounded_movement(at)
            } else {
                movement(at)
            };
            let (_, changes, _) = run_tick(&mut state, &[input]);
            assert!(!kicked(&changes), "jump {jump}");
        }
        assert_eq!(state.players[&player].vitals.health, 20.0, "jump {jump}");
        assert_eq!(state.players[&player].fall_distance, 0.0, "jump {jump}");
        assert!(state.entities.ground(slot), "jump {jump} lands grounded");
    }
}

#[test]
fn four_block_fall_landing_exactly_on_the_floor_still_damages() {
    let (mut state, player, pos) = flat_player_fixture();
    state.max_airtime_ticks = 1000;
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    state.entities.set_pos(slot, [pos[0], pos[1] + 4.0, pos[2]]);
    state.entities.set_ground(slot, false);
    run_tick(&mut state, &[grounded_movement(pos)]);
    assert_eq!(state.players[&player].vitals.health, 19.0);
    assert_eq!(state.players[&player].fall_distance, 0.0);
}

#[test]
fn permitted_airborne_states_do_not_trip_airtime() {
    for kind in [
        "water",
        "ladder",
        "elytra",
        "levitation",
        "slow_falling",
        "knockback",
        "slime",
        "solid_floor",
        "edge_support",
        "boat",
    ] {
        let (mut state, player, pos) = flat_player_fixture();
        state.max_airtime_ticks = 1;
        let slot = state.entities.slot_of(player).unwrap();
        match kind {
            "water" => state.world.set_block(
                pos[0].floor() as i32,
                pos[1].floor() as i32,
                pos[2].floor() as i32,
                state.registry.states.water,
            ),
            "ladder" => {
                let ladder = state
                    .registry
                    .blocks
                    .block(&Identifier::parse("minecraft:ladder").unwrap())
                    .unwrap()
                    .default
                    .0;
                state.world.set_block(
                    pos[0].floor() as i32,
                    pos[1].floor() as i32,
                    pos[2].floor() as i32,
                    ladder,
                );
            }
            "elytra" => {
                let elytra = state
                    .registry
                    .items
                    .id_of(&Identifier::parse("minecraft:elytra").unwrap())
                    .unwrap();
                state.players.get_mut(&player).unwrap().inventory.slots[38] =
                    ItemStack::new(elytra, 1);
                state.players.get_mut(&player).unwrap().fall_flying = true;
                state.entities.set_ground(slot, false);
            }
            "levitation" => {
                state
                    .players
                    .get_mut(&player)
                    .unwrap()
                    .give_effect(ActiveEffect::new(EffectId::Levitation, 20, 0));
            }
            "slow_falling" => {
                state
                    .players
                    .get_mut(&player)
                    .unwrap()
                    .give_effect(ActiveEffect::new(EffectId::SlowFalling, 20, 0));
            }
            "knockback" => state.entities.set_vel(slot, [0.0, 0.5, 0.0]),
            "slime" => {
                let slime = state
                    .registry
                    .blocks
                    .block(&Identifier::parse("minecraft:slime_block").unwrap())
                    .unwrap()
                    .default
                    .0;
                state.world.set_block(
                    pos[0].floor() as i32,
                    pos[1].floor() as i32 - 1,
                    pos[2].floor() as i32,
                    slime,
                );
            }
            "solid_floor" => {}
            "edge_support" => {
                let x = pos[0].floor() as i32;
                let y = pos[1].floor() as i32 - 1;
                let z = pos[2].floor() as i32;
                let obsidian = state
                    .registry
                    .blocks
                    .block(&Identifier::parse("minecraft:obsidian").unwrap())
                    .unwrap()
                    .default
                    .0;
                state.world.set_block(x, y, z, state.registry.states.air);
                state.world.set_block(x - 1, y, z, obsidian);
            }
            "boat" => {
                let uuid = state.entities.row(slot).uuid;
                let row = state.entities.row_mut(slot);
                row.entity_life.mount.vehicle = Some(player);
                row.entity_life.mount.rider = Some(uuid);
            }
            _ => unreachable!(),
        }
        state.world.fold_overlay();
        state.entities.set_ground(slot, false);
        let y = if matches!(kind, "slime" | "solid_floor" | "edge_support") {
            pos[1]
        } else {
            pos[1] + 0.25
        };
        let x = if kind == "edge_support" {
            pos[0].floor() + 0.1
        } else {
            pos[0]
        };
        let (_, changes, _) = run_tick(&mut state, &[movement([x, y, pos[2]])]);
        assert!(!kicked(&changes), "{kind}");
    }
}
