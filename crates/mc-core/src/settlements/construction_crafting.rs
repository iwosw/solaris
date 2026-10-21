//! Bounded builder crafting over canonical storage images.
//!
//! Pure planner: borrows storage [`inventory::Image`]s, consumes real stock
//! and adds exact vanilla batch yields (plus surplus) to a cloned post-image.
//! Never writes or commits; the caller transfers the requested unit to
//! builder carry and commits atomically. Any shortage or capacity failure
//! returns `None` and leaves the caller's images untouched.
//!
//! Only grid crafting (`Shaped`/`Shapeless`) from the vanilla registry is
//! used, so furnace-only transformations never apply. Ingredient matching
//! reuses the vanilla helpers, preserving colored wool/wood alternatives.

use super::inventory;
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_data::recipes::{Ingredient, RecipeKind};

/// Deepest allowed craft chain (log -> planks -> door is depth 2).
const MAX_DEPTH: u8 = 8;
/// Total ingredient/batch operations across one `craft` call.
const MAX_WORK: usize = 4096;
/// Batches for a single recipe step; a full carried stack needs up to 22.
const MAX_BATCHES: i32 = 32;
/// Native container slot cap mirrors the fence ledger (`min(99)`).
const MAX_SLOT: i32 = 99;

/// Plan post-craft slots in exact image order, guaranteeing one unit of
/// `item` across the returned images. Returns `None` when stock, recipes or
/// room cannot cover it. Already-stocked items return unchanged slot clones.
pub(crate) fn craft(
    state: &TickState,
    item: u32,
    images: &[inventory::Image],
) -> Option<Vec<Vec<ItemStack>>> {
    let mut post: Vec<Vec<ItemStack>> = images.iter().map(|image| image.slots.clone()).collect();
    if total(&post, item) >= 1 {
        return Some(post);
    }
    if post.is_empty() {
        return None;
    }
    state.registry.items.name_of(item)?;
    let mut work = 0usize;
    if ensure(state, &mut post, &mut work, item, 1, 0) && total(&post, item) >= 1 {
        Some(post)
    } else {
        None
    }
}

/// Craft until `need` units of `item` exist in `post` (a haul of many units).
pub(crate) fn ensure_units(
    state: &TickState,
    post: &mut Vec<Vec<ItemStack>>,
    item: u32,
    need: i32,
) -> bool {
    let mut work = 0;
    ensure(state, post, &mut work, item, need, 0)
}

/// Single-authority probe: delegates to [`craft`] so no second recipe
/// authority can diverge.
pub(crate) fn can_craft(state: &TickState, item: u32, images: &[inventory::Image]) -> bool {
    craft(state, item, images).is_some()
}

/// Read-only funding proof for a whole upgrade. Virtual consumption prevents
/// the same logs/intermediate recipe surplus from funding two requirements.
pub(crate) fn can_fund(
    state: &TickState,
    requirements: &[(u32, u32)],
    images: &[inventory::Image],
) -> bool {
    if requirements
        .iter()
        .map(|(_, count)| u64::from(*count))
        .sum::<u64>()
        > 65_536
    {
        return false;
    }
    let mut post: Vec<_> = images.iter().map(|image| image.slots.clone()).collect();
    for (item, count) in requirements {
        let mut remaining = *count;
        while remaining > 0 {
            // The executor crafts one requested unit per haul. Match that
            // bounded search budget while retaining the shared virtual stock,
            // rather than rejecting a fundable building after many cells.
            let mut work = 0;
            if !ensure(state, &mut post, &mut work, *item, 1, 0) {
                return false;
            }
            for stack in post
                .iter_mut()
                .flatten()
                .filter(|stack| stack.item_id == *item && stack.count > 0)
            {
                let take = remaining.min(stack.count as u32);
                remaining -= take;
                stack.count -= take as i32;
                if stack.count == 0 {
                    *stack = ItemStack::EMPTY;
                }
                if remaining == 0 {
                    break;
                }
            }
        }
    }
    true
}

