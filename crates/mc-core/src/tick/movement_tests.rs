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
