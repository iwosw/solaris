//! Furnace, smoker and blast furnace: vanilla `AbstractFurnaceBlockEntity`
//! cooking over owned state. Storage slots: 0 input, 1 fuel, 2 output.

use std::collections::BTreeMap;

use mc_data::inventory_semantics_26_1_2::{can_stack, decrement_stack};
use mc_data::recipes::{
    CookingKind, Recipe, RecipeKind, furnace_experience_award, furnace_fuel_ticks,
    ingredient_accepts_stack,
};
use mc_data::{Identifier, ItemStack};

use super::ItemRules;

pub const INPUT: usize = 0;
pub const FUEL: usize = 1;
pub const OUTPUT: usize = 2;
/// Furnace container limit (`AbstractFurnaceBlockEntity` uses the default 99,
/// clamped by each item's own stack size).
const FURNACE_MAX_STACK: i32 = 99;
/// Cooking progress lost per tick while unlit (`BURN_COOL_SPEED`).
const COOL_SPEED: i32 = 2;
const DEFAULT_COOK_TIME: i32 = 200;

/// Block-entity state besides the three slots.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FurnaceState {
    pub lit_remaining: i32,
    pub lit_total: i32,
    pub cook_progress: i32,
    pub cook_total: i32,
    /// Recipe id -> crafts since the output was last taken (experience).
    pub recipes_used: BTreeMap<String, i32>,
}

/// What one furnace tick changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FurnaceTick {
    /// Slots or state changed: mark the block entity dirty.
    pub changed: bool,
    /// The block's `lit` property must change to this value.
    pub lit: Option<bool>,
}

impl FurnaceState {
    #[must_use]
    pub fn is_lit(&self) -> bool {
        self.lit_remaining > 0
    }

    /// Menu properties 0-3 in vanilla data-slot order.
    #[must_use]
    pub fn properties(&self) -> [i32; 4] {
        [
            self.lit_remaining,
            self.lit_total,
            self.cook_progress,
            self.cook_total,
        ]
    }

    /// Vanilla `setItem(INPUT)`: a different input item restarts cooking.
    pub fn input_replaced(&mut self, before: &ItemStack, after: &ItemStack) {
        if !can_stack(before, after) {
            self.cook_progress = 0;
        }
    }

    /// One server tick.
    pub fn tick(
        &mut self,
        rules: &ItemRules<'_>,
        kind: CookingKind,
        slots: &mut [ItemStack],
    ) -> FurnaceTick {
        let was_lit = self.is_lit();
        let mut changed = false;
        if self.is_lit() {
            self.lit_remaining -= 1;
        }
        let has_input = !slots[INPUT].is_empty();
        let has_fuel = !slots[FUEL].is_empty();
        if self.is_lit() || (has_fuel && has_input) {
            let recipe = has_input
                .then(|| cooking_recipe(rules, kind, &slots[INPUT]))
                .flatten();
            let burnable = recipe.is_some_and(|recipe| can_burn(rules, recipe, slots));
            if !self.is_lit() && burnable {
                self.lit_remaining = fuel_ticks(rules, kind, &slots[FUEL]);
                self.lit_total = self.lit_remaining;
                if self.is_lit() {
                    changed = true;
                    let remainder = fuel_remainder(rules, &slots[FUEL]);
                    decrement_stack(&mut slots[FUEL]);
                    if slots[FUEL].is_empty()
                        && let Some(remainder) = remainder
                    {
                        slots[FUEL] = remainder;
                    }
                }
            }
            if self.is_lit() && burnable {
                let recipe = recipe.expect("burnable implies a recipe");
                self.cook_total = cook_time(recipe);
                self.cook_progress += 1;
                if self.cook_progress >= self.cook_total {
                    self.cook_progress = 0;
                    burn(rules, recipe, slots);
                    *self
                        .recipes_used
                        .entry(recipe.id.as_str().to_owned())
                        .or_default() += 1;
                    changed = true;
                }
            } else {
                self.cook_progress = 0;
            }
        } else if !self.is_lit() && self.cook_progress > 0 {
            self.cook_progress = (self.cook_progress - COOL_SPEED).clamp(0, self.cook_total);
        }
        let lit = (was_lit != self.is_lit()).then_some(self.is_lit());
        FurnaceTick {
            changed: changed || lit.is_some(),
            lit,
        }
    }

    /// Advance between fuel/item boundaries, rechecking output custody and
    /// the actual recipe after every boundary. Unlit tails cool normally.
    pub(crate) fn catch_up(
        &mut self,
        rules: &ItemRules<'_>,
        kind: CookingKind,
        slots: &mut [ItemStack],
        elapsed: u64,
    ) -> FurnaceTick {
        let was_lit = self.is_lit();
        let mut changed = false;
        let mut remaining = elapsed;
        while remaining > 0 {
            // The live owner handles each boundary (fuel expiry, ignition,
            // item completion, lost input/output space) without a second rule.
            changed |= self.tick(rules, kind, slots).changed;
            remaining -= 1;
            if self.is_lit() {
                let burnable = cooking_recipe(rules, kind, &slots[INPUT])
                    .is_some_and(|recipe| can_burn(rules, recipe, slots));
                let mut skip = remaining.min((self.lit_remaining - 1).max(0) as u64);
                if burnable {
                    skip = skip.min((self.cook_total - self.cook_progress - 1).max(0) as u64);
                    self.cook_progress += skip as i32;
                }
                self.lit_remaining -= skip as i32;
                remaining -= skip;
            } else {
                // If input/fuel could ignite, the boundary tick above already
                // did so. Otherwise only the no-input/no-fuel cooling tail
                // can change state; never multiply an unbounded elapsed value.
                if (slots[INPUT].is_empty() || slots[FUEL].is_empty()) && self.cook_progress > 0 {
                    let cooling = remaining.min(i32::MAX as u64) as i32;
                    self.cook_progress = self
                        .cook_progress
                        .saturating_sub(cooling.saturating_mul(COOL_SPEED))
                        .clamp(0, self.cook_total);
                }
                break;
            }
        }
        let lit = (was_lit != self.is_lit()).then_some(self.is_lit());
        FurnaceTick {
            changed: changed || lit.is_some(),
            lit,
        }
    }

