use crate::item_components::ItemFactsTable;
use crate::item_stack::{ArmorTrim, BannerPattern};
use crate::items::ItemRegistry;
use crate::tags::TagsData;
use crate::{Identifier, ItemEnchantment, ItemStack};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub const DYE_COLORS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

#[derive(Deserialize)]
struct Catalogue {
    smithing: Vec<Smithing>,
    brewing: Vec<Brew>,
    potion_ids: Vec<Identifier>,
    repair_items: BTreeMap<String, serde_json::Value>,
    enchantments: BTreeMap<String, Enchantment>,
    item_tags: BTreeMap<String, Vec<Identifier>>,
    enchantment_min_cost: BTreeMap<String, [i32; 2]>,
    enchantment_tags: BTreeMap<String, Vec<Identifier>>,
    max_damage: BTreeMap<String, u32>,
}
#[derive(Deserialize)]
struct Smithing {
    template: serde_json::Value,
    base: serde_json::Value,
    addition: serde_json::Value,
    result: Option<SmithingResult>,
    pattern: Option<Identifier>,
}
#[derive(Deserialize)]
struct SmithingResult {
    id: Identifier,
}
#[derive(Deserialize)]
struct Brew {
    from: String,
    ingredient: String,
    to: String,
    container: bool,
}
#[derive(Deserialize)]
struct Enchantment {
    max_level: i32,
    anvil_cost: i32,
    supported_items: serde_json::Value,
    #[serde(default)]
    exclusive_set: serde_json::Value,
}
static CATALOGUE: LazyLock<Catalogue> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/stations_26_1_2.json"))
        .expect("vanilla station data")
});
fn catalogue() -> &'static Catalogue {
    &CATALOGUE
}
pub fn potion_id(id: &Identifier) -> Option<u32> {
    catalogue()
        .potion_ids
        .iter()
        .position(|entry| entry == id)
        .map(|i| i as u32)
}
pub fn potion_name(id: u32) -> Option<Identifier> {
    catalogue().potion_ids.get(id as usize).cloned()
}
fn identifier(name: &str) -> Identifier {
    Identifier::parse(name).expect("canonical vanilla identifier")
}
fn matches(
    value: &serde_json::Value,
    name: &Identifier,
    raw: u32,
    tags: &TagsData,
    registry: &str,
) -> bool {
    match value {
        serde_json::Value::String(value) => {
            if let Some(tag) = value.strip_prefix('#') {
                tags.contains_raw_id(registry, tag, raw as i32)
                    || (if registry == "minecraft:item" {
                        &catalogue().item_tags
                    } else {
                        &catalogue().enchantment_tags
                    })
                    .get(tag)
                    .is_some_and(|members| members.contains(name))
            } else {
                value == name.as_str()
            }
        }
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| matches(value, name, raw, tags, registry)),
        _ => false,
    }
}
fn input_matches(
    value: &serde_json::Value,
    stack: &ItemStack,
    items: &ItemRegistry,
    tags: &TagsData,
) -> bool {
    !stack.is_empty()
        && items
            .name_of(stack.item_id)
            .is_some_and(|name| matches(value, name, stack.item_id, tags, "minecraft:item"))
}

pub fn smithing_result(
    input: &[ItemStack; 9],
    items: &ItemRegistry,
    tags: &TagsData,
) -> Option<ItemStack> {
    let recipe = catalogue().smithing.iter().find(|r| {
        input_matches(&r.template, &input[0], items, tags)
            && input_matches(&r.base, &input[1], items, tags)
            && input_matches(&r.addition, &input[2], items, tags)
    })?;
    let mut result = input[1].clone();
    result.count = 1;
    if let Some(transform) = &recipe.result {
        result.item_id = items.id_of(&transform.id)?;
    } else {
        let addition = items.name_of(input[2].item_id)?.path();
        let material = match addition {
            "iron_ingot" => "iron",
            "gold_ingot" => "gold",
            "copper_ingot" => "copper",
            "netherite_ingot" => "netherite",
            "amethyst_shard" => "amethyst",
            "lapis_lazuli" => "lapis",
            "resin_brick" => "resin",
            other => other,
        };
        let trim = ArmorTrim {
            material: identifier(&format!("minecraft:{material}")),
            pattern: recipe.pattern.clone()?,
        };
        if result.extended().trim.as_ref() == Some(&trim) {
            return None;
        }
        result.extended_mut().trim = Some(trim);
    }
    Some(result)
}

