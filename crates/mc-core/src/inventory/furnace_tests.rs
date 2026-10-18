use mc_data::ItemStack;
use mc_data::recipes::CookingKind;

use super::MenuKind;
use super::click_tests::{
    BUCKET, COAL, Data, IRON_INGOT, LAVA_BUCKET, PLANKS, RAW_IRON, STONE, Session, stack,
};
use super::furnace::{FUEL, FurnaceState, INPUT, OUTPUT};
use crate::boundary::ClickMode;

fn run(state: &mut FurnaceState, data: &Data, slots: &mut [ItemStack], ticks: usize) {
    let rules = data.rules();
    for _ in 0..ticks {
        state.tick(&rules, CookingKind::Furnace, slots);
    }
}

#[test]
fn coal_smelts_one_raw_iron_in_200_ticks() {
    let data = Data::new();
    let mut slots = vec![stack(RAW_IRON, 2), stack(COAL, 3), ItemStack::EMPTY];
    let mut state = FurnaceState::default();
    let rules = data.rules();
    let first = state.tick(&rules, CookingKind::Furnace, &mut slots);
    assert_eq!(first.lit, Some(true));
    assert_eq!(slots[FUEL].count, 2);
    run(&mut state, &data, &mut slots, 198);
    assert!(slots[OUTPUT].is_empty());
    run(&mut state, &data, &mut slots, 1);
    assert_eq!(slots[OUTPUT], stack(IRON_INGOT, 1));
    assert_eq!(slots[INPUT].count, 1);
    assert_eq!(slots[FUEL].count, 2, "one coal lasts 1600 ticks");
}

#[test]
fn unlit_furnace_loses_progress_two_per_tick() {
    let data = Data::new();
    let mut slots = vec![stack(RAW_IRON, 1), stack(COAL, 1), ItemStack::EMPTY];
    let mut state = FurnaceState::default();
    run(&mut state, &data, &mut slots, 50);
    assert_eq!(state.cook_progress, 50);
    slots[INPUT] = ItemStack::EMPTY;
    state.lit_remaining = 0;
    run(&mut state, &data, &mut slots, 10);
    assert_eq!(state.cook_progress, 30);
}

#[test]
fn burnt_lava_bucket_leaves_an_empty_bucket() {
    let data = Data::new();
    let mut slots = vec![stack(RAW_IRON, 1), stack(LAVA_BUCKET, 1), ItemStack::EMPTY];
    let mut state = FurnaceState::default();
    run(&mut state, &data, &mut slots, 1);
    assert_eq!(slots[FUEL], stack(BUCKET, 1));
}

#[test]
fn non_smeltable_input_never_lights_the_furnace() {
    let data = Data::new();
    let mut slots = vec![stack(STONE, 1), stack(COAL, 1), ItemStack::EMPTY];
    let mut state = FurnaceState::default();
    run(&mut state, &data, &mut slots, 20);
    assert!(!state.is_lit());
    assert_eq!(slots[FUEL].count, 1);
}

#[test]
fn fuel_slot_accepts_only_fuel_and_empty_buckets() {
    let mut s = Session::new(MenuKind::Furnace(CookingKind::Furnace));
    s.menu.carried = stack(STONE, 4);
    s.click(1, 0, ClickMode::Pickup);
    assert!(s.slot(FUEL).is_empty());
    s.menu.carried = stack(BUCKET, 3);
    s.click(1, 0, ClickMode::Pickup);
    assert_eq!(s.slot(FUEL), stack(BUCKET, 1), "fuel slot holds one bucket");
}

#[test]
fn hotbar_swap_credits_only_the_displaced_items_that_overflow() {
    for room in [0, 4, 10] {
        let mut s = Session::new(MenuKind::Furnace(CookingKind::Furnace));
        s.put(FUEL, stack(COAL, 10));
        s.inventory.slots[..36].fill(stack(STONE, 64));
        s.inventory.slots[0] = stack(BUCKET, 3);
        if room > 0 {
            s.inventory.slots[1] = stack(COAL, 64 - room);
        }
        let outcome = s.click(FUEL as i16, 0, ClickMode::Swap);
        assert_eq!(s.slot(FUEL), stack(BUCKET, 1));
        assert_eq!(s.inventory.slots[0], stack(BUCKET, 2));
        assert_eq!(outcome.crafted_output, None);
        if room < 10 {
            assert_eq!(outcome.manual_drops, vec![(COAL, 10 - room)]);
            assert_eq!(outcome.dropped, vec![stack(COAL, 10 - room)]);
        } else {
            assert!(outcome.manual_drops.is_empty());
            assert!(outcome.dropped.is_empty());
        }
    }
}

