use mc_data::item_components::{ItemFacts, ItemFactsTable};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::recipes::{
    Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapedRecipe,
    ShapelessRecipe, SmeltingRecipe,
};
use mc_data::tags::TagsData;
use mc_data::{Identifier, ItemStack};

use super::{ItemRules, Menu, MenuKind, PlayerInventory, SlotRef};
use crate::boundary::ClickMode;

pub(super) const LOG: u32 = 1;
pub(super) const PLANKS: u32 = 2;
pub(super) const STONE: u32 = 3;
pub(super) const HELMET: u32 = 4;
pub(super) const WATER_BUCKET: u32 = 5;
pub(super) const BUCKET: u32 = 6;
pub(super) const SPONGE: u32 = 7;
pub(super) const COAL: u32 = 8;
pub(super) const LAVA_BUCKET: u32 = 9;
pub(super) const RAW_IRON: u32 = 10;
pub(super) const IRON_INGOT: u32 = 11;

fn id(name: &str) -> Identifier {
    Identifier::parse(format!("minecraft:{name}")).unwrap()
}

pub(super) struct Data {
    items: ItemRegistry,
    facts: ItemFactsTable,
    tags: TagsData,
    recipes: Vec<Recipe>,
}

impl Data {
    pub(super) fn new() -> Self {
        let names = [
            (LOG, "oak_log"),
            (PLANKS, "oak_planks"),
            (STONE, "stone"),
            (HELMET, "diamond_helmet"),
            (WATER_BUCKET, "water_bucket"),
            (BUCKET, "bucket"),
            (SPONGE, "sponge"),
            (COAL, "coal"),
            (LAVA_BUCKET, "lava_bucket"),
            (RAW_IRON, "raw_iron"),
            (IRON_INGOT, "iron_ingot"),
        ];
        let items = ItemRegistry::from_report(
            &names
                .iter()
                .map(|(protocol_id, name)| ItemReport {
                    id: id(name),
                    protocol_id: *protocol_id,
                })
                .collect::<Vec<_>>(),
        );
        let stackable = |max| ItemFacts {
            max_stack_size: Some(max),
            ..ItemFacts::default()
        };
        let facts = ItemFactsTable::from_entries([
            (id("oak_log"), stackable(64)),
            (id("oak_planks"), stackable(64)),
            (id("stone"), stackable(64)),
            (id("sponge"), stackable(64)),
            (id("coal"), stackable(64)),
            (id("raw_iron"), stackable(64)),
            (id("iron_ingot"), stackable(64)),
            (id("bucket"), stackable(16)),
            (id("water_bucket"), stackable(1)),
            (id("lava_bucket"), stackable(1)),
            (
                id("diamond_helmet"),
                ItemFacts {
                    max_stack_size: Some(1),
                    equippable_slot: Some("head".into()),
                    ..ItemFacts::default()
                },
            ),
        ]);
        let shapeless = |inputs: &[&str], output: &str, count: u32| Recipe {
            id: id(output),
            show_notification: true,
            kind: RecipeKind::Shapeless(ShapelessRecipe {
                ingredients: inputs
                    .iter()
                    .map(|name| Ingredient {
                        alternatives: vec![IngredientAlternative::Item(id(name))],
                    })
                    .collect(),
            }),
            result: RecipeResult {
                item: id(output),
                count,
                stew_effects: Vec::new(),
            },
        };
        let smelting = Recipe {
            id: id("iron_ingot_from_smelting_raw_iron"),
            show_notification: true,
            kind: RecipeKind::Smelting(SmeltingRecipe {
                ingredient: Ingredient {
                    alternatives: vec![IngredientAlternative::Item(id("raw_iron"))],
                },
                cooking_time: 200,
                experience_milli: 1000,
            }),
            result: RecipeResult {
                item: id("iron_ingot"),
                count: 1,
                stew_effects: Vec::new(),
            },
        };
        let shaped = Recipe {
            id: id("stone_bricks"),
            show_notification: true,
            kind: RecipeKind::Shaped(ShapedRecipe {
                pattern: vec!["SS".into(), "SS".into()],
                key: [(
                    'S',
                    Ingredient {
                        alternatives: vec![IngredientAlternative::Item(id("stone"))],
                    },
                )]
                .into_iter()
                .collect(),
            }),
            result: RecipeResult {
                item: id("sponge"),
                count: 4,
                stew_effects: Vec::new(),
            },
        };
        let tags = TagsData::default().with_vanilla_fuel_values(&items);
        Self {
            items,
            facts,
            tags,
            recipes: vec![
                shapeless(&["oak_log"], "oak_planks", 4),
                shapeless(&["water_bucket", "stone"], "sponge", 1),
                smelting,
                shaped,
            ],
        }
    }

