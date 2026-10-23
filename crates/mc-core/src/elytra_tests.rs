use super::{flat_player_fixture, run_tick};
use crate::boundary::{ConnectionId, EntityId, PlayerChange, PlayerInput};
use crate::effects::{ActiveEffect, EffectId};
use crate::tick::TickState;
use mc_data::{Identifier, ItemEnchantment, ItemStack};

fn fixture() -> (TickState, EntityId, [f64; 3]) {
    let (mut state, player, mut pos) = flat_player_fixture();
    let elytra = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:elytra").unwrap())
            .unwrap(),
        1,
    );
    state.players.get_mut(&player).unwrap().inventory.slots[38] = elytra;
    pos[1] += 20.0;
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, pos);
    state.entities.set_ground(slot, false);
    state.entities.row_mut(slot).protected_until = 0;
    (state, player, pos)
}

fn start(state: &mut TickState) {
    run_tick(
        state,
        &[PlayerInput::StartFallFlying {
            connection: ConnectionId(1),
        }],
    );
}

fn support_landing(state: &mut TickState, player: EntityId) {
    let slot = state.entities.slot_of(player).unwrap();
    let mut pos = state.entities.pos(slot);
    pos[1] = pos[1].floor();
    state.entities.set_pos(slot, pos);
    state.world.set_block(
        pos[0].floor() as i32,
        pos[1] as i32 - 1,
        pos[2].floor() as i32,
        state.registry.states.stone,
    );
    state.world.fold_overlay();
}

#[test]
fn elytra_start_checks_airborne_equipment_fluid_riding_and_levitation() {
    for condition in [
        "valid",
        "ground",
        "missing",
        "wrong_item",
        "broken",
        "water",
        "lava",
        "bubble_column",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
        "riding",
        "levitation",
        "dead",
        "spectator",
    ] {
        let (mut state, player, pos) = fixture();
        let slot = state.entities.slot_of(player).unwrap();
        match condition {
            "ground" => state.entities.set_ground(slot, true),
            "missing" => {
                state.players.get_mut(&player).unwrap().inventory.slots[38] = ItemStack::EMPTY
            }
            "wrong_item" => {
                let item = state
                    .registry
                    .items
                    .id_of(&Identifier::parse("minecraft:diamond_chestplate").unwrap())
                    .unwrap();
                state.players.get_mut(&player).unwrap().inventory.slots[38] =
                    ItemStack::new(item, 1);
            }
            "broken" => {
                let stack = &state.players[&player].inventory.slots[38];
                let rules = state.registry.item_rules();
                let max = rules
                    .facts
                    .facts_for_stack(stack, rules.items)
                    .unwrap()
                    .max_damage
                    .unwrap();
                state.players.get_mut(&player).unwrap().inventory.slots[38].damage =
                    Some(max as i32 - 1);
            }
            "water" | "lava" | "bubble_column" | "kelp" | "kelp_plant" | "seagrass"
            | "tall_seagrass" => {
                let id = state
                    .registry
                    .blocks
                    .block(&Identifier::parse(format!("minecraft:{condition}")).unwrap())
                    .unwrap()
                    .default
                    .0;
                state.world.set_block(
                    pos[0].floor() as i32,
                    pos[1].floor() as i32,
                    pos[2].floor() as i32,
                    id,
                );
            }
            "riding" => {
                let uuid = state.entities.row(slot).uuid;
                state.entities.row_mut(slot).entity_life.mount.vehicle = Some(player);
                state.entities.row_mut(slot).entity_life.mount.rider = Some(uuid);
            }
            "levitation" => {
                state
                    .players
                    .get_mut(&player)
                    .unwrap()
                    .give_effect(ActiveEffect::new(EffectId::Levitation, 200, 0));
            }
            "dead" => {
                state.players.get_mut(&player).unwrap().vitals.health = 0.0;
            }
            "spectator" => {
                state.players.get_mut(&player).unwrap().game_mode =
                    crate::commands::GameMode::Spectator;
            }
            _ => {}
        }
        start(&mut state);
        assert_eq!(
            state.players[&player].fall_flying,
            condition == "valid",
            "{condition}"
        );
        let appearance = &state.entities.row(slot).appearance;
        assert_eq!(
            appearance.shared_flags & 0x80 != 0,
            condition == "valid",
            "{condition}"
        );
        assert_eq!(appearance.pose == 1, condition == "valid", "{condition}");
        if condition == "valid" {
            assert_eq!(state.entities.body(slot).height, 0.6);
            start(&mut state);
            assert_eq!(
                state.players[&player].fall_flying_ticks, 2,
                "duplicate start cannot restart wear timer"
            );
        }
    }
}

