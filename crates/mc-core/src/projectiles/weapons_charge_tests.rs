use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::tick::run_tick;

fn stack(state: &TickState, name: &str) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap(),
        1,
    )
}
fn fixture(hand: Hand) -> (TickState, EntityId) {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let bow = stack(&state, "minecraft:bow");
    let arrow = stack(&state, "minecraft:arrow");
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.slots[if hand == Hand::Main { 0 } else { OFF_HAND }] = bow;
    player.inventory.slots[9] = arrow;
    assert!(use_item(&mut state, actor, hand));
    (state, actor)
}
fn released(state: &TickState, actor: EntityId) {
    assert!(!state.projectiles.using.contains_key(&actor));
    let slot = state.entities.slot_of(actor).unwrap();
    assert!(
        state
            .entities
            .row(slot)
            .appearance
            .extra
            .iter()
            .any(|(id, value)| { *id == 8 && matches!(value, EntityDataValue::Byte(0)) })
    );
    assert_eq!(
        state.players[&actor].raised_shield(&state.registry.item_rules()),
        None
    );
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
    assert!(
        state
            .entities
            .slots()
            .all(|slot| state.entities.row(slot).projectile.is_none())
    );
}
fn shield_input() -> PlayerInput {
    PlayerInput::UseItem {
        connection: ConnectionId(1),
        hand: Hand::Main,
        yaw: 0.0,
        pitch: 0.0,
        sequence: 1,
    }
}

#[test]
fn weapon_charge_select_then_shield_then_release_same_tick_clears_both_uses() {
    let (mut state, actor) = fixture(Hand::Main);
    let shield = stack(&state, "minecraft:shield");
    state.players.get_mut(&actor).unwrap().inventory.slots[1] = shield;
    run_tick(
        &mut state,
        &[
            PlayerInput::SelectSlot {
                connection: ConnectionId(1),
                slot: 1,
            },
            shield_input(),
            PlayerInput::ReleaseUseItem {
                connection: ConnectionId(1),
            },
        ],
    );
    for _ in 0..6 {
        run_tick(&mut state, &[]);
    }
    released(&state, actor);
}

#[test]
fn weapon_charge_swap_then_shield_then_release_same_tick_clears_both_uses() {
    let (mut state, actor) = fixture(Hand::Main);
    let shield = stack(&state, "minecraft:shield");
    state.players.get_mut(&actor).unwrap().inventory.slots[OFF_HAND] = shield;
    run_tick(
        &mut state,
        &[
            PlayerInput::SwapHands {
                connection: ConnectionId(1),
            },
            shield_input(),
            PlayerInput::ReleaseUseItem {
                connection: ConnectionId(1),
            },
        ],
    );
    for _ in 0..6 {
        run_tick(&mut state, &[]);
    }
    released(&state, actor);
}

#[test]
fn weapon_charge_invalid_or_unchanged_selection_preserves_charge() {
    let (mut state, actor) = fixture(Hand::Main);
    for slot in [0, 9, u8::MAX] {
        run_tick(
            &mut state,
            &[PlayerInput::SelectSlot {
                connection: ConnectionId(1),
                slot,
            }],
        );
        assert!(state.projectiles.using.contains_key(&actor));
        assert_eq!(state.players[&actor].inventory.selected, 0);
    }
}

#[test]
fn weapon_charge_main_selection_preserves_offhand_charge_but_swap_cancels_it() {
    let (mut state, actor) = fixture(Hand::Off);
    run_tick(
        &mut state,
        &[PlayerInput::SelectSlot {
            connection: ConnectionId(1),
            slot: 1,
        }],
    );
    assert!(state.projectiles.using.contains_key(&actor));
    run_tick(
        &mut state,
        &[PlayerInput::SwapHands {
            connection: ConnectionId(1),
        }],
    );
    released(&state, actor);
}