    pub(super) fn rules(&self) -> ItemRules<'_> {
        ItemRules {
            items: &self.items,
            facts: &self.facts,
            tags: &self.tags,
            recipes: &self.recipes,
        }
    }
}

pub(super) fn stack(item: u32, count: i32) -> ItemStack {
    ItemStack::new(item, count)
}

pub(super) struct Session {
    pub(super) data: Data,
    pub(super) inventory: PlayerInventory,
    pub(super) container: Vec<ItemStack>,
    pub(super) menu: Menu,
    pub(super) dropped: Vec<ItemStack>,
}

impl Session {
    pub(super) fn new(kind: MenuKind) -> Self {
        Self {
            data: Data::new(),
            inventory: PlayerInventory::default(),
            container: vec![ItemStack::EMPTY; kind.container_size()],
            menu: Menu::new(u8::from(kind != MenuKind::Player), kind),
            dropped: Vec::new(),
        }
    }

    pub(super) fn click(&mut self, slot: i16, button: i8, mode: ClickMode) -> super::ClickOutcome {
        let state_id = self.menu.state_id;
        self.click_with_state(state_id, slot, button, mode)
    }

    fn click_with_state(
        &mut self,
        state_id: i32,
        slot: i16,
        button: i8,
        mode: ClickMode,
    ) -> super::ClickOutcome {
        let rules = self.data.rules();
        let outcome = self.menu.click(
            &rules,
            &mut self.inventory,
            &mut self.container,
            state_id,
            slot,
            button,
            mode,
            false,
        );
        self.dropped.extend(outcome.dropped.iter().cloned());
        outcome
    }

    pub(super) fn slot(&self, index: usize) -> ItemStack {
        self.menu.contents(&self.inventory, &self.container)[index].clone()
    }

    pub(super) fn put(&mut self, index: usize, item: ItemStack) {
        match self.menu.kind.slot(index).unwrap() {
            SlotRef::Inventory(i) => self.inventory.slots[i] = item,
            SlotRef::Container(i) => self.container[i] = item,
            SlotRef::Grid(i) => self.menu.grid[i] = item,
            SlotRef::Result => panic!("result is computed"),
        }
    }

    /// Items of `item` anywhere: slots, cursor and dropped.
    fn total(&self, item: u32) -> i32 {
        self.menu
            .contents(&self.inventory, &self.container)
            .iter()
            .enumerate()
            .filter(|(index, _)| self.menu.kind.slot(*index) != Some(SlotRef::Result))
            .map(|(_, stack)| stack)
            .chain(std::iter::once(&self.menu.carried))
            .chain(self.dropped.iter())
            .filter(|stack| stack.item_id == item && !stack.is_empty())
            .map(|stack| stack.count)
            .sum()
    }
}

#[test]
fn left_click_takes_whole_stack_and_right_click_takes_half_rounded_up() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(STONE, 7));
    s.click(9, 1, ClickMode::Pickup);
    assert_eq!(s.menu.carried.count, 4);
    assert_eq!(s.slot(9).count, 3);
    s.click(9, 0, ClickMode::Pickup);
    assert_eq!(s.slot(9).count, 7);
    assert!(s.menu.carried.is_empty());
}