#[test]
fn output_slot_refuses_placement() {
    let mut s = Session::new(MenuKind::Furnace(CookingKind::Furnace));
    s.menu.carried = stack(IRON_INGOT, 1);
    s.click(2, 0, ClickMode::Pickup);
    assert!(s.slot(OUTPUT).is_empty());
}

#[test]
fn shift_click_routes_smeltables_to_input_and_fuel_to_fuel() {
    let mut s = Session::new(MenuKind::Furnace(CookingKind::Furnace));
    s.inventory.slots[9] = stack(RAW_IRON, 5);
    s.inventory.slots[10] = stack(COAL, 5);
    s.click(3, 0, ClickMode::QuickMove);
    s.click(4, 0, ClickMode::QuickMove);
    assert_eq!(s.slot(INPUT), stack(RAW_IRON, 5));
    assert_eq!(s.slot(FUEL), stack(COAL, 5));
}

#[test]
fn output_joins_a_cursor_stack_only_whole() {
    let mut s = Session::new(MenuKind::Furnace(CookingKind::Furnace));
    s.put(OUTPUT, stack(IRON_INGOT, 10));
    s.menu.carried = stack(IRON_INGOT, 60);
    let outcome = s.click(2, 0, ClickMode::Pickup);
    assert_eq!(s.slot(OUTPUT).count, 10);
    assert!(!outcome.furnace_output_taken);
    assert_eq!(outcome.crafted_output, None);
    s.menu.carried = stack(IRON_INGOT, 50);
    let outcome = s.click(2, 0, ClickMode::Pickup);
    assert!(s.slot(OUTPUT).is_empty());
    assert_eq!(s.menu.carried.count, 60);
    assert!(outcome.furnace_output_taken);
    assert_eq!(outcome.crafted_output, Some((IRON_INGOT, 10)));
}

#[test]
fn furnace_output_credits_only_the_items_actually_extracted() {
    for (mode, button, expected) in [
        (ClickMode::Pickup, 0, 10),
        (ClickMode::Pickup, 1, 5),
        (ClickMode::QuickMove, 0, 10),
        (ClickMode::Swap, 0, 10),
        (ClickMode::Throw, 0, 1),
        (ClickMode::Throw, 1, 10),
    ] {
        let mut s = Session::new(MenuKind::Furnace(CookingKind::Furnace));
        s.put(OUTPUT, stack(IRON_INGOT, 10));
        let outcome = s.click(OUTPUT as i16, button, mode);
        assert_eq!(outcome.crafted_output, Some((IRON_INGOT, expected)));
        let manual = if mode == ClickMode::Throw {
            vec![(IRON_INGOT, expected)]
        } else {
            Vec::new()
        };
        assert_eq!(outcome.manual_drops, manual);
        assert_eq!(s.slot(OUTPUT).count, 10 - expected);
    }
}

#[test]
fn partial_furnace_shift_take_does_not_credit_unmoved_output() {
    let mut s = Session::new(MenuKind::Furnace(CookingKind::Furnace));
    s.put(OUTPUT, stack(IRON_INGOT, 10));
    s.inventory.slots[..36].fill(stack(STONE, 64));
    s.inventory.slots[8] = stack(IRON_INGOT, 60);
    let outcome = s.click(OUTPUT as i16, 0, ClickMode::QuickMove);
    assert_eq!(outcome.crafted_output, Some((IRON_INGOT, 4)));
    assert_eq!(s.slot(OUTPUT), stack(IRON_INGOT, 6));
    assert!(outcome.dropped.is_empty());
    let refused = s.click(OUTPUT as i16, 0, ClickMode::QuickMove);
    assert_eq!(refused.crafted_output, None);
}

