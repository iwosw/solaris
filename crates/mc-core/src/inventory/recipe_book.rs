//! Recipe-book placement (vanilla `ServerPlaceRecipe`), vanilla-like.
//!
//! A recipe is addressed by its index in `ItemRules::recipes`; the edge
//! announces recipe-book entries with the same index as display id.

use mc_data::ItemStack;
use mc_data::inventory_semantics_26_1_2::can_stack;
use mc_data::recipes::{Ingredient, RecipeKind, crafting_recipe_matches, ingredient_accepts_stack};

use super::crafting::grid_input;
use super::{ClickOutcome, ItemRules, Menu, PlayerInventory};

/// Inventory slots the recipe book takes ingredients from (hotbar and main).
const SOURCE_SLOTS: std::ops::Range<usize> = 0..36;

impl Menu {
    /// Moves the grid back into the inventory and fills it with ingredients
    /// for `recipe`: one more craft than the grid already held, or as many as
    /// possible with `make_all`. When the grid cannot be emptied into the
    /// inventory nothing changes; when the recipe cannot be afforded the grid
    /// stays empty.
    pub fn place_recipe(
        &mut self,
        rules: &ItemRules<'_>,
        inventory: &mut PlayerInventory,
        recipe: u32,
        make_all: bool,
        container: &mut [ItemStack],
    ) -> ClickOutcome {
        let before = self.contents(inventory, container);
        let carried_before = self.carried.clone();
        let mut container_changed = false;
        if let Some(plan) = self.plan(rules, inventory, recipe, make_all, container) {
            *inventory = plan.inventory;
            if matches!(self.kind, super::MenuKind::Furnace(_)) {
                container_changed = container[0] != plan.grid[0];
                container[0] = plan.grid.into_iter().next().expect("furnace input");
            } else {
                self.grid = plan.grid;
                self.result = super::crafting::grid_result(rules, &self.grid);
            }
        }
        self.outcome(
            &before,
            &carried_before,
            Vec::new(),
            container_changed,
            inventory,
            container,
        )
    }

    fn plan(
        &self,
        rules: &ItemRules<'_>,
        inventory: &PlayerInventory,
        recipe_index: u32,
        make_all: bool,
        container: &[ItemStack],
    ) -> Option<Placement> {
        let recipe = rules.recipes.get(usize::try_from(recipe_index).ok()?)?;
        let cooking = match (&self.kind, &recipe.kind) {
            (
                super::MenuKind::Furnace(mc_data::recipes::CookingKind::Furnace),
                RecipeKind::Smelting(recipe),
            )
            | (
                super::MenuKind::Furnace(mc_data::recipes::CookingKind::Smoker),
                RecipeKind::Smoking(recipe),
            )
            | (
                super::MenuKind::Furnace(mc_data::recipes::CookingKind::BlastFurnace),
                RecipeKind::Blasting(recipe),
            ) if container.len() == 3 => Some(recipe),
            (super::MenuKind::Furnace(_), _) => return None,
            _ => None,
        };
        let grid = if cooking.is_some() {
            &container[..1]
        } else {
            self.grid.as_slice()
        };
        let positions = if let Some(cooking) = cooking {
            vec![(0, &cooking.ingredient)]
        } else {
            let width = match grid.len() {
                4 => 2,
                9 => 3,
                _ => return None,
            };
            ingredient_positions(&recipe.kind, width)?
        };
        let already = if let Some(cooking) = cooking {
            ingredient_accepts_stack(
                rules.items,
                rules.facts,
                rules.tags,
                &grid[0],
                &cooking.ingredient,
            )
        } else {
            grid_input(grid).is_some_and(|input| {
                !self.result.is_empty()
                    && crafting_recipe_matches(rules.items, rules.facts, rules.tags, &input, recipe)
            })
        };
        let held = grid
            .iter()
            .filter(|stack| !stack.is_empty())
            .map(|stack| stack.count)
            .min()
            .unwrap_or(0);

        let mut cleared = inventory.clone();
        for stack in grid {
            let mut stack = stack.clone();
            cleared.add(rules, &mut stack);
            if !stack.is_empty() {
                return None;
            }
        }
        let desired = if make_all {
            64
        } else if already {
            held + 1
        } else {
            1
        };
        let mut grid = vec![ItemStack::EMPTY; grid.len()];
        for amount in (1..=desired).rev() {
            if let Some(picks) = pick_sources(rules, &cleared, &positions, amount) {
                for (&(slot, _), kind) in positions.iter().zip(picks) {
                    grid[slot] = take_simple(&mut cleared, &kind, amount);
                }
                break;
            }
        }
        Some(Placement {
            inventory: cleared,
            grid,
        })
    }
}