#[test]
fn right_click_with_cursor_places_exactly_one() {
    let mut s = Session::new(MenuKind::Player);
    s.menu.carried = stack(STONE, 5);
    s.click(10, 1, ClickMode::Pickup);
    assert_eq!(s.slot(10).count, 1);
    assert_eq!(s.menu.carried.count, 4);
}

#[test]
fn left_drag_splits_evenly_and_keeps_the_remainder() {
    let mut s = Session::new(MenuKind::Player);
    s.menu.carried = stack(STONE, 10);
    s.click(-999, 0, ClickMode::QuickCraft);
    for slot in [9, 10, 11] {
        s.click(slot, 1, ClickMode::QuickCraft);
    }
    s.click(-999, 2, ClickMode::QuickCraft);
    assert_eq!(
        [s.slot(9).count, s.slot(10).count, s.slot(11).count],
        [3, 3, 3]
    );
    assert_eq!(s.menu.carried.count, 1);
}

#[test]
fn right_drag_places_one_per_slot() {
    let mut s = Session::new(MenuKind::Player);
    s.menu.carried = stack(STONE, 10);
    s.click(-999, 4, ClickMode::QuickCraft);
    for slot in [9, 10, 11, 12] {
        s.click(slot, 5, ClickMode::QuickCraft);
    }
    s.click(-999, 6, ClickMode::QuickCraft);
    assert_eq!(s.menu.carried.count, 6);
    assert!((9..13).all(|slot| s.slot(slot).count == 1));
}

#[test]
fn shift_click_from_chest_fills_the_rightmost_hotbar_slot_first() {
    let mut s = Session::new(MenuKind::Chest { rows: 3 });
    s.put(0, stack(STONE, 20));
    s.click(0, 0, ClickMode::QuickMove);
    assert!(s.slot(0).is_empty());
    assert_eq!(s.inventory.slots[8], stack(STONE, 20));
}

#[test]
fn shift_click_from_inventory_goes_into_the_chest() {
    let mut s = Session::new(MenuKind::Chest { rows: 1 });
    s.inventory.slots[0] = stack(STONE, 64);
    s.put(3, stack(STONE, 60));
    let hotbar_first = 9 + 27;
    s.click(
        i16::try_from(hotbar_first).unwrap(),
        0,
        ClickMode::QuickMove,
    );
    assert_eq!(s.container[3].count, 64);
    assert_eq!(s.container[0].count, 60);
    assert!(s.inventory.slots[0].is_empty());
}

#[test]
fn taking_the_crafting_result_consumes_one_craft() {
    let mut s = Session::new(MenuKind::Player);
    s.put(1, stack(LOG, 1));
    s.click(1, 0, ClickMode::Pickup);
    s.click(1, 0, ClickMode::Pickup);
    assert_eq!(s.slot(0), stack(PLANKS, 4));
    let outcome = s.click(0, 0, ClickMode::Pickup);
    assert_eq!(outcome.crafted_output, Some((PLANKS, 4)));
    assert_eq!(outcome.crafted, vec![(PLANKS, 4, MenuKind::Player)]);
    assert_eq!(s.menu.carried, stack(PLANKS, 4));
    assert!(s.slot(1).is_empty());
    assert!(s.slot(0).is_empty());
}

#[test]
fn shift_clicking_the_result_crafts_until_ingredients_run_out() {
    let mut s = Session::new(MenuKind::CraftingTable);
    s.put(5, stack(LOG, 3));
    s.click(5, 0, ClickMode::Pickup);
    s.click(5, 0, ClickMode::Pickup);
    let outcome = s.click(0, 0, ClickMode::QuickMove);
    assert_eq!(outcome.crafted_output, Some((PLANKS, 12)));
    assert_eq!(
        outcome.crafted,
        vec![(PLANKS, 4, MenuKind::CraftingTable); 3]
    );
    assert!(s.slot(5).is_empty());
    assert!(s.slot(0).is_empty());
    let planks: i32 = s
        .inventory
        .slots
        .iter()
        .filter(|stack| stack.item_id == PLANKS)
        .map(|stack| stack.count)
        .sum();
    assert_eq!(planks, 12);
}

