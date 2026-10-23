//! Crafting grid result and vanilla `ResultSlot.onTake` consumption.

use mc_data::inventory_semantics_26_1_2::can_stack;
use mc_data::recipes::crafting_result_from_input;
use mc_data::{Identifier, ItemStack};

use super::{ItemRules, PlayerInventory};

/// A 2x2 grid placed top-left in 3x3, or a 3x3 grid as is.
pub(super) fn grid_input(grid: &[ItemStack]) -> Option<[ItemStack; 9]> {
    let mut input: [ItemStack; 9] = std::array::from_fn(|_| ItemStack::EMPTY);
    match grid.len() {
        4 => {
            for (index, stack) in grid.iter().enumerate() {
                input[(index / 2) * 3 + index % 2] = stack.clone();
            }
        }
        9 => {
            for (slot, stack) in input.iter_mut().zip(grid) {
                *slot = stack.clone();
            }
        }
        _ => return None,
    }
    Some(input)
}

/// Recomputes the result of a 2x2 or 3x3 grid.
pub(super) fn grid_result(rules: &ItemRules<'_>, grid: &[ItemStack]) -> ItemStack {
    let Some(input) = grid_input(grid) else {
        return ItemStack::EMPTY;
    };
    crafting_result_from_input(rules.items, rules.facts, rules.tags, rules.recipes, &input)
}

/// Vanilla `Item.craftRemainder` as set in 26.1.2 `Items`; the data reports
/// do not carry it.
const CRAFT_REMAINDERS: [(&str, &str); 5] = [
    ("minecraft:water_bucket", "minecraft:bucket"),
    ("minecraft:lava_bucket", "minecraft:bucket"),
    ("minecraft:milk_bucket", "minecraft:bucket"),
    ("minecraft:dragon_breath", "minecraft:glass_bottle"),
    ("minecraft:honey_bottle", "minecraft:glass_bottle"),
];

/// Item left in the grid when an ingredient is consumed. Plugin items are
/// not their carrier item and leave nothing.
fn craft_remainder(rules: &ItemRules<'_>, stack: &ItemStack) -> Option<ItemStack> {
    if stack
        .item_model
        .as_deref()
        .is_some_and(|model| rules.facts.custom(model).is_some())
    {
        return None;
    }
    let name = rules.items.name_of(stack.item_id)?;
    let (_, remainder) = CRAFT_REMAINDERS
        .iter()
        .find(|(item, _)| *item == name.as_str())?;
    let id = rules.items.id_of(&Identifier::parse(*remainder).ok()?)?;
    Some(ItemStack::new(id, 1))
}

/// Consumes one craft from the grid: every occupied slot loses one item and
/// receives its remainder; a remainder that cannot go back into its grid slot
/// goes to the inventory, else into `dropped`.
pub(super) fn consume_craft(
    rules: &ItemRules<'_>,
    grid: &mut [ItemStack],
    inventory: &mut PlayerInventory,
    dropped: &mut Vec<ItemStack>,
) {
    let retained_source =
        mc_data::recipes::cloning::source(rules.items, rules.facts, rules.tags, grid);
    for (index, slot) in grid.iter_mut().enumerate() {
        if Some(index) == retained_source {
            continue;
        }
        if slot.is_empty() {
            continue;
        }
        let remainder = craft_remainder(rules, slot);
        mc_data::inventory_semantics_26_1_2::decrement_stack(slot);
        let Some(mut remainder) = remainder else {
            continue;
        };
        if slot.is_empty() {
            *slot = remainder;
        } else if can_stack(slot, &remainder) {
            slot.count += remainder.count;
        } else {
            inventory.add(rules, &mut remainder);
            if !remainder.is_empty() {
                dropped.push(remainder);
            }
        }
    }
}
