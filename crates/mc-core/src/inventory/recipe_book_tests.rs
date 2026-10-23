use mc_data::ItemStack;

use super::MenuKind;
use super::click_tests::{LOG, PLANKS, STONE, Session, stack};

const PLANKS_RECIPE: u32 = 0;
const SHAPED_2X2: u32 = 3;

fn place(s: &mut Session, recipe: u32, make_all: bool) {
    let rules = s.data.rules();
    s.menu
        .place_recipe(&rules, &mut s.inventory, recipe, make_all, &mut []);
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

#[test]
fn cooking_books_match_station_kind_and_keep_non_input_custody() {
    use mc_data::recipes::CookingKind;
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    for (kind, recipe_name, ingredient) in [
        (
            CookingKind::Furnace,
            "iron_ingot_from_smelting_raw_iron",
            "raw_iron",
        ),
        (
            CookingKind::BlastFurnace,
            "iron_ingot_from_blasting_raw_iron",
            "raw_iron",
        ),
        (CookingKind::Smoker, "cooked_beef_from_smoking", "beef"),
    ] {
        let item = |name: &str, count| {
            ItemStack::new(
                registry
                    .items
                    .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
                    .unwrap(),
                count,
            )
        };
        let recipe = registry
            .recipes
            .iter()
            .position(|r| r.id.path() == recipe_name)
            .unwrap() as u32;
        let mut menu = super::Menu::new(1, MenuKind::Furnace(kind));
        let mut inventory = super::PlayerInventory::default();
        inventory.slots[0] = item(ingredient, 5);
        let mut container = [ItemStack::EMPTY, item("coal", 3), item("iron_ingot", 2)];
        let retained = [container[1].clone(), container[2].clone()];
        menu.place_recipe(
            &registry.item_rules(),
            &mut inventory,
            recipe,
            false,
            &mut container,
        );
        assert_eq!(container[0], item(ingredient, 1));
        menu.place_recipe(
            &registry.item_rules(),
            &mut inventory,
            recipe,
            false,
            &mut container,
        );
        assert_eq!(container[0], item(ingredient, 2));
        menu.place_recipe(
            &registry.item_rules(),
            &mut inventory,
            recipe,
            true,
            &mut container,
        );
        assert_eq!(container[0], item(ingredient, 5));
        assert_eq!(&container[1..], &retained);
        assert!(inventory.slots[0].is_empty());
    }
}