#[test]
fn the_result_is_not_taken_when_the_cursor_cannot_hold_all_of_it() {
    let mut s = Session::new(MenuKind::Player);
    s.put(1, stack(LOG, 1));
    s.click(1, 0, ClickMode::Pickup);
    s.click(1, 0, ClickMode::Pickup);
    s.menu.carried = stack(PLANKS, 62);
    let outcome = s.click(0, 0, ClickMode::Pickup);
    assert_eq!(outcome.crafted_output, None);
    assert!(outcome.crafted.is_empty());
    assert_eq!(s.menu.carried.count, 62);
    assert_eq!(s.slot(1), stack(LOG, 1));
}

#[test]
fn throwing_crafting_results_credits_each_whole_output() {
    for (button, expected) in [(0, 4), (1, 12)] {
        let mut s = Session::new(MenuKind::CraftingTable);
        s.put(1, stack(LOG, 3));
        s.click(1, 0, ClickMode::Pickup);
        s.click(1, 0, ClickMode::Pickup);
        let outcome = s.click(0, button, ClickMode::Throw);
        assert_eq!(outcome.crafted_output, Some((PLANKS, expected)));
        assert_eq!(
            outcome.crafted,
            vec![(PLANKS, 4, MenuKind::CraftingTable); (expected / 4) as usize]
        );
        assert_eq!(
            outcome.manual_drops,
            vec![(PLANKS, 4); (expected / 4) as usize]
        );
        assert_eq!(
            outcome.dropped.iter().map(|stack| stack.count).sum::<i32>(),
            expected
        );
        assert_eq!(s.menu.grid[0].count, 3 - expected / 4);
        assert!(s.menu.carried.is_empty());
    }
}

#[test]
fn shift_crafting_credits_the_whole_output_including_dropped_remainder() {
    let mut s = Session::new(MenuKind::Player);
    s.put(1, stack(LOG, 2));
    s.click(1, 0, ClickMode::Pickup);
    s.click(1, 0, ClickMode::Pickup);
    s.inventory.slots[..36].fill(stack(STONE, 64));
    s.inventory.slots[8] = stack(PLANKS, 63);
    let outcome = s.click(0, 0, ClickMode::QuickMove);
    assert_eq!(outcome.crafted_output, Some((PLANKS, 4)));
    assert_eq!(s.inventory.slots[8], stack(PLANKS, 64));
    assert_eq!(outcome.dropped, vec![stack(PLANKS, 3)]);
    assert!(outcome.manual_drops.is_empty());
    assert_eq!(s.slot(1), stack(LOG, 1));
}

#[test]
fn shift_crafting_does_not_credit_when_no_output_fits() {
    let mut s = Session::new(MenuKind::CraftingTable);
    s.put(1, stack(LOG, 3));
    s.click(1, 0, ClickMode::Pickup);
    s.click(1, 0, ClickMode::Pickup);
    s.inventory.slots[..36].fill(stack(STONE, 64));
    let outcome = s.click(0, 0, ClickMode::QuickMove);
    assert_eq!(outcome.crafted_output, None);
    assert!(outcome.dropped.is_empty());
    assert_eq!(s.slot(1), stack(LOG, 3));
    assert_eq!(s.slot(0), stack(PLANKS, 4));
}