#[test]
fn gathering_furnace_output_does_not_credit_existing_inventory_items() {
    let mut s = Session::new(MenuKind::Furnace(CookingKind::Furnace));
    s.put(OUTPUT, stack(IRON_INGOT, 5));
    s.menu.carried = stack(IRON_INGOT, 1);
    s.inventory.slots[9] = stack(IRON_INGOT, 3);
    let outcome = s.click(4, 0, ClickMode::PickupAll);
    assert_eq!(outcome.crafted_output, Some((IRON_INGOT, 5)));
    assert_eq!(s.menu.carried, stack(IRON_INGOT, 9));
    assert!(s.slot(OUTPUT).is_empty());
}

#[test]
fn taking_output_awards_the_recipes_experience_once() {
    let data = Data::new();
    let mut slots = vec![stack(RAW_IRON, 3), stack(COAL, 1), ItemStack::EMPTY];
    let mut state = FurnaceState::default();
    run(&mut state, &data, &mut slots, 600);
    assert_eq!(slots[OUTPUT], stack(IRON_INGOT, 3));
    let rules = data.rules();
    assert_eq!(state.take_experience(&rules, 7), 3);
    assert_eq!(state.take_experience(&rules, 7), 0);
}

#[test]
fn replacing_the_input_item_restarts_cooking() {
    let mut s = Session::new(MenuKind::Furnace(CookingKind::Furnace));
    s.put(INPUT, stack(RAW_IRON, 2));
    s.menu.carried = stack(STONE, 1);
    let outcome = s.click(0, 0, ClickMode::Pickup);
    let (before, after) = outcome.furnace_input_change.expect("input changed");
    let mut state = FurnaceState {
        cook_progress: 120,
        ..FurnaceState::default()
    };
    state.input_replaced(&before, &after);
    assert_eq!(state.cook_progress, 0);
}

#[test]
fn full_data_furnace_accepts_acacia_planks_and_cooks_charcoal() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let registry = crate::registry::CoreRegistry::load(Some(&dir)).expect("full registry loads");
    let rules = registry.item_rules();
    let item = |name| {
        registry
            .items
            .id_of(&mc_data::Identifier::parse(name).expect("item identifier"))
            .expect("vanilla item")
    };
    let log = item("minecraft:acacia_log");
    let planks = item("minecraft:acacia_planks");
    let charcoal = item("minecraft:charcoal");
    assert!(
        rules
            .tags
            .contains_raw_id("minecraft:item", "minecraft:planks", planks as i32)
    );
    assert_eq!(
        rules.tags.fuel_values().burn_duration(planks),
        Some(300),
        "full-data startup must initialize fuel values, not just resolved tags"
    );
    let mut menu = super::Menu::new(1, MenuKind::Furnace(CookingKind::Furnace));
    let mut inventory = super::PlayerInventory::default();
    inventory.slots[0] = stack(log, 4);
    inventory.slots[1] = stack(planks, 7);
    let mut slots = vec![ItemStack::EMPTY; 3];
    for slot in [30, 0, 31, 1] {
        let state_id = menu.state_id;
        let outcome = menu.click(
            &rules,
            &mut inventory,
            &mut slots,
            state_id,
            slot,
            0,
            ClickMode::Pickup,
            false,
        );
        assert!(!outcome.resync, "current-state pickup must not need resync");
    }
    assert_eq!(slots[INPUT], stack(log, 4));
    assert_eq!(slots[FUEL], stack(planks, 7));
    assert!(
        menu.carried.is_empty(),
        "fuel placement must empty the cursor"
    );
    let mut state = FurnaceState::default();
    for _ in 0..200 {
        state.tick(&rules, CookingKind::Furnace, &mut slots);
    }
    assert_eq!(state.lit_total, 300);
    assert_eq!(slots[INPUT], stack(log, 3));
    assert_eq!(slots[FUEL], stack(planks, 6));
    assert_eq!(slots[OUTPUT], stack(charcoal, 1));
}