#[test]
fn elytra_wears_every_twenty_ticks_stops_when_unusable_and_respects_unbreaking() {
    let (mut state, player, _) = fixture();
    start(&mut state);
    for _ in 1..19 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(
        state.players[&player].inventory.slots[38]
            .damage
            .unwrap_or(0),
        0
    );
    run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].inventory.slots[38].damage, Some(1));
    for _ in 0..20 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.players[&player].inventory.slots[38].damage, Some(2));
    let rules = state.registry.item_rules();
    let max = rules
        .facts
        .facts_for_stack(&state.players[&player].inventory.slots[38], rules.items)
        .unwrap()
        .max_damage
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[38].damage = Some(max as i32 - 2);
    for _ in 0..20 {
        run_tick(&mut state, &[]);
    }
    assert!(!state.players[&player].fall_flying);
    assert_eq!(
        state.players[&player].inventory.slots[38].damage,
        Some(max as i32 - 1),
        "unusable elytra stays equipped rather than disappearing"
    );

    let (mut state, player, _) = fixture();
    let p = state.players.get_mut(&player).unwrap();
    p.inventory.slots[38].enchantments.push(ItemEnchantment {
        id: Identifier::parse("minecraft:unbreaking").unwrap(),
        level: 3,
    });
    let stack = p.inventory.slots[38].clone();
    let mut expected_seed = p.enchantment_seed;
    let expected_wear: i32 = (0..10)
        .map(|_| crate::enchantments::durability_damage(&stack, 1, false, &mut expected_seed))
        .sum();
    start(&mut state);
    for _ in 1..200 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(
        state.players[&player].inventory.slots[38]
            .damage
            .unwrap_or(0),
        expected_wear
    );
    assert_eq!(state.players[&player].enchantment_seed, expected_seed);
}

#[test]
fn elytra_fast_glide_is_not_corrected_and_shallow_descent_resets_fall_damage() {
    let (mut state, player, pos) = fixture();
    start(&mut state);
    let rocket = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:firework_rocket").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = ItemStack::new(rocket, 2);
    run_tick(
        &mut state,
        &[PlayerInput::UseItem {
            connection: ConnectionId(1),
            hand: crate::boundary::Hand::Main,
            yaw: 0.0,
            pitch: 0.0,
            sequence: 1,
        }],
    );
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
    state.players.get_mut(&player).unwrap().fall_distance = 100.0;
    let next = [pos[0] + 12.0, pos[1] - 0.2, pos[2]];
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Move {
            connection: ConnectionId(1),
            position: next,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }],
    );
    let slot = state.entities.slot_of(player).unwrap();
    assert_eq!(state.entities.pos(slot), next);
    assert!(
        !drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change, PlayerChange::PositionCorrected { .. }))
    );
    assert!(state.players[&player].fall_distance < 0.3);
    run_tick(
        &mut state,
        &[PlayerInput::MoveStatus {
            connection: ConnectionId(1),
            on_ground: true,
        }],
    );
    assert!(
        state.players[&player].fall_flying,
        "an unsupported ground claim is not a landing"
    );
    support_landing(&mut state, player);
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::MoveStatus {
            connection: ConnectionId(1),
            on_ground: true,
        }],
    );
    assert!(!state.players[&player].fall_flying);
    assert_eq!(state.entities.row(slot).appearance.shared_flags & 0x80, 0);
    assert_ne!(state.entities.row(slot).appearance.pose, 1);
    assert_eq!(state.entities.body(slot).height, 1.8);
    assert_eq!(state.players[&player].vitals.health, 20.0);
    assert!(
        !drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change, PlayerChange::PositionCorrected { .. }))
    );

    // Rotation/status packets have no vertical velocity sample. They must not
    // turn a steep dive into a shallow glide just by reporting zero displacement.
    let (mut state, player, pos) = fixture();
    start(&mut state);
    state.players.get_mut(&player).unwrap().fall_distance = 5.0;
    run_tick(
        &mut state,
        &[PlayerInput::Move {
            connection: ConnectionId(1),
            position: [pos[0], pos[1] - 1.0, pos[2]],
            yaw: 0.0,
            pitch: 80.0,
            on_ground: false,
        }],
    );
    run_tick(
        &mut state,
        &[PlayerInput::MoveRotation {
            connection: ConnectionId(1),
            yaw: 10.0,
            pitch: 80.0,
            on_ground: false,
        }],
    );
    assert_eq!(state.players[&player].fall_distance, 6.0);
    support_landing(&mut state, player);
    run_tick(
        &mut state,
        &[PlayerInput::MoveStatus {
            connection: ConnectionId(1),
            on_ground: true,
        }],
    );
    assert_eq!(state.players[&player].vitals.health, 17.0);
}

