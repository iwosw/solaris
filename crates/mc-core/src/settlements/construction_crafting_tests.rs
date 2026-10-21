use super::{can_craft, can_fund, craft, missing_base};
use crate::settlements::allocator::tests::test_state;
use crate::settlements::inventory;
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_script::ScriptInventoryFence;

fn state() -> TickState {
    test_state()
}

fn item(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}

fn image(slots: Vec<ItemStack>) -> inventory::Image {
    inventory::Image {
        endpoint: inventory::Endpoint::Container {
            dimension: "minecraft:overworld".into(),
            position: [0, 0, 0],
        },
        slots,
        fence: ScriptInventoryFence::try_new(
            1,
            "0000000000000000000000000000000000000000000000000000000000000000".into(),
        )
        .unwrap(),
    }
}

fn stock(item: u32, count: i32) -> ItemStack {
    ItemStack::new(item, count)
}

fn sum(slots: &[Vec<ItemStack>], item: u32) -> i32 {
    let mut total = 0;
    for image in slots {
        for stack in image {
            if !stack.is_empty() && stack.item_id == item {
                total += stack.count.max(0);
            }
        }
    }
    total
}

fn chest(slots: Vec<ItemStack>) -> Vec<inventory::Image> {
    let mut padded = slots;
    padded.resize(27, ItemStack::EMPTY);
    vec![image(padded)]
}

#[test]
fn logs_to_door_exact_yield_and_surplus() {
    let state = state();
    let log = item(&state, "minecraft:oak_log");
    let planks = item(&state, "minecraft:oak_planks");
    let door = item(&state, "minecraft:oak_door");
    // Two logs -> 8 planks; one door batch consumes 6 planks and yields 3.
    let images = chest(vec![stock(log, 2)]);
    let post = craft(&state, door, &images).expect("two logs cover one door");
    assert_eq!(sum(&post, door), 3);
    assert_eq!(sum(&post, planks), 2);
    assert_eq!(sum(&post, log), 0);
    assert!(can_craft(&state, door, &images));
}

#[test]
fn wool_bed_chain() {
    let state = state();
    let log = item(&state, "minecraft:oak_log");
    let red_bed = item(&state, "minecraft:red_bed");
    let wool = item(&state, "minecraft:red_wool");
    // Bed needs 3 wool + 3 planks; one log covers the planks leg.
    let images = chest(vec![stock(wool, 3), stock(log, 1)]);
    let post = craft(&state, red_bed, &images).expect("wool plus log covers a bed");
    assert_eq!(sum(&post, red_bed), 1);
    assert_eq!(sum(&post, wool), 0);
}

#[test]
fn missing_ingredient_refuses_atomically() {
    let state = state();
    let log = item(&state, "minecraft:oak_log");
    let door = item(&state, "minecraft:oak_door");
    let planks = item(&state, "minecraft:oak_planks");
    // One log -> 4 planks, but a door needs 6: must refuse with None.
    let images = chest(vec![stock(log, 1)]);
    assert!(craft(&state, door, &images).is_none());
    assert!(!can_craft(&state, door, &images));
    // Pure stock gap: the raw log is not planks, so all six are still missing.
    assert_eq!(
        missing_base(&state, &[(planks, 6)], &images),
        vec![(planks, 6)]
    );
}

#[test]
fn no_room_refuses() {
    let state = state();
    let stone = item(&state, "minecraft:stone");
    let door = item(&state, "minecraft:oak_door");
    let log = item(&state, "minecraft:oak_log");
    // Every slot full of stone: consumption impossible (logs stuck) and no
    // empty/mergeable room for intermediates either.
    let full: Vec<ItemStack> = (0..27).map(|_| stock(stone, 64)).collect();
    let mut images = chest(full);
    images[0].slots[0] = stock(log, 6);
    assert!(craft(&state, door, &images).is_none());
}

#[test]
fn existing_item_returns_unchanged() {
    let state = state();
    let door = item(&state, "minecraft:oak_door");
    let images = chest(vec![stock(door, 1)]);
    let post = craft(&state, door, &images).expect("stocked item needs no craft");
    assert_eq!(post.len(), 1);
    assert_eq!(post[0].len(), images[0].slots.len());
    assert_eq!(sum(&post, door), 1);
}

#[test]
fn whole_upgrade_funding_never_double_spends_recipe_surplus_or_logs() {
    let state = state();
    let log = item(&state, "minecraft:oak_log");
    let planks = item(&state, "minecraft:oak_planks");
    let door = item(&state, "minecraft:oak_door");
    let images = chest(vec![stock(log, 2)]);
    assert!(can_fund(&state, &[(door, 3), (planks, 2)], &images));
    assert!(!can_fund(&state, &[(door, 3), (planks, 3)], &images));
    assert_eq!(
        images[0].slots[0],
        stock(log, 2),
        "funding proofs never mutate stock"
    );
}

#[test]
fn whole_storage_funding_accepts_real_raw_stock_across_many_cell_crafts() {
    let state = state();
    let log = item(&state, "minecraft:oak_log");
    let coal = item(&state, "minecraft:coal");
    let cobble = item(&state, "minecraft:cobblestone");
    let images = chest(vec![
        stock(log, 64),
        stock(log, 64),
        stock(log, 64),
        stock(log, 43),
        stock(coal, 24),
        stock(cobble, 64),
        stock(cobble, 38),
    ]);
    let materials = &crate::settlements::blueprints::get(&state.registry, "storage")
        .unwrap()
        .level(1)
        .unwrap()
        .materials;
    assert!(
        can_fund(&state, materials, &images),
        "real logs, coal and cobble cover Storage L1 through the same per-cell recipes"
    );
    assert_eq!(
        images[0].slots[0],
        stock(log, 64),
        "funding remains read-only"
    );
}