struct Placement {
    inventory: PlayerInventory,
    grid: Vec<ItemStack>,
}

/// Grid slot and ingredient for every non-empty recipe cell.
fn ingredient_positions(kind: &RecipeKind, width: usize) -> Option<Vec<(usize, &Ingredient)>> {
    match kind {
        RecipeKind::Shaped(shaped) => {
            if shaped.pattern.len() > width {
                return None;
            }
            let mut positions = Vec::new();
            for (row, line) in shaped.pattern.iter().enumerate() {
                if line.chars().count() > width {
                    return None;
                }
                for (column, symbol) in line.chars().enumerate() {
                    if symbol != ' ' {
                        positions.push((row * width + column, shaped.key.get(&symbol)?));
                    }
                }
            }
            Some(positions)
        }
        RecipeKind::Shapeless(shapeless) => (shapeless.ingredients.len() <= width * width)
            .then(|| shapeless.ingredients.iter().enumerate().collect()),
        _ => None,
    }
}

/// Plain stacks only: damaged, enchanted or renamed items are never pulled
/// into a recipe (vanilla `findSlotMatchingCraftingIngredient`).
fn is_simple(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && stack.damage.unwrap_or(0) == 0
        && stack.enchantments.is_empty()
        && stack.custom_name.is_none()
}

/// For each cell, the stack kind (item and components) to use, such that
/// `amount` of every pick is available at once. Cells take the first matching
/// kind in inventory order.
fn pick_sources(
    rules: &ItemRules<'_>,
    inventory: &PlayerInventory,
    positions: &[(usize, &Ingredient)],
    amount: i32,
) -> Option<Vec<ItemStack>> {
    let mut remaining: Vec<(&ItemStack, i32, i32)> = Vec::new();
    for slot in SOURCE_SLOTS {
        let stack = &inventory.slots[slot];
        if !is_simple(stack) {
            continue;
        }
        match remaining
            .iter_mut()
            .find(|(kind, _, _)| can_stack(kind, stack))
        {
            Some((_, count, _)) => *count += stack.count,
            None => remaining.push((stack, stack.count, rules.max_stack(stack))),
        }
    }
    let mut picks = Vec::with_capacity(positions.len());
    for (_, ingredient) in positions {
        let (kind, count, _) = remaining.iter_mut().find(|(kind, count, max)| {
            *count >= amount
                && *max >= amount
                && ingredient_accepts_stack(rules.items, rules.facts, rules.tags, kind, ingredient)
        })?;
        *count -= amount;
        picks.push((*kind).clone());
    }
    Some(picks)
}

/// Removes `amount` plain items stackable with `kind` from the source slots.
fn take_simple(inventory: &mut PlayerInventory, kind: &ItemStack, amount: i32) -> ItemStack {
    let mut taken = ItemStack::EMPTY;
    for slot in SOURCE_SLOTS {
        if taken.count >= amount {
            break;
        }
        let stack = &mut inventory.slots[slot];
        if !is_simple(stack) || !can_stack(stack, kind) {
            continue;
        }
        let part =
            mc_data::inventory_semantics_26_1_2::take_from_stack(stack, amount - taken.count);
        if taken.is_empty() {
            taken = part;
        } else {
            taken.count += part.count;
        }
    }
    taken
}
