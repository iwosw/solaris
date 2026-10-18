use mc_data::ItemStack;

use super::MenuKind;
use super::click_tests::{LOG, PLANKS, STONE, Session, stack};

const PLANKS_RECIPE: u32 = 0;
const SHAPED_2X2: u32 = 3;

fn place(s: &mut Session, recipe: u32, make_all: bool) {
    let rules = s.data.rules();
    s.menu
        .place_recipe(&rules, &mut s.inventory, recipe, make_all);
}

#[test]
fn clicking_a_recipe_places_one_craft_and_again_adds_one_more() {
    let mut s = Session::new(MenuKind::Player);
    s.inventory.slots[3] = stack(LOG, 3);
    place(&mut s, PLANKS_RECIPE, false);
    assert_eq!(s.menu.grid[0], stack(LOG, 1));
    assert_eq!(s.menu.result, stack(PLANKS, 4));
    assert_eq!(s.inventory.slots[3].count, 2);
    place(&mut s, PLANKS_RECIPE, false);
    assert_eq!(s.menu.grid[0], stack(LOG, 2));
    assert_eq!(s.inventory.slots[3].count, 1);
}

#[test]
fn shift_clicking_a_recipe_places_as_many_crafts_as_possible() {
    let mut s = Session::new(MenuKind::CraftingTable);
    s.inventory.slots[0] = stack(LOG, 40);
    s.inventory.slots[20] = stack(LOG, 30);
    place(&mut s, PLANKS_RECIPE, true);
    assert_eq!(s.menu.grid[0], stack(LOG, 64));
    let left: i32 = s
        .inventory
        .slots
        .iter()
        .filter(|x| x.item_id == LOG)
        .map(|x| x.count)
        .sum();
    assert_eq!(left, 6);
}

#[test]
fn shaped_recipe_fills_its_pattern() {
    let mut s = Session::new(MenuKind::Player);
    s.inventory.slots[9] = stack(STONE, 10);
    place(&mut s, SHAPED_2X2, false);
    assert!(s.menu.grid.iter().all(|cell| *cell == stack(STONE, 1)));
    assert_eq!(s.inventory.slots[9].count, 6);
    assert!(!s.menu.result.is_empty());
}

#[test]
fn unaffordable_recipe_returns_the_grid_and_leaves_it_empty() {
    let mut s = Session::new(MenuKind::Player);
    s.menu.grid[2] = stack(STONE, 2);
    place(&mut s, SHAPED_2X2, false);
    assert!(s.menu.grid.iter().all(ItemStack::is_empty));
    assert!(s.inventory.slots.contains(&stack(STONE, 2)));
}

#[test]
fn renamed_items_are_never_pulled_into_a_recipe() {
    let mut s = Session::new(MenuKind::Player);
    let mut named = stack(LOG, 5);
    named.custom_name = Some("Heirloom".into());
    s.inventory.slots[0] = named.clone();
    place(&mut s, PLANKS_RECIPE, false);
    assert!(s.menu.grid.iter().all(ItemStack::is_empty));
    assert_eq!(s.inventory.slots[0], named);
}

#[test]
fn placing_never_merges_stacks_of_different_components() {
    let mut s = Session::new(MenuKind::Player);
    let mut modeled = stack(LOG, 1);
    modeled.item_model = Some(
        mc_data::Identifier::parse("test:carved_log")
            .unwrap()
            .into(),
    );
    s.inventory.slots[0] = stack(LOG, 1);
    s.inventory.slots[1] = modeled.clone();
    place(&mut s, PLANKS_RECIPE, true);
    assert_eq!(s.menu.grid[0], stack(LOG, 1));
    assert_eq!(s.inventory.slots[1], modeled);
}