    /// Experience for taking output; clears the used-recipe record.
    pub fn take_experience(&mut self, rules: &ItemRules<'_>, seed: u64) -> i32 {
        let award = furnace_experience_award(rules.recipes, &self.recipes_used, seed);
        self.recipes_used.clear();
        award
    }
}

/// The cooking recipe of `kind` whose ingredient accepts the actual input
/// stack (a plugin item never cooks as its carrier).
fn cooking_recipe<'a>(
    rules: &ItemRules<'a>,
    kind: CookingKind,
    input: &ItemStack,
) -> Option<&'a Recipe> {
    rules.recipes.iter().find(|recipe| {
        let cooking = match (&recipe.kind, kind) {
            (RecipeKind::Smelting(cooking), CookingKind::Furnace)
            | (RecipeKind::Smoking(cooking), CookingKind::Smoker)
            | (RecipeKind::Blasting(cooking), CookingKind::BlastFurnace) => cooking,
            _ => return false,
        };
        ingredient_accepts_stack(
            rules.items,
            rules.facts,
            rules.tags,
            input,
            &cooking.ingredient,
        )
    })
}

fn cook_time(recipe: &Recipe) -> i32 {
    match &recipe.kind {
        RecipeKind::Smelting(cooking)
        | RecipeKind::Smoking(cooking)
        | RecipeKind::Blasting(cooking) => i32::try_from(cooking.cooking_time).unwrap_or(i32::MAX),
        _ => DEFAULT_COOK_TIME,
    }
    .max(1)
}

pub(crate) fn is_fuel(rules: &ItemRules<'_>, kind: CookingKind, stack: &ItemStack) -> bool {
    !stack.is_empty() && furnace_fuel_ticks(rules.tags, kind, stack.item_id).is_some()
}

pub(super) fn can_smelt(rules: &ItemRules<'_>, kind: CookingKind, stack: &ItemStack) -> bool {
    cooking_recipe(rules, kind, stack).is_some()
}

pub(crate) fn is_empty_bucket(rules: &ItemRules<'_>, stack: &ItemStack) -> bool {
    rules
        .items
        .name_of(stack.item_id)
        .is_some_and(|name| name.as_str() == "minecraft:bucket")
}

fn fuel_ticks(rules: &ItemRules<'_>, kind: CookingKind, fuel: &ItemStack) -> i32 {
    if fuel.is_empty() {
        return 0;
    }
    furnace_fuel_ticks(rules.tags, kind, fuel.item_id).map_or(0, i32::from)
}

/// Lava bucket burns into an empty bucket.
fn fuel_remainder(rules: &ItemRules<'_>, fuel: &ItemStack) -> Option<ItemStack> {
    let name = rules.items.name_of(fuel.item_id)?;
    (name.as_str() == "minecraft:lava_bucket").then_some(())?;
    let bucket = rules
        .items
        .id_of(&Identifier::parse("minecraft:bucket").ok()?)?;
    Some(ItemStack::new(bucket, 1))
}

fn can_burn(rules: &ItemRules<'_>, recipe: &Recipe, slots: &[ItemStack]) -> bool {
    let Some(result) = recipe.result.to_stack(rules.items, rules.facts) else {
        return false;
    };
    let output = &slots[OUTPUT];
    if output.is_empty() {
        return true;
    }
    if !can_stack(output, &result) {
        return false;
    }
    let total = output.count + result.count;
    if total <= FURNACE_MAX_STACK && total <= rules.max_stack(output) {
        return true;
    }
    total <= rules.max_stack(&result)
}

fn burn(rules: &ItemRules<'_>, recipe: &Recipe, slots: &mut [ItemStack]) {
    let Some(result) = recipe.result.to_stack(rules.items, rules.facts) else {
        return;
    };
    if slots[OUTPUT].is_empty() {
        slots[OUTPUT] = result;
    } else {
        slots[OUTPUT].count += result.count;
    }
    let wet_sponge = rules
        .items
        .name_of(slots[INPUT].item_id)
        .is_some_and(|name| name.as_str() == "minecraft:wet_sponge");
    if wet_sponge
        && is_empty_bucket(rules, &slots[FUEL])
        && let Some(water) = Identifier::parse("minecraft:water_bucket")
            .ok()
            .and_then(|id| rules.items.id_of(&id))
    {
        slots[FUEL] = ItemStack::new(water, 1);
    }
    decrement_stack(&mut slots[INPUT]);
}