/// Selectable raw IDs in exactly the banner-pattern tag order sent to the client.
pub fn loom_patterns<'a>(
    pattern_item: &ItemStack,
    items: &ItemRegistry,
    tags: &'a TagsData,
) -> &'a [i32] {
    static REGISTRY: LazyLock<Identifier> =
        LazyLock::new(|| identifier("minecraft:banner_pattern"));
    let Some(registry) = tags.registries.get(&*REGISTRY) else {
        return &[];
    };
    let pattern = if pattern_item.is_empty() {
        None
    } else {
        let Some(pattern) = items
            .name_of(pattern_item.item_id)
            .and_then(|name| name.path().strip_suffix("_banner_pattern"))
        else {
            return &[];
        };
        Some(pattern)
    };
    registry
        .iter()
        .find(|(tag, _)| {
            tag.namespace() == "minecraft"
                && match pattern {
                    None => tag.path() == "no_item_required",
                    Some(pattern) => tag.path().strip_prefix("pattern_item/") == Some(pattern),
                }
        })
        .map_or(&[], |(_, entries)| entries.as_slice())
}
pub fn dye_color(stack: &ItemStack, items: &ItemRegistry) -> Option<i32> {
    let name = items.name_of(stack.item_id)?.path().strip_suffix("_dye")?;
    DYE_COLORS
        .iter()
        .position(|color| *color == name)
        .map(|i| i as i32)
}
pub fn loom_result(
    input: &[ItemStack; 9],
    selected: i32,
    items: &ItemRegistry,
    tags: &TagsData,
) -> Option<ItemStack> {
    if input[0].is_empty()
        || input[1].is_empty()
        || input[0].extended().banner_patterns.len() >= 6
        || !items.name_of(input[0].item_id)?.path().ends_with("_banner")
    {
        return None;
    }
    let patterns = loom_patterns(&input[2], items, tags);
    let selected = if !input[2].is_empty() && patterns.len() == 1 {
        0
    } else {
        usize::try_from(selected).ok()?
    };
    let pattern = tags.banner_pattern(*patterns.get(selected)?)?.clone();
    let mut out = input[0].clone();
    out.count = 1;
    out.extended_mut().banner_patterns.push(BannerPattern {
        pattern,
        color: dye_color(&input[1], items)?,
    });
    Some(out)
}

pub fn brewing_result(
    bottle: &ItemStack,
    ingredient: &ItemStack,
    items: &ItemRegistry,
) -> Option<ItemStack> {
    if bottle.is_empty() || ingredient.is_empty() {
        return None;
    }
    let container = items.name_of(bottle.item_id)?.path();
    if !matches!(container, "potion" | "splash_potion" | "lingering_potion") {
        return None;
    }
    let ingredient = items.name_of(ingredient.item_id)?.path();
    let potion = bottle.extended().potion.as_ref()?.path();
    let r = catalogue().brewing.iter().find(|r| {
        r.ingredient == ingredient && r.from == if r.container { container } else { potion }
    })?;
    let item_id = if r.container {
        items.id_of(&identifier(&format!("minecraft:{}", r.to)))?
    } else {
        bottle.item_id
    };
    let mut out = bottle.clone();
    out.item_id = item_id;
    out.extended_mut().potion = Some(identifier(&format!(
        "minecraft:{}",
        if r.container { potion } else { &r.to }
    )));
    Some(out)
}
pub fn brewing_ingredient(stack: &ItemStack, items: &ItemRegistry) -> bool {
    !stack.is_empty()
        && items.name_of(stack.item_id).is_some_and(|name| {
            catalogue()
                .brewing
                .iter()
                .any(|r| r.ingredient == name.path())
        })
}

#[derive(Debug, Clone)]
pub struct AnvilResult {
    pub stack: ItemStack,
    pub cost: i32,
    pub material_count: i32,
    pub only_rename: bool,
}