/// Equivalence oracle: `catch_up` must match explicit repeated `tick`s
/// (state, slots, aggregated `changed`, initial-to-final `lit` edge).
fn catch_up_matches_ticks(
    input: ItemStack,
    fuel: ItemStack,
    output: ItemStack,
    state: FurnaceState,
    elapsed: u64,
) {
    let data = Data::new();
    let rules = data.rules();
    let mut caught_state = state.clone();
    let mut caught_slots = vec![input.clone(), fuel.clone(), output.clone()];
    let caught = caught_state.catch_up(&rules, CookingKind::Furnace, &mut caught_slots, elapsed);
    let was_lit = state.is_lit();
    let mut ticked_state = state;
    let mut ticked_slots = vec![input, fuel, output];
    let mut changed = false;
    for _ in 0..elapsed {
        changed |= ticked_state
            .tick(&rules, CookingKind::Furnace, &mut ticked_slots)
            .changed;
    }
    assert_eq!(caught_state, ticked_state, "state after {elapsed} ticks");
    assert_eq!(caught_slots, ticked_slots, "slots after {elapsed} ticks");
    assert_eq!(caught.changed, changed, "changed after {elapsed} ticks");
    assert_eq!(
        caught.lit,
        (was_lit != ticked_state.is_lit()).then_some(ticked_state.is_lit()),
        "lit edge after {elapsed} ticks"
    );
}

#[test]
fn catch_up_cooks_two_items_in_400_ticks() {
    let data = Data::new();
    let rules = data.rules();
    let mut slots = vec![stack(RAW_IRON, 8), stack(COAL, 2), ItemStack::EMPTY];
    let mut state = FurnaceState::default();
    let outcome = state.catch_up(&rules, CookingKind::Furnace, &mut slots, 400);
    assert_eq!(slots[OUTPUT], stack(IRON_INGOT, 2));
    assert_eq!(slots[INPUT], stack(RAW_IRON, 6));
    assert_eq!(slots[FUEL], stack(COAL, 1), "one coal lasts 1600 ticks");
    assert!(state.is_lit());
    assert!(outcome.changed);
    assert_eq!(outcome.lit, Some(true));
}

#[test]
fn catch_up_stops_cooking_when_output_fills() {
    let data = Data::new();
    let rules = data.rules();
    let mut slots = vec![stack(RAW_IRON, 2), stack(COAL, 1), stack(IRON_INGOT, 63)];
    let mut state = FurnaceState::default();
    state.catch_up(&rules, CookingKind::Furnace, &mut slots, 400);
    assert_eq!(slots[OUTPUT], stack(IRON_INGOT, 64));
    assert_eq!(slots[INPUT], stack(RAW_IRON, 1));
    assert_eq!(state.cook_progress, 0, "lost burnable resets progress");
}

#[test]
fn catch_up_cools_unlit_progress_to_rest() {
    let data = Data::new();
    let rules = data.rules();
    let mut slots = vec![stack(RAW_IRON, 1), ItemStack::EMPTY, ItemStack::EMPTY];
    let mut state = FurnaceState {
        cook_progress: 50,
        cook_total: 200,
        ..FurnaceState::default()
    };
    let outcome = state.catch_up(&rules, CookingKind::Furnace, &mut slots, 10);
    assert_eq!(state.cook_progress, 30);
    assert!(!outcome.changed);
    assert_eq!(outcome.lit, None);
    // Cooling clamps at zero rather than wrapping.
    state.catch_up(&rules, CookingKind::Furnace, &mut slots, 200);
    assert_eq!(state.cook_progress, 0);
}

#[test]
fn catch_up_burns_out_one_coal_at_the_fuel_boundary() {
    let data = Data::new();
    let rules = data.rules();
    let mut slots = vec![stack(RAW_IRON, 8), stack(COAL, 1), ItemStack::EMPTY];
    let mut state = FurnaceState::default();
    let outcome = state.catch_up(&rules, CookingKind::Furnace, &mut slots, 1601);
    assert_eq!(slots[OUTPUT], stack(IRON_INGOT, 8));
    assert!(slots[INPUT].is_empty());
    assert!(slots[FUEL].is_empty());
    assert!(!state.is_lit());
    assert!(outcome.changed);
    assert_eq!(outcome.lit, None, "unlit at both ends of the window");
}