#[test]
fn elytra_stops_on_equipment_removal_or_entering_fluid() {
    for condition in [
        "missing",
        "water",
        "lava",
        "bubble_column",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
        "riding",
    ] {
        let (mut state, player, pos) = fixture();
        start(&mut state);
        let slot = state.entities.slot_of(player).unwrap();
        match condition {
            "missing" => {
                state.players.get_mut(&player).unwrap().inventory.slots[38] = ItemStack::EMPTY
            }
            "riding" => {
                let uuid = state.entities.row(slot).uuid;
                state.entities.row_mut(slot).entity_life.mount.vehicle = Some(player);
                state.entities.row_mut(slot).entity_life.mount.rider = Some(uuid);
            }
            _ => {
                let id = state
                    .registry
                    .blocks
                    .block(&Identifier::parse(format!("minecraft:{condition}")).unwrap())
                    .unwrap()
                    .default
                    .0;
                state.world.set_block(
                    pos[0].floor() as i32,
                    pos[1].floor() as i32,
                    pos[2].floor() as i32,
                    id,
                );
            }
        }
        run_tick(&mut state, &[]);
        assert!(!state.players[&player].fall_flying, "{condition}");
        assert_eq!(
            state.entities.row(slot).appearance.shared_flags & 0x80,
            0,
            "{condition}"
        );
    }
}

#[test]
fn elytra_rejoin_restores_flight_pose_and_worn_equipment() {
    let (mut state, player, pos) = fixture();
    let uuid = state.connections.uuid_of(ConnectionId(1)).unwrap();
    start(&mut state);
    for _ in 1..20 {
        run_tick(&mut state, &[]);
    }
    run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: ConnectionId(1),
        }],
    );
    assert!(state.offline_players[&uuid].fall_flying);
    assert_eq!(state.offline_players[&uuid].inventory[38].damage, Some(1));
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid,
            name: "glider".into(),
            permission_level: 0,
        }],
    );
    let restored = state.connections.entity_of(ConnectionId(2)).unwrap();
    assert_ne!(restored, player);
    assert!(state.players[&restored].fall_flying);
    let slot = state.entities.slot_of(restored).unwrap();
    assert_eq!(state.entities.pos(slot), pos);
    assert!(!state.entities.ground(slot));
    assert_eq!(state.entities.row(slot).appearance.pose, 1);
    assert_eq!(
        state.entities.row(slot).appearance.shared_flags & 0x80,
        0x80
    );
    assert_eq!(state.entities.body(slot).height, 0.6);
    assert_eq!(state.players[&restored].inventory.slots[38].damage, Some(1));
}