pub fn anvil_result(
    left: &ItemStack,
    right: &ItemStack,
    rename: Option<&str>,
    creative: bool,
    items: &ItemRegistry,
    facts: &ItemFactsTable,
    tags: &TagsData,
) -> Option<AnvilResult> {
    if left.is_empty() {
        return None;
    }
    let name = items.name_of(left.item_id)?;
    let max_damage = facts
        .facts_for_stack(left, items)
        .and_then(|f| f.max_damage)
        .or_else(|| catalogue().max_damage.get(name.as_str()).copied())
        .unwrap_or(0) as i32;
    let mut out = left.clone();
    let mut added_cost = 0i32;
    let prior_cost = left
        .extended()
        .repair_cost
        .saturating_add(right.extended().repair_cost);
    let mut material_count = 0;
    let book = items
        .name_of(right.item_id)
        .is_some_and(|name| name.path() == "enchanted_book");
    if !right.is_empty() {
        if max_damage > 0
            && catalogue()
                .repair_items
                .get(name.as_str())
                .is_some_and(|value| input_matches(value, right, items, tags))
        {
            let mut damage = left.damage.unwrap_or(0);
            while damage > 0 && material_count < right.count {
                damage -= damage.min(max_damage / 4);
                material_count += 1;
                added_cost += 1;
            }
            if material_count == 0 {
                return None;
            }
            out.damage = Some(damage);
        } else {
            if !book && (left.item_id != right.item_id || max_damage == 0) {
                return None;
            }
            if !book && max_damage > 0 {
                let damage = (left.damage.unwrap_or(0) + right.damage.unwrap_or(0)
                    - max_damage
                    - max_damage * 12 / 100)
                    .max(0);
                if damage < left.damage.unwrap_or(0) {
                    out.damage = Some(damage);
                    added_cost += 2;
                }
            }
            let incoming = if book {
                &right.extended().stored_enchantments
            } else {
                &right.enchantments
            };
            let output_enchantments = if name.path() == "enchanted_book" {
                &mut out.extended_mut().stored_enchantments
            } else {
                &mut out.enchantments
            };
            let mut accepted = false;
            let mut rejected = false;
            for e in incoming {
                let Some(def) = catalogue().enchantments.get(e.id.path()) else {
                    rejected = true;
                    continue;
                };
                let mut allowed = creative
                    || name.path() == "enchanted_book"
                    || matches(
                        &def.supported_items,
                        name,
                        left.item_id,
                        tags,
                        "minecraft:item",
                    );
                for existing in output_enchantments
                    .iter()
                    .filter(|existing| existing.id != e.id)
                {
                    let raw = crate::required_registry_entry_id("enchantment", &existing.id)
                        .unwrap_or(u32::MAX);
                    if matches(
                        &def.exclusive_set,
                        &existing.id,
                        raw,
                        tags,
                        "minecraft:enchantment",
                    ) {
                        allowed = false;
                        added_cost += 1;
                    }
                }
                if !allowed {
                    rejected = true;
                    continue;
                }
                accepted = true;
                let old = output_enchantments
                    .iter()
                    .find(|existing| existing.id == e.id)
                    .map_or(0, |existing| existing.level);
                let level = if old == e.level {
                    e.level + 1
                } else {
                    old.max(e.level)
                }
                .min(def.max_level);
                output_enchantments.retain(|existing| existing.id != e.id);
                output_enchantments.push(ItemEnchantment {
                    id: e.id.clone(),
                    level,
                });
                added_cost += if book {
                    (def.anvil_cost / 2).max(1)
                } else {
                    def.anvil_cost
                } * level;
                if left.count > 1 {
                    added_cost = 40;
                }
            }
            if rejected && !accepted {
                return None;
            }
            material_count = right.count;
        }
    }
    let mut rename_cost = 0;
    if let Some(rename) = rename {
        if rename.trim().is_empty() {
            if out.custom_name.take().is_some() {
                rename_cost = 1;
            }
        } else if out.custom_name.as_deref() != Some(rename) {
            out.custom_name = Some(rename.into());
            rename_cost = 1;
        }
    }
    added_cost += rename_cost;
    if added_cost == 0 {
        return None;
    }
    let only_rename = rename_cost == added_cost;
    let mut cost = prior_cost.saturating_add(added_cost);
    if only_rename && cost >= 40 {
        cost = 39;
    }
    if cost >= 40 && !creative {
        out = ItemStack::EMPTY;
    }
    out.extended_mut().repair_cost = left
        .extended()
        .repair_cost
        .max(right.extended().repair_cost);
    if !only_rename {
        out.extended_mut().repair_cost = out
            .extended()
            .repair_cost
            .saturating_mul(2)
            .saturating_add(1);
    }
    Some(AnvilResult {
        stack: out,
        cost,
        material_count,
        only_rename,
    })
}

