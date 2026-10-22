//! Vanilla crafting recipe reader.
//!
//! This loader keeps the data sidecar boundary in `mc-data`: it parses
//! shaped, shapeless, cooking, and bounded stonecutting recipe JSON into
//! small Solaris data types without executing crafting or depending on Play
//! inventory packets.

mod armor_dye;
pub mod work;
pub use armor_dye::armor_dye_crafting_result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::{Identifier, read_json_file, visit_json_files};

const REQUIRED_RECIPES: &str = include_str!("../data/required_recipes.json");

#[derive(Debug, Error)]
pub enum RecipeDataError {
    #[error("recipe data io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("recipe data parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error("invalid shaped recipe key {key:?} in {path}")]
    InvalidKey { path: PathBuf, key: String },
    #[error("shaped recipe {path} references missing key {key:?}")]
    MissingKey { path: PathBuf, key: char },
    #[error("invalid ingredient in {path}: alternatives must not be empty")]
    EmptyIngredient { path: PathBuf },
    #[error(
        "invalid shaped recipe pattern in {path}: pattern must be a non-empty 1..=3 by 1..=3 character grid"
    )]
    InvalidPattern { path: PathBuf },
    #[error("shaped recipe {path} declares unused key {key:?}")]
    UnusedKey { path: PathBuf, key: char },
    #[error("invalid shapeless recipe ingredient count {count} in {path}: expected 1..=9")]
    InvalidIngredientCount { path: PathBuf, count: usize },
    #[error("invalid recipe result count {count} in {path}: expected a positive count")]
    InvalidResultCount { path: PathBuf, count: u32 },
    #[error("invalid cooking time {value} in {path}: expected a positive tick count")]
    InvalidCookingTime { path: PathBuf, value: u32 },
    #[error("invalid cooking experience {value} in {path}")]
    InvalidCookingExperience { path: PathBuf, value: f64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub id: Identifier,
    pub kind: RecipeKind,
    pub result: RecipeResult,
    pub show_notification: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeKind {
    Shaped(ShapedRecipe),
    Shapeless(ShapelessRecipe),
    Smelting(SmeltingRecipe),
    Blasting(SmeltingRecipe),
    Smoking(SmeltingRecipe),
    CampfireCooking(SmeltingRecipe),
    Stonecutting(StonecuttingRecipe),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapedRecipe {
    pub pattern: Vec<String>,
    pub key: BTreeMap<char, Ingredient>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapelessRecipe {
    pub ingredients: Vec<Ingredient>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmeltingRecipe {
    pub ingredient: Ingredient,
    pub cooking_time: u32,
    pub experience_milli: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StonecuttingRecipe {
    pub ingredient: Ingredient,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingredient {
    pub alternatives: Vec<IngredientAlternative>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngredientAlternative {
    Item(Identifier),
    Tag(Identifier),
}

#[must_use]
pub fn ingredient_accepts_item(
    items: &crate::items::ItemRegistry,
    tags: &crate::tags::TagsData,
    item_id: u32,
    ingredient: &Ingredient,
) -> bool {
    ingredient
        .alternatives
        .iter()
        .any(|alternative| match alternative {
            IngredientAlternative::Item(item) => items.id_of(item) == Some(item_id),
            IngredientAlternative::Tag(tag) => i32::try_from(item_id)
                .ok()
                .is_some_and(|raw_id| tags.contains_raw_id("minecraft:item", tag.as_str(), raw_id)),
        })
}

#[must_use]
pub fn ingredient_accepts_stack(
    items: &crate::items::ItemRegistry,
    item_facts: &crate::item_components::ItemFactsTable,
    tags: &crate::tags::TagsData,
    stack: &crate::ItemStack,
    ingredient: &Ingredient,
) -> bool {
    if let Some(custom) = stack
        .item_model
        .as_deref()
        .and_then(|id| item_facts.custom(id))
    {
        return item_facts.custom_for_stack(stack, items).is_some()
            && ingredient.alternatives.iter().any(
                |alternative| matches!(alternative, IngredientAlternative::Item(id) if id == &custom.id),
            );
    }
    ingredient_accepts_item(items, tags, stack.item_id, ingredient)
}

#[must_use]
pub fn stonecutting_recipe_accepts_input(
    recipe: &Recipe,
    items: &crate::items::ItemRegistry,
    item_facts: &crate::item_components::ItemFactsTable,
    tags: &crate::tags::TagsData,
    input: &crate::ItemStack,
) -> bool {
    if input.is_empty() {
        return false;
    }
    let RecipeKind::Stonecutting(stonecutting) = &recipe.kind else {
        return false;
    };
    if recipe.result.item.as_str() == "minecraft:air" {
        return false;
    }
    let Some(result_item_id) = items.id_of(&recipe.result.item) else {
        return false;
    };
    let Ok(count) = i32::try_from(recipe.result.count) else {
        return false;
    };
    let result = crate::ItemStack::new(result_item_id, count);
    if count <= 0
        || count > crate::item_semantics_26_1_2::max_stack_for_stack(item_facts, items, &result)
    {
        return false;
    }
    ingredient_accepts_item(items, tags, input.item_id, &stonecutting.ingredient)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookingKind {
    Furnace,
    Smoker,
    BlastFurnace,
}

#[must_use]
pub fn find_cooking_recipe_for_item(
    recipes: &[Recipe],
    items: &crate::items::ItemRegistry,
    tags: &crate::tags::TagsData,
    kind: CookingKind,
    item_id: u32,
) -> Option<Recipe> {
    recipes.iter().find_map(|recipe| {
        let cooking = match (&recipe.kind, kind) {
            (RecipeKind::Smelting(cooking), CookingKind::Furnace) => cooking,
            (RecipeKind::Smoking(cooking), CookingKind::Smoker) => cooking,
            (RecipeKind::Blasting(cooking), CookingKind::BlastFurnace) => cooking,
            _ => return None,
        };
        ingredient_accepts_item(items, tags, item_id, &cooking.ingredient).then(|| recipe.clone())
    })
}

#[must_use]
pub fn furnace_fuel_ticks(
    tags: &crate::tags::TagsData,
    kind: CookingKind,
    item_id: u32,
) -> Option<i16> {
    let ticks = tags.fuel_values().burn_duration(item_id)?;
    Some(match kind {
        CookingKind::Furnace => ticks,
        CookingKind::Smoker | CookingKind::BlastFurnace => ticks / 2,
    })
}

#[must_use]
pub fn shaped_recipe_matches(
    items: &crate::items::ItemRegistry,
    item_facts: &crate::item_components::ItemFactsTable,
    tags: &crate::tags::TagsData,
    input: &[crate::ItemStack; 9],
    shaped: &ShapedRecipe,
) -> bool {
    let height = shaped.pattern.len();
    let width = shaped
        .pattern
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    if height == 0 || width == 0 || height > 3 || width > 3 {
        return false;
    }
    for top in 0..=(3 - height) {
        for left in 0..=(3 - width) {
            'orientation: for mirrored in [false, true] {
                for row in 0..3 {
                    for col in 0..3 {
                        let stack = &input[row * 3 + col];
                        let ingredient = if row >= top
                            && row < top + height
                            && col >= left
                            && col < left + width
                        {
                            let pattern_col = if mirrored {
                                width - 1 - (col - left)
                            } else {
                                col - left
                            };
                            shaped
                                .pattern
                                .get(row - top)
                                .and_then(|pattern_row| pattern_row.chars().nth(pattern_col))
                                .filter(|ch| *ch != ' ')
                                .and_then(|ch| shaped.key.get(&ch))
                        } else {
                            None
                        };
                        match ingredient {
                            Some(ingredient)
                                if !stack.is_empty()
                                    && ingredient_accepts_stack(
                                        items, item_facts, tags, stack, ingredient,
                                    ) => {}
                            None if stack.is_empty() => {}
                            _ => continue 'orientation,
                        }
                    }
                }
                return true;
            }
        }
    }
    false
}

#[must_use]
pub fn shapeless_recipe_matches(
    items: &crate::items::ItemRegistry,
    item_facts: &crate::item_components::ItemFactsTable,
    tags: &crate::tags::TagsData,
    input: &[crate::ItemStack; 9],
    shapeless: &ShapelessRecipe,
) -> bool {
    let stacks: Vec<_> = input.iter().filter(|stack| !stack.is_empty()).collect();
    if stacks.len() != shapeless.ingredients.len() {
        return false;
    }
    let mut used = vec![false; shapeless.ingredients.len()];
    for stack in stacks {
        let Some((idx, _)) = shapeless
            .ingredients
            .iter()
            .enumerate()
            .find(|(idx, ingredient)| {
                !used[*idx] && ingredient_accepts_stack(items, item_facts, tags, stack, ingredient)
            })
        else {
            return false;
        };
        used[idx] = true;
    }
    true
}

#[must_use]
pub fn crafting_recipe_matches(
    items: &crate::items::ItemRegistry,
    item_facts: &crate::item_components::ItemFactsTable,
    tags: &crate::tags::TagsData,
    input: &[crate::ItemStack; 9],
    recipe: &Recipe,
) -> bool {
    match &recipe.kind {
        RecipeKind::Shaped(shaped) => shaped_recipe_matches(items, item_facts, tags, input, shaped),
        RecipeKind::Shapeless(shapeless) => {
            shapeless_recipe_matches(items, item_facts, tags, input, shapeless)
        }
        RecipeKind::Smelting(_)
        | RecipeKind::Blasting(_)
        | RecipeKind::Smoking(_)
        | RecipeKind::CampfireCooking(_)
        | RecipeKind::Stonecutting(_) => false,
    }
}

#[must_use]
pub fn repair_item_crafting_result(
    items: &crate::items::ItemRegistry,
    item_facts: &crate::item_components::ItemFactsTable,
    input: &[crate::ItemStack; 9],
) -> Option<crate::ItemStack> {
    let mut occupied = input.iter().filter(|stack| !stack.is_empty());
    let first = occupied.next()?;
    let second = occupied.next()?;
    if occupied.next().is_some()
        || first.item_id != second.item_id
        || first.item_model != second.item_model
        || first.count != 1
        || second.count != 1
    {
        return None;
    }
    let first_damage = first.damage?;
    let second_damage = second.damage?;
    let max_damage = i32::try_from(item_facts.facts_for_stack(first, items)?.max_damage?).ok()?;
    if max_damage <= 0 {
        return None;
    }
    let remaining = max_damage.saturating_sub(first_damage)
        + max_damage.saturating_sub(second_damage)
        + max_damage / 20;
    let mut repaired = first.clone();
    repaired.enchantments.clear();
    repaired.damage = Some((max_damage - remaining).max(0));
    Some(repaired)
}

#[must_use]
pub fn crafting_result_from_input(
    items: &crate::items::ItemRegistry,
    item_facts: &crate::item_components::ItemFactsTable,
    tags: &crate::tags::TagsData,
    recipes: &[Recipe],
    input: &[crate::ItemStack; 9],
) -> crate::ItemStack {
    if let Some(result) = armor_dye_crafting_result(items, input)
        .or_else(|| repair_item_crafting_result(items, item_facts, input))
    {
        return result;
    }
    recipes
        .iter()
        .find(|recipe| crafting_recipe_matches(items, item_facts, tags, input, recipe))
        .and_then(|recipe| recipe.result.to_stack(items, item_facts))
        .unwrap_or(crate::ItemStack::EMPTY)
}

#[must_use]
pub fn furnace_experience_award(
    recipes: &[Recipe],
    recipes_used: &BTreeMap<String, i32>,
    seed: u64,
) -> i32 {
    let mut total = 0_u64;
    for (recipe_id, count) in recipes_used {
        let Ok(count) = u64::try_from(*count) else {
            continue;
        };
        let Some(recipe) = recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == recipe_id)
        else {
            continue;
        };
        let experience_milli = match &recipe.kind {
            RecipeKind::Smelting(recipe)
            | RecipeKind::Blasting(recipe)
            | RecipeKind::Smoking(recipe) => recipe.experience_milli,
            RecipeKind::Shaped(_)
            | RecipeKind::Shapeless(_)
            | RecipeKind::CampfireCooking(_)
            | RecipeKind::Stonecutting(_) => continue,
        };
        let scaled = count.saturating_mul(u64::from(experience_milli));
        let mut recipe_award = scaled / 1_000;
        let remainder = scaled % 1_000;
        if remainder > 0 {
            let mut recipe_seed = seed ^ 0xCBF2_9CE4_8422_2325;
            for byte in recipe_id.bytes() {
                recipe_seed ^= u64::from(byte);
                recipe_seed = recipe_seed.wrapping_mul(0x0000_0100_0000_01B3);
            }
            if splitmix64(recipe_seed) % 1_000 < remainder {
                recipe_award = recipe_award.saturating_add(1);
            }
        }
        total = total.saturating_add(recipe_award);
    }
    total.min(i32::MAX as u64) as i32
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeResult {
    pub item: Identifier,
    pub count: u32,
    pub stew_effects: Vec<crate::item_stack::StewEffect>,
}

impl RecipeResult {
    #[must_use]
    pub fn to_stack(
        &self,
        items: &crate::items::ItemRegistry,
        item_facts: &crate::item_components::ItemFactsTable,
    ) -> Option<crate::ItemStack> {
        let count = i32::try_from(self.count).ok().filter(|count| *count > 0)?;
        if let Some(custom) = item_facts.custom(&self.item) {
            let item_id = items.id_of(&custom.carrier)?;
            return Some(
                crate::ItemStack::new(item_id, count)
                    .with_item_model(custom.id.clone())
                    .with_custom_name(&custom.name),
            );
        }
        let item_id = items.id_of(&self.item)?;
        let mut stack = crate::ItemStack::new(item_id, count);
        stack.stew_effects = self.stew_effects.clone();
        Some(stack)
    }
}

/// Load shaped, shapeless, cooking, and simple stonecutting recipes from
/// `<data>/minecraft/recipe`. Shaped patterns discard blank outer rows and
/// columns so grid fitting and manual placement use the same vanilla bounds.
///
/// Stonecutting is deliberately bounded to one ingredient plus an id/count
/// result. Component-bearing outputs and unsupported recipe types are skipped,
/// so Solaris never advertises an output it cannot reproduce. A missing
/// directory is not an error: startup keeps working without sidecar recipe data.
pub fn load_recipes(recipe_dir: impl AsRef<Path>) -> Result<Vec<Recipe>, RecipeDataError> {
    let recipe_dir = recipe_dir.as_ref();
    if !recipe_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    visit_json_files(
        recipe_dir,
        &mut |path| {
            paths.push(path);
            Ok(())
        },
        &|path, source| RecipeDataError::Io { path, source },
    )?;
    paths.sort();

    let mut recipes = Vec::new();
    for path in paths {
        if let Some(recipe) = load_one_recipe(recipe_dir, &path)? {
            recipes.push(recipe);
        }
    }
    Ok(recipes)
}

/// Repo-owned crafting baseline for running Solaris without a local recipe sidecar.
#[must_use]
pub fn solaris_required_recipes() -> Vec<Recipe> {
    let path = Path::new("crates/mc-data/data/required_recipes.json");
    let mut raw: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(REQUIRED_RECIPES).expect("embedded required recipe JSON is valid");
    // Keep legacy fallback display ids stable while adding the vanilla-id recipe.
    let bone_meal = raw.remove("minecraft:bone_meal");
    let mut recipes = raw
        .into_iter()
        .map(|(id, value)| {
            let id = Identifier::parse(id).expect("embedded required recipe id is valid");
            parse_recipe_value(path, id, value)
                .expect("embedded required recipe JSON uses supported recipe shapes")
                .expect("embedded required recipe JSON contains supported recipe types")
        })
        .collect::<Vec<_>>();
    if let Some(value) = bone_meal {
        let id = Identifier::parse("minecraft:bone_meal").expect("static identifier");
        recipes.push(
            parse_recipe_value(path, id, value)
                .expect("embedded bone meal recipe uses a supported shape")
                .expect("embedded bone meal recipe is supported"),
        );
    }
    recipes
}

/// Parse one vanilla-format crafting recipe authored by a plugin. Only
/// `minecraft:crafting_shaped` and `minecraft:crafting_shapeless` are accepted;
/// any other or unsupported shape is an error rather than a skipped recipe, so a
/// plugin never believes a result is craftable when it is not.
pub fn parse_plugin_crafting_recipe(id: Identifier, json: &str) -> Result<Recipe, String> {
    let path = Path::new("plugin crafting recipe");
    let value: serde_json::Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if !matches!(
        kind,
        "minecraft:crafting_shaped" | "minecraft:crafting_shapeless"
    ) {
        return Err(format!("unsupported plugin crafting recipe type {kind:?}"));
    }
    parse_recipe_value(path, id, value)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "plugin crafting recipe is unsupported".to_owned())
}

fn load_one_recipe(root: &Path, path: &Path) -> Result<Option<Recipe>, RecipeDataError> {
    let id = id_from_file(root, path)?;
    let value: serde_json::Value = read_json_file(
        path,
        &|path, source| RecipeDataError::Io { path, source },
        &|path, source| RecipeDataError::Parse { path, source },
    )?;
    parse_recipe_value(path, id, value)
}

fn parse_recipe_value(
    path: &Path,
    id: Identifier,
    value: serde_json::Value,
) -> Result<Option<Recipe>, RecipeDataError> {
    let header: RawRecipeHeader = from_value(path, value.clone())?;
    let mut recipe = match header.kind.as_str() {
        "minecraft:crafting_shaped" => {
            let raw: RawShapedRecipe = from_value(path, value)?;
            Ok(Some(parse_shaped_recipe(path, id, raw)?))
        }
        "minecraft:crafting_shapeless" => {
            let raw: RawShapelessRecipe = from_value(path, value)?;
            Ok(Some(parse_shapeless_recipe(path, id, raw)?))
        }
        "minecraft:smelting"
        | "minecraft:blasting"
        | "minecraft:smoking"
        | "minecraft:campfire_cooking" => {
            let raw: RawSmeltingRecipe = from_value(path, value)?;
            Ok(Some(parse_smelting_recipe(path, id, raw, header.kind)?))
        }
        "minecraft:stonecutting" if stonecutting_result_is_supported(&value) => {
            let raw: RawStonecuttingRecipe = from_value(path, value)?;
            Ok(Some(Recipe {
                id,
                show_notification: true,
                kind: RecipeKind::Stonecutting(StonecuttingRecipe {
                    ingredient: parse_ingredient(path, raw.ingredient)?,
                }),
                result: parse_result(path, raw.result)?,
            }))
        }
        _ => Ok(None),
    }?;
    if let Some(recipe) = &mut recipe {
        recipe.show_notification = header.show_notification;
    }
    Ok(recipe)
}

fn stonecutting_result_is_supported(value: &serde_json::Value) -> bool {
    let Some(result) = value.get("result") else {
        return false;
    };
    match result {
        serde_json::Value::String(_) => true,
        serde_json::Value::Object(fields) => fields
            .keys()
            .all(|key| matches!(key.as_str(), "id" | "count")),
        _ => false,
    }
}

fn parse_shaped_recipe(
    path: &Path,
    id: Identifier,
    mut raw: RawShapedRecipe,
) -> Result<Recipe, RecipeDataError> {
    validate_pattern(path, &raw.pattern)?;
    trim_shaped_pattern(&mut raw.pattern);

    let mut key = BTreeMap::new();
    for (raw_key, raw_ingredient) in raw.key {
        let mut chars = raw_key.chars();
        let Some(ch) = chars.next() else {
            return Err(RecipeDataError::InvalidKey {
                path: path.to_path_buf(),
                key: raw_key,
            });
        };
        if chars.next().is_some() || ch == ' ' {
            return Err(RecipeDataError::InvalidKey {
                path: path.to_path_buf(),
                key: raw_key,
            });
        }
        key.insert(ch, parse_ingredient(path, raw_ingredient)?);
    }

    let mut used_keys = BTreeMap::new();
    for row in &raw.pattern {
        for ch in row.chars().filter(|ch| *ch != ' ') {
            if !key.contains_key(&ch) {
                return Err(RecipeDataError::MissingKey {
                    path: path.to_path_buf(),
                    key: ch,
                });
            }
            used_keys.insert(ch, ());
        }
    }
    if let Some((&unused, _)) = key.iter().find(|(ch, _)| !used_keys.contains_key(ch)) {
        return Err(RecipeDataError::UnusedKey {
            path: path.to_path_buf(),
            key: unused,
        });
    }

    Ok(Recipe {
        id,
        show_notification: true,
        kind: RecipeKind::Shaped(ShapedRecipe {
            pattern: raw.pattern,
            key,
        }),
        result: parse_result(path, raw.result)?,
    })
}

fn parse_shapeless_recipe(
    path: &Path,
    id: Identifier,
    raw: RawShapelessRecipe,
) -> Result<Recipe, RecipeDataError> {
    if !(1..=9).contains(&raw.ingredients.len()) {
        return Err(RecipeDataError::InvalidIngredientCount {
            path: path.to_path_buf(),
            count: raw.ingredients.len(),
        });
    }
    let ingredients = raw
        .ingredients
        .into_iter()
        .map(|ingredient| parse_ingredient(path, ingredient))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Recipe {
        id,
        show_notification: true,
        kind: RecipeKind::Shapeless(ShapelessRecipe { ingredients }),
        result: parse_result(path, raw.result)?,
    })
}

fn parse_smelting_recipe(
    path: &Path,
    id: Identifier,
    raw: RawSmeltingRecipe,
    raw_kind: String,
) -> Result<Recipe, RecipeDataError> {
    if raw.cooking_time == 0 {
        return Err(RecipeDataError::InvalidCookingTime {
            path: path.to_path_buf(),
            value: raw.cooking_time,
        });
    }
    let smelting = SmeltingRecipe {
        ingredient: parse_ingredient(path, raw.ingredient)?,
        cooking_time: raw.cooking_time,
        experience_milli: cooking_experience_milli(path, raw.experience)?,
    };
    let kind = match raw_kind.as_str() {
        "minecraft:smelting" => RecipeKind::Smelting(smelting),
        "minecraft:blasting" => RecipeKind::Blasting(smelting),
        "minecraft:smoking" => RecipeKind::Smoking(smelting),
        "minecraft:campfire_cooking" => RecipeKind::CampfireCooking(smelting),
        _ => unreachable!("caller filters supported cooking recipe types"),
    };
    Ok(Recipe {
        id,
        show_notification: true,
        kind,
        result: parse_result(path, raw.result)?,
    })
}

/// Vanilla shaped patterns use the occupied bounding rectangle; whitespace
/// inside that rectangle remains a required empty crafting slot.
fn trim_shaped_pattern(pattern: &mut Vec<String>) {
    let first_row = pattern
        .iter()
        .position(|row| row.chars().any(|ch| ch != ' '));
    let last_row = pattern
        .iter()
        .rposition(|row| row.chars().any(|ch| ch != ' '));
    let (Some(first_row), Some(last_row)) = (first_row, last_row) else {
        return;
    };
    pattern.truncate(last_row + 1);
    pattern.drain(..first_row);
    let left = pattern
        .iter()
        .filter_map(|row| row.chars().position(|ch| ch != ' '))
        .min()
        .expect("validated pattern has an occupied cell");
    let right = pattern
        .iter()
        .filter_map(|row| {
            row.chars()
                .enumerate()
                .filter(|(_, ch)| *ch != ' ')
                .map(|(col, _)| col)
                .last()
        })
        .max()
        .expect("validated pattern has an occupied cell");
    for row in pattern {
        let start = row
            .char_indices()
            .nth(left)
            .map_or(row.len(), |(idx, _)| idx);
        let end = row
            .char_indices()
            .nth(right + 1)
            .map_or(row.len(), |(idx, _)| idx);
        row.truncate(end);
        row.drain(..start);
    }
}

fn validate_pattern(path: &Path, pattern: &[String]) -> Result<(), RecipeDataError> {
    if !(1..=3).contains(&pattern.len()) {
        return Err(RecipeDataError::InvalidPattern {
            path: path.to_path_buf(),
        });
    }
    let Some(width) = pattern.first().map(|row| row.chars().count()) else {
        return Err(RecipeDataError::InvalidPattern {
            path: path.to_path_buf(),
        });
    };
    if !(1..=3).contains(&width)
        || pattern.iter().any(|row| row.chars().count() != width)
        || pattern.iter().all(|row| row.chars().all(|ch| ch == ' '))
    {
        return Err(RecipeDataError::InvalidPattern {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn parse_ingredient(path: &Path, raw: RawIngredient) -> Result<Ingredient, RecipeDataError> {
    let alternatives = match raw {
        RawIngredient::Single(value) => vec![parse_ingredient_alternative(path, value)?],
        RawIngredient::Alternatives(values) => values
            .into_iter()
            .map(|value| parse_ingredient_alternative(path, value))
            .collect::<Result<Vec<_>, _>>()?,
    };
    if alternatives.is_empty() {
        return Err(RecipeDataError::EmptyIngredient {
            path: path.to_path_buf(),
        });
    }
    Ok(Ingredient { alternatives })
}

fn parse_ingredient_alternative(
    path: &Path,
    value: RawIngredientValue,
) -> Result<IngredientAlternative, RecipeDataError> {
    match value {
        RawIngredientValue::String(value) => {
            if let Some(tag) = value.strip_prefix('#') {
                parse_id(path, tag.to_string()).map(IngredientAlternative::Tag)
            } else {
                parse_id(path, value).map(IngredientAlternative::Item)
            }
        }
        RawIngredientValue::Object {
            item: Some(item),
            tag: None,
        } => parse_id(path, item).map(IngredientAlternative::Item),
        RawIngredientValue::Object {
            item: None,
            tag: Some(tag),
        } => parse_id(path, tag).map(IngredientAlternative::Tag),
        RawIngredientValue::Object { item, tag } => Err(RecipeDataError::InvalidIdentifier {
            path: path.to_path_buf(),
            value: format!("ingredient object item={item:?} tag={tag:?}"),
        }),
    }
}

fn parse_result(path: &Path, raw: RawResult) -> Result<RecipeResult, RecipeDataError> {
    let (item, count, components) = match raw {
        RawResult::Object {
            id,
            count,
            components,
        } => (id, count, components),
        RawResult::Id(id) => (id, default_count(), RawResultComponents::default()),
    };
    if count == 0 {
        return Err(RecipeDataError::InvalidResultCount {
            path: path.to_path_buf(),
            count,
        });
    }
    for effect in &components.stew_effects {
        if crate::mob_effects_26_1_2::MobEffect::from_name(effect.id.as_str()).is_none() {
            return Err(RecipeDataError::InvalidIdentifier {
                path: path.to_path_buf(),
                value: effect.id.as_str().into(),
            });
        }
    }
    Ok(RecipeResult {
        item: parse_id(path, item)?,
        count,
        stew_effects: components.stew_effects,
    })
}

fn id_from_file(root: &Path, path: &Path) -> Result<Identifier, RecipeDataError> {
    let rel = path
        .strip_prefix(root)
        .expect("walk yields paths under recipe root")
        .with_extension("");
    let mut joined = String::from("minecraft:");
    for component in rel.components() {
        if !joined.ends_with(':') {
            joined.push('/');
        }
        joined.push_str(component.as_os_str().to_string_lossy().as_ref());
    }
    parse_id(path, joined)
}

fn parse_id(path: &Path, value: String) -> Result<Identifier, RecipeDataError> {
    Identifier::parse(value.clone()).map_err(|_| RecipeDataError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

fn from_value<T: for<'de> Deserialize<'de>>(
    path: &Path,
    value: serde_json::Value,
) -> Result<T, RecipeDataError> {
    serde_json::from_value(value).map_err(|source| RecipeDataError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn default_count() -> u32 {
    1
}

fn default_cooking_time() -> u32 {
    200
}

fn cooking_experience_milli(path: &Path, experience: f64) -> Result<u32, RecipeDataError> {
    let scaled = experience * 1000.0;
    if !scaled.is_finite() || scaled < 0.0 || scaled > f64::from(u32::MAX) {
        return Err(RecipeDataError::InvalidCookingExperience {
            path: path.to_path_buf(),
            value: experience,
        });
    }
    Ok(scaled.round() as u32)
}

#[derive(Deserialize)]
struct RawRecipeHeader {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default = "default_show_notification")]
    show_notification: bool,
}

fn default_show_notification() -> bool {
    true
}

#[derive(Deserialize)]
struct RawShapedRecipe {
    #[serde(rename = "type")]
    _kind: String,
    pattern: Vec<String>,
    key: BTreeMap<String, RawIngredient>,
    result: RawResult,
}

#[derive(Deserialize)]
struct RawShapelessRecipe {
    #[serde(rename = "type")]
    _kind: String,
    ingredients: Vec<RawIngredient>,
    result: RawResult,
}

#[derive(Deserialize)]
struct RawSmeltingRecipe {
    #[serde(rename = "type")]
    _kind: String,
    ingredient: RawIngredient,
    result: RawResult,
    #[serde(
        default = "default_cooking_time",
        rename = "cookingtime",
        alias = "cooking_time"
    )]
    cooking_time: u32,
    #[serde(default)]
    experience: f64,
}

#[derive(Deserialize)]
struct RawStonecuttingRecipe {
    #[serde(rename = "type")]
    _kind: String,
    ingredient: RawIngredient,
    result: RawResult,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawIngredient {
    Single(RawIngredientValue),
    Alternatives(Vec<RawIngredientValue>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawIngredientValue {
    String(String),
    Object {
        item: Option<String>,
        tag: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawResult {
    Object {
        id: String,
        #[serde(default = "default_count")]
        count: u32,
        #[serde(default)]
        components: RawResultComponents,
    },
    Id(String),
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResultComponents {
    #[serde(default, rename = "minecraft:suspicious_stew_effects")]
    stew_effects: Vec<crate::item_stack::StewEffect>,
}

#[cfg(test)]
#[path = "recipes_tests.rs"]
mod tests;