#[test]
fn hotbar_crafting_swap_credits_only_an_accepted_output() {
    let mut s = Session::new(MenuKind::Player);
    s.put(1, stack(LOG, 2));
    s.click(1, 0, ClickMode::Pickup);
    s.click(1, 0, ClickMode::Pickup);
    let outcome = s.click(0, 0, ClickMode::Swap);
    assert_eq!(outcome.crafted_output, Some((PLANKS, 4)));
    assert_eq!(s.inventory.slots[0], stack(PLANKS, 4));
    let refused = s.click(0, 0, ClickMode::Swap);
    assert_eq!(refused.crafted_output, None);
    assert_eq!(s.slot(1), stack(LOG, 1));
}

#[test]
fn crafting_same_item_inputs_credits_output_not_inventory_delta() {
    let mut s = Session::new(MenuKind::Player);
    s.data.recipes[0].kind = RecipeKind::Shapeless(ShapelessRecipe {
        ingredients: vec![Ingredient {
            alternatives: vec![IngredientAlternative::Item(id("oak_planks"))],
        }],
    });
    s.data.recipes[0].result.count = 1;
    s.put(1, stack(PLANKS, 3));
    s.click(1, 0, ClickMode::Pickup);
    s.click(1, 0, ClickMode::Pickup);
    let total_before = s.total(PLANKS);
    let outcome = s.click(0, 0, ClickMode::QuickMove);
    assert_eq!(outcome.crafted_output, Some((PLANKS, 3)));
    assert_eq!(s.total(PLANKS), total_before);
    assert!(s.slot(1).is_empty());
}

#[test]
fn bucket_ingredients_leave_an_empty_bucket_in_the_grid() {
    let mut s = Session::new(MenuKind::CraftingTable);
    s.put(1, stack(WATER_BUCKET, 1));
    s.put(2, stack(STONE, 1));
    s.click(2, 0, ClickMode::Pickup);
    s.click(2, 0, ClickMode::Pickup);
    s.click(0, 0, ClickMode::Pickup);
    assert_eq!(s.menu.carried, stack(SPONGE, 1));
    assert_eq!(s.slot(1), stack(BUCKET, 1));
    assert!(s.slot(2).is_empty());
}

#[test]
fn armor_slots_accept_only_matching_equipment() {
    let mut s = Session::new(MenuKind::Player);
    s.menu.carried = stack(STONE, 1);
    s.click(5, 0, ClickMode::Pickup);
    assert!(s.slot(5).is_empty());
    s.menu.carried = stack(HELMET, 1);
    s.click(5, 0, ClickMode::Pickup);
    assert_eq!(s.slot(5), stack(HELMET, 1));
}

#[test]
fn shift_clicking_a_helmet_equips_it() {
    let mut s = Session::new(MenuKind::Player);
    s.inventory.slots[12] = stack(HELMET, 1);
    s.click(12, 0, ClickMode::QuickMove);
    assert_eq!(s.inventory.slots[39], stack(HELMET, 1));
    assert!(s.inventory.slots[12].is_empty());
}

#[test]
fn double_click_gathers_partial_stacks_before_full_ones() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(STONE, 64));
    s.put(10, stack(STONE, 5));
    s.put(11, stack(STONE, 7));
    s.menu.carried = stack(STONE, 1);
    s.click(20, 0, ClickMode::PickupAll);
    assert_eq!(s.menu.carried.count, 64);
    assert!(s.slot(10).is_empty() && s.slot(11).is_empty());
    assert_eq!(s.slot(9).count, 13);
}

#[test]
fn q_drops_one_and_ctrl_q_drops_the_stack() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(STONE, 5));
    let first = s.click(9, 0, ClickMode::Throw);
    assert_eq!(first.manual_drops, vec![(STONE, 1)]);
    assert_eq!(first.crafted_output, None);
    assert_eq!(s.slot(9).count, 4);
    let rest = s.click(9, 1, ClickMode::Throw);
    assert_eq!(rest.manual_drops, vec![(STONE, 4)]);
    assert_eq!(rest.crafted_output, None);
    assert!(s.slot(9).is_empty());
    assert_eq!(s.dropped, vec![stack(STONE, 1), stack(STONE, 4)]);
}