pub fn smithing_input_allowed(
    slot: usize,
    stack: &ItemStack,
    items: &ItemRegistry,
    tags: &TagsData,
) -> bool {
    catalogue().smithing.iter().any(|recipe| {
        input_matches(
            match slot {
                0 => &recipe.template,
                1 => &recipe.base,
                2 => &recipe.addition,
                _ => return false,
            },
            stack,
            items,
            tags,
        )
    })
}

fn is_curse(enchantment: &ItemEnchantment) -> bool {
    catalogue()
        .enchantment_tags
        .get("minecraft:curse")
        .is_some_and(|members| members.contains(&enchantment.id))
}

pub fn grindstone_input_allowed(
    stack: &ItemStack,
    items: &ItemRegistry,
    facts: &ItemFactsTable,
) -> bool {
    !stack.is_empty()
        && (facts
            .facts_for_stack(stack, items)
            .and_then(|f| f.max_damage)
            .is_some()
            || items
                .name_of(stack.item_id)
                .is_some_and(|name| catalogue().max_damage.contains_key(name.as_str()))
            || !stack.enchantments.is_empty()
            || !stack.extended().stored_enchantments.is_empty())
}

/// Disenchant or repair a pair, preserving curses and unrelated stack components.
pub fn grindstone_result(
    left: &ItemStack,
    right: &ItemStack,
    items: &ItemRegistry,
    facts: &ItemFactsTable,
) -> Option<ItemStack> {
    let (first, second) = if left.is_empty() {
        (right, left)
    } else {
        (left, right)
    };
    if !grindstone_input_allowed(first, items, facts) {
        return None;
    }
    let mut out = first.clone();
    if !second.is_empty() {
        if first.item_id != second.item_id || first.count != 1 || second.count != 1 {
            return None;
        }
        let name = items.name_of(first.item_id)?;
        let max = facts
            .facts_for_stack(first, items)
            .and_then(|f| f.max_damage)
            .or_else(|| catalogue().max_damage.get(name.as_str()).copied())
            .unwrap_or(0) as i32;
        if max == 0 {
            if first != second {
                return None;
            }
            out.count = 2;
        } else {
            out.damage = Some(
                (first.damage.unwrap_or(0) + second.damage.unwrap_or(0) - max - max * 5 / 100)
                    .max(0),
            );
        }
        for curse in second.enchantments.iter().filter(|e| is_curse(e)) {
            if !out.enchantments.iter().any(|e| e.id == curse.id) {
                out.enchantments.push(curse.clone());
            }
        }
        for curse in second
            .extended()
            .stored_enchantments
            .iter()
            .filter(|e| is_curse(e))
        {
            if !out
                .extended()
                .stored_enchantments
                .iter()
                .any(|e| e.id == curse.id)
            {
                out.extended_mut().stored_enchantments.push(curse.clone());
            }
        }
    } else if first.enchantments.is_empty() && first.extended().stored_enchantments.is_empty() {
        return None;
    }
    out.enchantments.retain(is_curse);
    out.extended_mut().stored_enchantments.retain(is_curse);
    out.extended_mut().repair_cost = 0;
    let curses = out.enchantments.len() + out.extended().stored_enchantments.len();
    for _ in 0..curses {
        out.extended_mut().repair_cost = out
            .extended()
            .repair_cost
            .saturating_mul(2)
            .saturating_add(1);
    }
    if items
        .name_of(out.item_id)
        .is_some_and(|name| name.path() == "enchanted_book")
        && out.extended().stored_enchantments.is_empty()
    {
        out.item_id = items.id_of(&identifier("minecraft:book"))?;
    }
    Some(out)
}

pub fn grindstone_experience(input: &[ItemStack; 9], seed: u64) -> i32 {
    let sum = input[..2]
        .iter()
        .flat_map(|stack| {
            stack
                .enchantments
                .iter()
                .chain(&stack.extended().stored_enchantments)
        })
        .filter(|e| !is_curse(e))
        .fold(0i32, |sum, e| {
            let cost = catalogue()
                .enchantment_min_cost
                .get(e.id.path())
                .map_or(0, |[base, step]| {
                    base.saturating_add(step.saturating_mul(e.level.saturating_sub(1)))
                });
            sum.saturating_add(cost.max(0))
        });
    let half = sum.saturating_add(1) / 2;
    if half == 0 {
        0
    } else {
        half + (seed % half as u64) as i32
    }
}

