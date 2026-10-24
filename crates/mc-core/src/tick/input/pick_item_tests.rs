use crate::boundary::{BlockPos, ConnectionId, EntityId, PlayerChange, PlayerInput};
use crate::commands::GameMode;
use crate::tick::{TickState, run_tick};
use mc_data::ItemStack;

fn item(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
}

fn count(state: &TickState, player: EntityId, id: u32) -> i32 {
    state.players[&player]
        .inventory
        .slots
        .iter()
        .filter(|stack| stack.item_id == id)
        .map(|stack| stack.count)
        .sum()
}

/// Pick the platform stone under the player's feet; true when the client was
/// told its held slot.
fn pick_stone(state: &mut TickState, centre: [f64; 3]) -> bool {
    let (_, drains, _) = run_tick(
        state,
        &[PlayerInput::PickItemFromBlock {
            connection: ConnectionId(1),
            position: BlockPos {
                x: centre[0] as i32,
                y: 200,
                z: centre[2] as i32,
            },
        }],
    );
    drains
        .iter()
        .flat_map(|(_, changes)| changes)
        .any(|change| matches!(change, PlayerChange::HeldSlot { .. }))
}

#[test]
fn survival_pick_moves_an_existing_stack_into_the_hotbar_without_creating_items() {
    let (mut state, player, centre) = crate::hostile_tests::setup(1.0);
    let (stone, dirt) = (item(&state, "stone"), item(&state, "dirt"));
    let inventory = &mut state.players.get_mut(&player).unwrap().inventory;
    inventory.slots[0] = ItemStack::new(dirt, 5);
    inventory.slots[20] = ItemStack::new(stone, 7);
    assert!(pick_stone(&mut state, centre));
    let inventory = &state.players[&player].inventory;
    assert_eq!(inventory.selected, 1, "first empty hotbar slot");
    assert_eq!(inventory.slots[1], ItemStack::new(stone, 7));
    assert!(inventory.slots[20].is_empty());
    assert_eq!(count(&state, player, dirt), 5);
    // A hotbar match is only selected.
    state.players.get_mut(&player).unwrap().select_slot(0);
    assert!(pick_stone(&mut state, centre));
    assert_eq!(state.players[&player].inventory.selected, 1);
    assert_eq!(count(&state, player, stone), 7);
}

#[test]
fn survival_pick_without_a_match_changes_nothing() {
    let (mut state, player, centre) = crate::hostile_tests::setup(1.0);
    let stone = item(&state, "stone");
    assert!(!pick_stone(&mut state, centre));
    assert_eq!(count(&state, player, stone), 0);
    assert_eq!(state.players[&player].inventory.selected, 0);
}

#[test]
fn creative_pick_adds_one_item_and_keeps_the_displaced_stack() {
    let (mut state, player, centre) = crate::hostile_tests::setup(1.0);
    let (stone, dirt) = (item(&state, "stone"), item(&state, "dirt"));
    let player_state = state.players.get_mut(&player).unwrap();
    player_state.game_mode = GameMode::Creative;
    for slot in 0..9 {
        player_state.inventory.slots[slot] = ItemStack::new(dirt, 1);
    }
    assert!(pick_stone(&mut state, centre));
    let inventory = &state.players[&player].inventory;
    assert_eq!(inventory.selected, 0, "no empty or enchanted choice");
    assert_eq!(inventory.slots[0], ItemStack::new(stone, 1));
    assert_eq!(count(&state, player, dirt), 9, "displaced to a free slot");
}

#[test]
fn creative_entity_pick_gives_the_spawn_egg_and_survival_does_not() {
    for (mode, eggs) in [(GameMode::Survival, 0), (GameMode::Creative, 1)] {
        let (mut state, player, centre) = crate::hostile_tests::setup(1.0);
        state.players.get_mut(&player).unwrap().game_mode = mode;
        let cow = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
            .unwrap();
        let target = state.spawn_mob(cow, [centre[0] + 1.0, centre[1], centre[2]], 10.0);
        run_tick(
            &mut state,
            &[PlayerInput::PickItemFromEntity {
                connection: ConnectionId(1),
                target,
            }],
        );
        let egg = item(&state, "cow_spawn_egg");
        assert_eq!(count(&state, player, egg), eggs, "{mode:?}");
    }
}