#[test]
fn outside_cursor_click_records_one_manual_drop_with_its_actual_count() {
    for (button, expected) in [(0, 5), (1, 1)] {
        let mut s = Session::new(MenuKind::Player);
        s.menu.carried = stack(STONE, 5);
        let outcome = s.click(-999, button, ClickMode::Pickup);
        assert_eq!(outcome.manual_drops, vec![(STONE, expected)]);
        assert_eq!(outcome.dropped, vec![stack(STONE, expected)]);
        assert_eq!(outcome.crafted_output, None);
        assert_eq!(s.menu.carried.count, 5 - expected);
    }
}

#[test]
fn refused_and_empty_drop_actions_do_not_record_manual_drops() {
    let mut s = Session::new(MenuKind::Player);
    assert!(s.click(-999, 0, ClickMode::Pickup).manual_drops.is_empty());
    assert!(s.click(9, 1, ClickMode::Throw).manual_drops.is_empty());
    s.put(9, stack(STONE, 5));
    s.menu.carried = stack(PLANKS, 1);
    let refused = s.click(9, 1, ClickMode::Throw);
    assert!(refused.manual_drops.is_empty());
    assert!(refused.dropped.is_empty());
    assert_eq!(s.slot(9), stack(STONE, 5));
}

#[test]
fn number_key_swaps_with_the_hotbar() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(STONE, 5));
    s.inventory.slots[2] = stack(LOG, 3);
    s.click(9, 2, ClickMode::Swap);
    assert_eq!(s.slot(9), stack(LOG, 3));
    assert_eq!(s.inventory.slots[2], stack(STONE, 5));
}

#[test]
fn stale_state_id_requests_a_full_resync_and_changes_bump_the_state_id() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(STONE, 5));
    let outcome = s.click_with_state(41, 9, 0, ClickMode::Pickup);
    assert!(outcome.resync);
    assert_eq!(s.menu.state_id, 1);
    let outcome = s.click(9, 0, ClickMode::Pickup);
    assert!(!outcome.resync);
    assert_eq!(outcome.changed_slots, vec![(9, stack(STONE, 5))]);
    assert_eq!(outcome.cursor, Some(ItemStack::EMPTY));
}

#[test]
fn closing_returns_cursor_and_grid_to_the_inventory() {
    let mut s = Session::new(MenuKind::CraftingTable);
    s.put(1, stack(LOG, 2));
    s.menu.carried = stack(STONE, 3);
    let rules = s.data.rules();
    let dropped = s.menu.close(&rules, &mut s.inventory);
    assert!(dropped.is_empty());
    assert!(s.menu.grid.iter().all(ItemStack::is_empty));
    assert!(s.menu.carried.is_empty());
    assert!(s.inventory.slots.contains(&stack(LOG, 2)));
    assert!(s.inventory.slots.contains(&stack(STONE, 3)));
}

#[test]
fn no_click_sequence_creates_or_destroys_items() {
    let mut s = Session::new(MenuKind::Chest { rows: 2 });
    s.put(0, stack(STONE, 64));
    s.put(1, stack(STONE, 33));
    s.put(20, stack(STONE, 17));
    s.inventory.slots[4] = stack(STONE, 9);
    let start = s.total(STONE);
    let modes = [
        ClickMode::Pickup,
        ClickMode::QuickMove,
        ClickMode::Swap,
        ClickMode::Throw,
        ClickMode::PickupAll,
        ClickMode::QuickCraft,
    ];
    let mut seed = 0x9E37_79B9_u32;
    for _ in 0..4000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let mode = modes[(seed % 6) as usize];
        let slot = if seed.is_multiple_of(23) {
            -999
        } else {
            i16::try_from((seed >> 3) % 54).unwrap()
        };
        let button = match mode {
            ClickMode::Swap => i8::try_from((seed >> 9) % 9).unwrap(),
            ClickMode::QuickCraft => i8::try_from((seed >> 9) % 7).unwrap(),
            _ => i8::try_from((seed >> 9) % 2).unwrap(),
        };
        s.click(slot, button, mode);
        assert_eq!(
            s.total(STONE),
            start,
            "after {mode:?} slot {slot} button {button}"
        );
    }
}