/// Compatible canonical enchantments for loot-table enchantment selection.
/// Books accept every candidate; equipment uses the same support tags as anvil.
pub fn random_loot_enchantment_candidates(
    item: &Identifier,
    options: Option<&Identifier>,
) -> Vec<(Identifier, i32)> {
    let data = catalogue();
    let members = options.and_then(|tag| data.enchantment_tags.get(tag.as_str()));
    if options.is_some() && members.is_none() {
        return Vec::new();
    }
    data.enchantments
        .iter()
        .filter_map(|(name, definition)| {
            let enchantment = identifier(&format!("minecraft:{name}"));
            if members.is_some_and(|members| !members.contains(&enchantment)) {
                return None;
            }
            let supported = item.path() == "book"
                || item.path() == "enchanted_book"
                || match &definition.supported_items {
                    serde_json::Value::String(name) => name.strip_prefix('#').map_or_else(
                        || name == item.as_str(),
                        |tag| {
                            data.item_tags
                                .get(tag)
                                .is_some_and(|members| members.contains(item))
                        },
                    ),
                    serde_json::Value::Array(values) => values
                        .iter()
                        .any(|value| value.as_str() == Some(item.as_str())),
                    _ => false,
                };
            supported.then_some((enchantment, definition.max_level))
        })
        .collect()
}
/// `/enchant` validity (wiki Commands/enchant): the enchantment must exist,
/// the level must be within `1..=max_level`, the target item must accept it
/// (same support tags as the anvil path), and it must not conflict with a
/// different held enchantment in the same exclusive set. Re-applying the
/// same enchantment at any level replaces it (no duplicate entries).
pub fn command_enchant_error(
    item: &Identifier,
    item_raw: u32,
    held_enchantments: &[ItemEnchantment],
    enchantment: &Identifier,
    level: i32,
    tags: &TagsData,
) -> Option<&'static str> {
    let Some(def) = catalogue().enchantments.get(enchantment.path()) else {
        return Some("Unknown enchantment");
    };
    if level < 1 {
        return Some("Enchantment level must be at least 1");
    }
    if level > def.max_level {
        return Some("Enchantment level exceeds the maximum level");
    }
    let supported = item.path() == "book"
        || item.path() == "enchanted_book"
        || matches(&def.supported_items, item, item_raw, tags, "minecraft:item");
    if !supported {
        return Some("That enchantment cannot be applied to the held item");
    }
    for existing in held_enchantments
        .iter()
        .filter(|existing| existing.id != *enchantment)
    {
        let raw =
            crate::required_registry_entry_id("enchantment", &existing.id).unwrap_or(u32::MAX);
        if matches(
            &def.exclusive_set,
            &existing.id,
            raw,
            tags,
            "minecraft:enchantment",
        ) {
            return Some("That enchantment conflicts with an enchantment on the held item");
        }
    }
    None
}
/// Apply a compatible random loot enchantment without dropping existing stack
/// components. `levels` selects the highest affordable level for fishing loot;
/// zero asks for a uniformly random legal level (`enchant_randomly`).
pub fn random_loot_enchantments(
    items: &ItemRegistry,
    stack: &mut ItemStack,
    levels: i32,
    seed: u64,
) -> bool {
    let Some(item) = items.name_of(stack.item_id) else {
        return false;
    };
    let candidates =
        random_loot_enchantment_candidates(item, Some(&identifier("minecraft:on_random_loot")));
    if candidates.is_empty() {
        return false;
    }
    let (enchantment, maximum) = &candidates[(seed % candidates.len() as u64) as usize];
    let level = if levels <= 0 {
        1 + ((seed.rotate_left(17) % *maximum as u64) as i32)
    } else {
        let [base, step] = catalogue()
            .enchantment_min_cost
            .get(enchantment.path())
            .copied()
            .unwrap_or([1, 0]);
        (if step > 0 {
            1 + (levels - base).max(0) / step
        } else {
            1
        })
        .min(*maximum)
    };
    if item.path() == "book" || item.path() == "enchanted_book" {
        let Some(enchanted_book) = items.id_of(&identifier("minecraft:enchanted_book")) else {
            return false;
        };
        stack.item_id = enchanted_book;
        stack.extended_mut().stored_enchantments = vec![ItemEnchantment {
            id: enchantment.clone(),
            level,
        }];
    } else {
        *stack = stack.clone().with_enchantment(enchantment.clone(), level);
    }
    true
}

#[cfg(test)]
mod tests;