/// Simple stock gap for explicit requirements: `(item, still-missing)` in
/// input order. Pure counting, no recipe logic.
pub(crate) fn missing_base(
    _state: &TickState,
    requirements: &[(u32, u32)],
    images: &[inventory::Image],
) -> Vec<(u32, u32)> {
    let mut missing = Vec::new();
    for &(item, need) in requirements {
        let need = u64::from(need);
        let mut have = 0u64;
        for image in images {
            for stack in &image.slots {
                if !stack.is_empty() && stack.item_id == item {
                    have += u64::try_from(stack.count.max(0)).unwrap_or(0);
                }
            }
        }
        if have < need {
            missing.push((item, u32::try_from(need - have).unwrap_or(u32::MAX)));
        }
    }
    missing
}

fn total(post: &[Vec<ItemStack>], item: u32) -> i32 {
    let mut sum = 0i32;
    for image in post {
        for stack in image {
            if !stack.is_empty() && stack.item_id == item {
                sum = sum.saturating_add(stack.count.max(0));
            }
        }
    }
    sum
}

/// Consume one unit satisfying a vanilla ingredient, scanning images then
/// slots in order for determinism.
fn take_matching(state: &TickState, post: &mut [Vec<ItemStack>], ingredient: &Ingredient) -> bool {
    for image in post.iter_mut() {
        for slot in image.iter_mut() {
            if slot.is_empty() {
                continue;
            }
            if !mc_data::recipes::ingredient_accepts_stack(
                &state.registry.items,
                &state.registry.item_facts,
                &state.registry.tags,
                slot,
                ingredient,
            ) {
                continue;
            }
            slot.count -= 1;
            if slot.count <= 0 {
                *slot = ItemStack::EMPTY;
            }
            return true;
        }
    }
    false
}
/// Capacity-checked insert mirroring the native ledger: merge first, then
/// empty slots, both in image/slot order.
pub(crate) fn add_stack(
    state: &TickState,
    post: &mut [Vec<ItemStack>],
    template: &ItemStack,
) -> bool {
    let mut remaining = template.count;
    if remaining <= 0 {
        return false;
    }
    let max = state
        .registry
        .item_rules()
        .max_stack(template)
        .min(MAX_SLOT);
    if max <= 0 {
        return false;
    }
    for empty in [false, true] {
        for image in post.iter_mut() {
            for slot in image.iter_mut() {
                if remaining <= 0 {
                    return true;
                }
                if slot.is_empty() != empty {
                    continue;
                }
                if empty {
                    let place = remaining.min(max);
                    let mut placed = template.clone();
                    placed.count = place;
                    *slot = placed;
                    remaining -= place;
                } else {
                    if !mc_data::inventory_semantics_26_1_2::can_stack(slot, template) {
                        continue;
                    }
                    if slot.count >= max {
                        continue;
                    }
                    let place = (max - slot.count).min(remaining);
                    if place <= 0 {
                        continue;
                    }
                    slot.count += place;
                    remaining -= place;
                }
            }
        }
    }
    remaining <= 0
}

/// Expanded one-unit-per-entry ingredient list; `None` for non-grid or
/// malformed recipes (never crafted here).
fn expanded(recipe: &mc_data::recipes::Recipe) -> Option<Vec<Ingredient>> {
    match &recipe.kind {
        RecipeKind::Shaped(shaped) => {
            let mut out = Vec::new();
            for row in &shaped.pattern {
                for ch in row.chars() {
                    if ch == ' ' {
                        continue;
                    }
                    out.push(shaped.key.get(&ch)?.clone());
                }
            }
            if out.is_empty() || out.len() > 9 {
                return None;
            }
            Some(out)
        }
        RecipeKind::Shapeless(shapeless) => {
            if shapeless.ingredients.is_empty() || shapeless.ingredients.len() > 9 {
                return None;
            }
            Some(shapeless.ingredients.clone())
        }
        _ => None,
    }
}

fn is_grid(recipe: &mc_data::recipes::Recipe) -> bool {
    recipe.id.as_str().starts_with("minecraft:")
        && matches!(
            recipe.kind,
            RecipeKind::Shaped(_) | RecipeKind::Shapeless(_)
        )
}

