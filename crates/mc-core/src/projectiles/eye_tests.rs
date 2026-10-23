use super::flight_tests::setup;
use crate::boundary::Hand;
use mc_data::{Identifier, ItemStack};

#[test]
fn physics_eye_without_stronghold_is_handled_without_consumption() {
    let (mut state, player, _) = setup();
    let item = state.registry.items.id_of(&Identifier::parse("minecraft:ender_eye").unwrap()).unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = ItemStack::new(item, 2);
    assert!(super::weapons::use_item(&mut state, player, Hand::Main));
    assert_eq!(state.players[&player].inventory.slots[0].count, 2);
    assert!(state.projectiles.spawned.is_empty());
}