#[test]
fn beacon_layout_is_one_payment_slot_plus_player_tail() {
    let kind = MenuKind::Beacon;
    assert_eq!(kind.menu_type(), Some(9));
    assert_eq!(kind.slot_count(), 37);
    assert_eq!(kind.container_size(), 0);
    assert_eq!(kind.slot(0), Some(SlotRef::Grid(0)));
    assert_eq!(kind.slot(1), Some(SlotRef::Inventory(9)));
    assert_eq!(kind.slot(27), Some(SlotRef::Inventory(35)));
    assert_eq!(kind.slot(28), Some(SlotRef::Inventory(0)));
    assert_eq!(kind.slot(36), Some(SlotRef::Inventory(8)));
    assert_eq!(kind.slot(37), None);
}

#[test]
fn beacon_payment_slot_rejects_non_currency() {
    let mut s = Session::new(MenuKind::Beacon);
    // The click-test registry only knows iron_ingot as beacon currency;
    // stone must be refused while iron is accepted.
    s.menu.carried = stack(STONE, 1);
    s.click(0, 0, ClickMode::Pickup);
    assert!(s.slot(0).is_empty());
    assert_eq!(s.menu.carried, stack(STONE, 1));
    s.menu.carried = stack(IRON_INGOT, 1);
    s.click(0, 0, ClickMode::Pickup);
    assert_eq!(s.slot(0), stack(IRON_INGOT, 1));
    assert!(s.menu.carried.is_empty());
}

#[test]
fn beacon_shift_click_moves_single_payment_and_returns_on_close() {
    let mut s = Session::new(MenuKind::Beacon);
    s.inventory.slots[9] = stack(IRON_INGOT, 1);
    s.click(1, 0, ClickMode::QuickMove);
    assert_eq!(s.slot(0), stack(IRON_INGOT, 1));
    assert!(s.inventory.slots[9].is_empty());
    // With the payment occupied, vanilla shift-click moves main<->hotbar
    // instead; nothing is lost.
    s.inventory.slots[10] = stack(IRON_INGOT, 5);
    s.click(2, 0, ClickMode::QuickMove);
    assert_eq!(s.slot(0), stack(IRON_INGOT, 1));
    assert!(s.inventory.slots[10].is_empty());
    assert_eq!(s.total(IRON_INGOT), 6);
    // Payment slot drains back into the inventory, nothing is destroyed.
    s.click(0, 0, ClickMode::QuickMove);
    assert!(s.slot(0).is_empty());
    assert_eq!(s.total(IRON_INGOT), 6);
    s.inventory.slots[9] = stack(IRON_INGOT, 1);
    s.click(1, 0, ClickMode::QuickMove);
    let rules = s.data.rules();
    let dropped = s.menu.close(&rules, &mut s.inventory);
    assert!(dropped.is_empty());
    assert!(s.menu.grid.iter().all(ItemStack::is_empty));
    assert_eq!(s.total(IRON_INGOT), 7);
}

#[test]
fn crafter_disabled_slot_refuses_shift_click_top_up() {
    let mut session = Session::new(MenuKind::Crafter);
    session.put(0, stack(LOG, 3));
    session.put(36, stack(LOG, 12));
    session.menu.crafter_disabled = 1;
    session.click(36, 0, ClickMode::QuickMove);
    assert_eq!(session.slot(0), stack(LOG, 3));
    assert_eq!(session.slot(1), stack(LOG, 12));
    assert!(session.slot(36).is_empty());
    assert_eq!(session.total(LOG), 15);
}