#[test]
fn catch_up_matches_repeated_ticks_across_boundaries() {
    let midway = FurnaceState {
        lit_remaining: 100,
        lit_total: 1600,
        cook_progress: 150,
        cook_total: 200,
        ..FurnaceState::default()
    };
    let cooling = FurnaceState {
        cook_progress: 50,
        cook_total: 200,
        ..FurnaceState::default()
    };
    let almost_done = FurnaceState {
        lit_remaining: 1600,
        lit_total: 1600,
        cook_progress: 199,
        cook_total: 200,
        ..FurnaceState::default()
    };
    let fresh_burn = FurnaceState {
        lit_remaining: 1600,
        lit_total: 1600,
        cook_progress: 1,
        cook_total: 200,
        ..FurnaceState::default()
    };
    let last_flame = FurnaceState {
        lit_remaining: 1,
        lit_total: 1600,
        cook_progress: 150,
        cook_total: 200,
        ..FurnaceState::default()
    };
    let full_output_lit = FurnaceState {
        lit_remaining: 100,
        lit_total: 1600,
        cook_progress: 150,
        cook_total: 200,
        ..FurnaceState::default()
    };
    let bad_fuel_lit = FurnaceState {
        lit_remaining: 5,
        lit_total: 1600,
        cook_progress: 150,
        cook_total: 200,
        ..FurnaceState::default()
    };
    let cases: Vec<(ItemStack, ItemStack, ItemStack, FurnaceState, Vec<u64>)> = vec![
        (
            stack(RAW_IRON, 2),
            stack(COAL, 1),
            ItemStack::EMPTY,
            FurnaceState::default(),
            vec![0, 1, 2, 199, 200, 201, 400, 1599, 1600, 1601],
        ),
        (
            stack(RAW_IRON, 1),
            stack(COAL, 1),
            ItemStack::EMPTY,
            FurnaceState::default(),
            vec![200, 201, 1600],
        ),
        (
            stack(RAW_IRON, 2),
            stack(COAL, 1),
            stack(IRON_INGOT, 63),
            FurnaceState::default(),
            vec![1, 200, 400],
        ),
        (
            stack(RAW_IRON, 1),
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            cooling,
            vec![1, 10, 25, 26],
        ),
        (
            stack(RAW_IRON, 1),
            stack(LAVA_BUCKET, 1),
            ItemStack::EMPTY,
            FurnaceState::default(),
            vec![1, 200, 201],
        ),
        (
            stack(RAW_IRON, 2),
            stack(PLANKS, 1),
            ItemStack::EMPTY,
            FurnaceState::default(),
            vec![200, 299, 300, 301, 400],
        ),
        (
            stack(RAW_IRON, 2),
            stack(COAL, 1),
            ItemStack::EMPTY,
            midway,
            vec![50, 51, 100, 150],
        ),
        // Initial progress one tick from completing an item.
        (
            stack(RAW_IRON, 2),
            stack(COAL, 1),
            ItemStack::EMPTY,
            almost_done,
            vec![1, 2, 3],
        ),
        // Initial progress of one tick: jump must not skip the completion.
        (
            stack(RAW_IRON, 2),
            stack(COAL, 1),
            ItemStack::EMPTY,
            fresh_burn,
            vec![1, 199, 200],
        ),
        // One flame tick left: expiry boundary forces relight from fuel.
        (
            stack(RAW_IRON, 2),
            stack(COAL, 1),
            ItemStack::EMPTY,
            last_flame,
            vec![1, 2, 50],
        ),
        // Output full while lit: progress resets once space is lost.
        (
            stack(RAW_IRON, 2),
            stack(COAL, 1),
            stack(IRON_INGOT, 64),
            full_output_lit,
            vec![1, 2, 50],
        ),
        // Invalid fuel left while lit: unlit tail cools once flame dies.
        (
            stack(RAW_IRON, 1),
            stack(STONE, 1),
            ItemStack::EMPTY,
            bad_fuel_lit,
            vec![1, 5, 6, 20],
        ),
    ];
    for (input, fuel, output, state, windows) in cases {
        for elapsed in windows {
            catch_up_matches_ticks(
                input.clone(),
                fuel.clone(),
                output.clone(),
                state.clone(),
                elapsed,
            );
        }
    }
}
