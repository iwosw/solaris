//! Trial-chamber loot closure. Vanilla tables load once from the content cache; rolls retain item components.
use crate::loot::chest_26_1_2::ChestRng;
use crate::{
    Identifier, ItemEnchantment, ItemStack, item_components::ItemFactsTable, items::ItemRegistry,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

macro_rules! table {
    ($path:literal) => {
        (
            concat!("minecraft:", $path),
            concat!("data/minecraft/loot_table/", $path, ".json"),
        )
    };
}
const SOURCES: &[(&str, &str)] = &[
    table!("chests/trial_chambers/reward"),
    table!("chests/trial_chambers/reward_common"),
    table!("chests/trial_chambers/reward_rare"),
    table!("chests/trial_chambers/reward_unique"),
    table!("chests/trial_chambers/reward_ominous"),
    table!("chests/trial_chambers/reward_ominous_common"),
    table!("chests/trial_chambers/reward_ominous_rare"),
    table!("chests/trial_chambers/reward_ominous_unique"),
    table!("spawners/trial_chamber/key"),
    table!("spawners/trial_chamber/consumables"),
    table!("spawners/trial_chamber/items_to_drop_when_ominous"),
    table!("spawners/ominous/trial_chamber/key"),
    table!("spawners/ominous/trial_chamber/consumables"),
];

/// Immutable trial loot parsed once from the selected vanilla directory.
pub struct TrialLootTables {
    tables: BTreeMap<&'static str, Value>,
}

impl TrialLootTables {
    pub fn load(root: &Path) -> Result<Self, String> {
        let mut tables = BTreeMap::new();
        for &(name, relative) in SOURCES {
            let source = crate::runtime_resources::read_json(root, relative)?;
            let table = serde_json::from_str(&source).map_err(|error| {
                format!(
                    "parsing trial loot {}: {error}",
                    root.join(relative).display()
                )
            })?;
            tables.insert(name, table);
        }
        Ok(Self { tables })
    }
}

fn sample(value: &Value, rng: &mut ChestRng) -> f64 {
    if let Some(number) = value.as_f64() {
        return number;
    }
    let min = value["min"].as_f64().expect("uniform min");
    let max = value["max"].as_f64().expect("uniform max");
    if min.fract() == 0.0 && max.fract() == 0.0 {
        rng.next_range_inclusive(min as u32, max as u32) as f64
    } else {
        min + (max - min) * (rng.next_u64() as f64 / u64::MAX as f64)
    }
}
fn enchant(
    stack: &mut ItemStack,
    item: &Identifier,
    enchantment: ItemEnchantment,
    items: &ItemRegistry,
) -> Result<(), String> {
    if item.path() == "book" {
        stack.item_id = items
            .id_of(&Identifier::parse("minecraft:enchanted_book").expect("id"))
            .ok_or("missing enchanted_book")?;
        stack.extended_mut().stored_enchantments.push(enchantment);
    } else {
        stack.enchantments.push(enchantment);
    }
    Ok(())
}
fn functions(
    stack: &mut ItemStack,
    item: &Identifier,
    entry: &Value,
    rng: &mut ChestRng,
    items: &ItemRegistry,
    facts: &ItemFactsTable,
) -> Result<(), String> {
    let Some(functions) = entry.get("functions").and_then(Value::as_array) else {
        return Ok(());
    };
    for function in functions {
        match function["function"].as_str().expect("function name") {
            "minecraft:set_count" => stack.count = sample(&function["count"], rng) as i32,
            "minecraft:set_potion" => {
                stack.extended_mut().potion = Some(
                    Identifier::parse(function["id"].as_str().expect("potion id"))
                        .map_err(|e| e.to_string())?,
                )
            }
            "minecraft:set_ominous_bottle_amplifier" => {
                stack.extended_mut().ominous_bottle_amplifier =
                    Some(sample(&function["amplifier"], rng) as i32)
            }
            "minecraft:set_damage" => {
                let maximum = facts
                    .get(item)
                    .and_then(|facts| facts.max_damage)
                    .ok_or_else(|| format!("missing durability for {item}"))?;
                stack.damage = Some(
                    ((1.0 - sample(&function["damage"], rng)) * f64::from(maximum)).floor() as i32,
                );
            }
            "minecraft:set_enchantments" => {
                for (name, level) in function["enchantments"].as_object().expect("enchantments") {
                    enchant(
                        stack,
                        item,
                        ItemEnchantment {
                            id: Identifier::parse(name).map_err(|e| e.to_string())?,
                            level: sample(level, rng) as i32,
                        },
                        items,
                    )?;
                }
            }
            "minecraft:enchant_randomly" | "minecraft:enchant_with_levels" => {
                let candidates = if let Some(options) = function["options"].as_array() {
                    options
                        .iter()
                        .map(|name| {
                            let id = Identifier::parse(name.as_str().expect("enchantment id"))
                                .expect("id");
                            let max =
                                crate::stations::random_loot_enchantment_candidates(item, None)
                                    .into_iter()
                                    .find(|(candidate, _)| candidate == &id)
                                    .map_or(1, |(_, max)| max);
                            (id, max)
                        })
                        .collect::<Vec<_>>()
                } else {
                    let tag = function["options"]
                        .as_str()
                        .and_then(|name| name.strip_prefix('#'))
                        .map(|name| Identifier::parse(name).expect("tag"));
                    crate::stations::random_loot_enchantment_candidates(item, tag.as_ref())
                };
                if !candidates.is_empty() {
                    let (id, maximum) =
                        &candidates[rng.next_bounded(candidates.len() as u64) as usize];
                    // Gameplay uses the existing chest enchantment selection, not vanilla's enchanting algorithm.
                    let level = rng.next_range_inclusive(1, *maximum as u32) as i32;
                    enchant(
                        stack,
                        item,
                        ItemEnchantment {
                            id: id.clone(),
                            level,
                        },
                        items,
                    )?;
                }
            }
            name => return Err(format!("unsupported trial loot function {name}")),
        }
    }
    Ok(())
}
impl TrialLootTables {
    fn roll_into(
        &self,
        name: &str,
        rng: &mut ChestRng,
        items: &ItemRegistry,
        facts: &ItemFactsTable,
        output: &mut Vec<ItemStack>,
    ) -> Result<(), String> {
        let table = self
            .tables
            .get(name)
            .ok_or_else(|| format!("unknown trial loot table {name}"))?;
        for pool in table["pools"].as_array().expect("pools") {
            let mut eligible = true;
            if let Some(conditions) = pool.get("conditions").and_then(Value::as_array) {
                for condition in conditions {
                    if condition["condition"].as_str() != Some("minecraft:random_chance") {
                        return Err("unsupported trial loot condition".into());
                    }
                    eligible &= (rng.next_u64() as f64 / u64::MAX as f64)
                        < condition["chance"].as_f64().expect("chance");
                }
            }
            if !eligible {
                continue;
            }
            let entries = pool["entries"].as_array().expect("entries");
            let total: u64 = entries
                .iter()
                .map(|entry| entry["weight"].as_u64().unwrap_or(1))
                .sum();
            for _ in 0..sample(&pool["rolls"], rng) as u32 {
                let mut pick = rng.next_bounded(total);
                let entry = entries
                    .iter()
                    .find(|entry| {
                        let weight = entry["weight"].as_u64().unwrap_or(1);
                        if pick < weight {
                            true
                        } else {
                            pick -= weight;
                            false
                        }
                    })
                    .expect("weighted entry");
                if entry["type"].as_str() == Some("minecraft:loot_table") {
                    self.roll_into(
                        entry["value"].as_str().expect("table reference"),
                        rng,
                        items,
                        facts,
                        output,
                    )?;
                } else {
                    let item = Identifier::parse(entry["name"].as_str().expect("item name"))
                        .map_err(|e| e.to_string())?;
                    let id = items
                        .id_of(&item)
                        .ok_or_else(|| format!("unregistered trial loot item {item}"))?;
                    let mut stack = ItemStack::new(id, 1);
                    functions(&mut stack, &item, entry, rng, items, facts)?;
                    if !stack.is_empty() {
                        output.push(stack);
                    }
                }
            }
        }
        Ok(())
    }
    pub fn roll(
        &self,
        name: &str,
        seed: u64,
        items: &ItemRegistry,
        facts: &ItemFactsTable,
    ) -> Result<Vec<ItemStack>, String> {
        let mut result = Vec::new();
        self.roll_into(name, &mut ChestRng::new(seed), items, facts, &mut result)?;
        Ok(result)
    }
}
pub fn contains(name: &str) -> bool {
    SOURCES.iter().any(|(supported, _)| *supported == name)
}

#[cfg(test)]
#[path = "trial_loot_tests.rs"]
mod tests;