/// Concrete result identity; custom-model results match by carrier id.
fn result_identity(state: &TickState, recipe: &mc_data::recipes::Recipe) -> Option<(u32, i32)> {
    let count = i32::try_from(recipe.result.count).ok().filter(|c| *c > 0)?;
    if let Some(id) = state.registry.items.id_of(&recipe.result.item) {
        return Some((id, count));
    }
    let stack = recipe
        .result
        .to_stack(&state.registry.items, &state.registry.item_facts)?;
    if stack.count <= 0 {
        return None;
    }
    Some((stack.item_id, count))
}

/// Ensure `need` units of a concrete item exist in `post`, crafting grid
/// batches recursively. Tries every vanilla recipe for the item in registry
/// order with backtracking; surplus from exact batch yields stays in `post`.
fn ensure(
    state: &TickState,
    post: &mut Vec<Vec<ItemStack>>,
    work: &mut usize,
    target: u32,
    need: i32,
    depth: u8,
) -> bool {
    if need <= 0 {
        return true;
    }
    if depth > MAX_DEPTH || *work > MAX_WORK {
        return false;
    }
    if total(post, target) >= need {
        return true;
    }
    let deficit = need - total(post, target);
    for recipe in state.registry.recipes.iter() {
        if !is_grid(recipe) {
            continue;
        }
        let Some((id, yield_count)) = result_identity(state, recipe) else {
            continue;
        };
        if id != target || yield_count <= 0 {
            continue;
        }
        let batches = deficit / yield_count + i32::from(deficit % yield_count != 0);
        if batches <= 0 || batches > MAX_BATCHES {
            continue;
        }
        let Some(ingredients) = expanded(recipe) else {
            continue;
        };
        let snapshot = post.clone();
        let mut ok = true;
        for _ in 0..batches {
            *work = work.saturating_add(1);
            if *work > MAX_WORK {
                ok = false;
                break;
            }
            for ingredient in &ingredients {
                *work = work.saturating_add(1);
                if *work > MAX_WORK {
                    ok = false;
                    break;
                }
                if take_matching(state, post, ingredient) {
                    continue;
                }
                if !ensure_ingredient(state, post, work, ingredient, depth + 1) {
                    ok = false;
                    break;
                }
            }
            if !ok {
                break;
            }
            let Some(output) = recipe
                .result
                .to_stack(&state.registry.items, &state.registry.item_facts)
            else {
                ok = false;
                break;
            };
            if output.count <= 0 || !add_stack(state, post, &output) {
                ok = false;
                break;
            }
        }
        if ok && total(post, target) >= need {
            return true;
        }
        *post = snapshot;
        if *work > MAX_WORK {
            return false;
        }
    }
    false
}

/// Satisfy one generic ingredient unit: consume matching stock, else craft a
/// matching grid result (registry order) and consume the crafted unit.
fn ensure_ingredient(
    state: &TickState,
    post: &mut Vec<Vec<ItemStack>>,
    work: &mut usize,
    ingredient: &Ingredient,
    depth: u8,
) -> bool {
    if take_matching(state, post, ingredient) {
        return true;
    }
    if depth > MAX_DEPTH || *work > MAX_WORK {
        return false;
    }
    for recipe in state.registry.recipes.iter() {
        if !is_grid(recipe) {
            continue;
        }
        let Some((id, _)) = result_identity(state, recipe) else {
            continue;
        };
        if !mc_data::recipes::ingredient_accepts_item(
            &state.registry.items,
            &state.registry.tags,
            id,
            ingredient,
        ) {
            continue;
        }
        let snapshot = post.clone();
        if ensure(state, post, work, id, 1, depth + 1) && take_matching(state, post, ingredient) {
            return true;
        }
        *post = snapshot;
        if *work > MAX_WORK {
            return false;
        }
    }
    false
}

#[cfg(test)]
#[path = "construction_crafting_tests.rs"]
mod tests;
