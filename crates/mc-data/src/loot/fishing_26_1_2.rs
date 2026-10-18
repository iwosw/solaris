//! Vanilla 26.1.2 fishing tables, compiled once and rolled into canonical stacks.
//! Uses the shared loot RNG and random-loot enchanting; no hand-maintained drops.

use std::path::Path;

use serde::Deserialize;

use super::chest_26_1_2::ChestRng;
use crate::item_components::ItemFactsTable;
use crate::items::ItemRegistry;
use crate::{Identifier, ItemStack};

#[derive(Deserialize)]
struct Table {
    #[serde(rename = "type")]
    kind: String,
    pools: Vec<Pool>,
}

#[derive(Deserialize)]
struct Pool {
    rolls: f64,
    bonus_rolls: f64,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(flatten)]
    value: EntryValue,
    #[serde(default = "one")]
    weight: i32,
    #[serde(default)]
    quality: i32,
    #[serde(default)]
    conditions: Vec<Condition>,
    #[serde(default)]
    functions: Vec<Function>,
}

fn one() -> i32 {
    1
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum EntryValue {
    #[serde(rename = "minecraft:item")]
    Item { name: Identifier },
    #[serde(rename = "minecraft:loot_table")]
    Table { value: Identifier },
}

#[derive(Deserialize)]
#[serde(tag = "condition")]
enum Condition {
    #[serde(rename = "minecraft:entity_properties")]
    EntityProperties {
        entity: String,
        predicate: HookPredicate,
    },
    #[serde(rename = "minecraft:location_check")]
    LocationCheck { predicate: BiomePredicate },
}

#[derive(Deserialize)]
struct HookPredicate {
    type_specific: HookSpecific,
}

#[derive(Deserialize)]
struct HookSpecific {
    #[serde(rename = "type")]
    kind: String,
    in_open_water: bool,
}

#[derive(Deserialize)]
struct BiomePredicate {
    biomes: Vec<Identifier>,
}

#[derive(Deserialize)]
#[serde(tag = "function")]
enum Function {
    #[serde(rename = "minecraft:set_count")]
    Count { add: bool, count: f64 },
    #[serde(rename = "minecraft:set_potion")]
    Potion { id: Identifier },
    #[serde(rename = "minecraft:set_damage")]
    Damage { add: bool, damage: Uniform },
    #[serde(rename = "minecraft:enchant_with_levels")]
    Enchant { levels: f64, options: String },
}

#[derive(Deserialize)]
struct Uniform {
    #[serde(rename = "type")]
    kind: String,
    min: f64,
    max: f64,
}

const SOURCES: &[(&str, &str)] = &[
    (
        "gameplay/fishing",
        "data/minecraft/loot_table/gameplay/fishing.json",
    ),
    (
        "gameplay/fishing/fish",
        "data/minecraft/loot_table/gameplay/fishing/fish.json",
    ),
    (
        "gameplay/fishing/junk",
        "data/minecraft/loot_table/gameplay/fishing/junk.json",
    ),
    (
        "gameplay/fishing/treasure",
        "data/minecraft/loot_table/gameplay/fishing/treasure.json",
    ),
];

/// Fishing source closure owned by the server registry.
pub struct FishingLootTables {
    tables: Vec<Table>,
}

impl FishingLootTables {
    pub fn load(root: &Path) -> Result<Self, String> {
        let mut tables = Vec::with_capacity(SOURCES.len());
        for &(_, relative) in SOURCES {
            let source = crate::runtime_resources::read_json(root, relative)?;
            let table: Table = serde_json::from_str(&source).map_err(|error| {
                format!(
                    "parsing fishing loot {}: {error}",
                    root.join(relative).display()
                )
            })?;
            if table.kind != "minecraft:fishing"
                || table.pools.len() != 1
                || table.pools[0].rolls != 1.0
                || table.pools[0].bonus_rolls != 0.0
            {
                return Err(format!(
                    "unsupported fishing loot table {}",
                    root.join(relative).display()
                ));
            }
            tables.push(table);
        }
        Ok(Self { tables })
    }
}

impl Entry {
    fn weight(&self, luck: i32, open: bool, jungle: bool) -> u64 {
        let eligible = self.conditions.iter().all(|condition| match condition {
            Condition::EntityProperties { entity, predicate } => {
                assert_eq!(entity, "this");
                assert_eq!(predicate.type_specific.kind, "minecraft:fishing_hook");
                open == predicate.type_specific.in_open_water
            }
            Condition::LocationCheck { predicate } => {
                assert!(predicate.biomes.iter().all(|biome| matches!(
                    biome.path(),
                    "jungle" | "sparse_jungle" | "bamboo_jungle"
                )));
                jungle
            }
        });
        if eligible {
            (self.weight + self.quality * luck).max(0) as u64
        } else {
            0
        }
    }
}

impl FishingLootTables {
    /// Roll gameplay/fishing with treasure gated by the bobber's open-water region.
    /// Luck applies the root table's quality modifiers; jungle enables bamboo junk.
    #[must_use]
    pub fn roll(
        &self,
        items: &ItemRegistry,
        facts: &ItemFactsTable,
        luck: i32,
        open: bool,
        jungle: bool,
        seed: u64,
    ) -> Option<ItemStack> {
        let mut rng = ChestRng::new(seed);
        let mut table = &self.tables[0];
        loop {
            let entries = &table.pools[0].entries;
            let total: u64 = entries
                .iter()
                .map(|e| e.weight(luck.clamp(-32, 32), open, jungle))
                .sum();
            if total == 0 {
                return None;
            }
            let mut pick = rng.next_bounded(total);
            let entry = entries.iter().find(|entry| {
                let weight = entry.weight(luck.clamp(-32, 32), open, jungle);
                if pick < weight {
                    true
                } else {
                    pick -= weight;
                    false
                }
            })?;
            let name = match &entry.value {
                EntryValue::Table { value } => {
                    let index = SOURCES.iter().position(|(path, _)| {
                        value.namespace() == "minecraft" && value.path() == *path
                    })?;
                    // The vanilla root references only leaf tables, never itself.
                    assert_ne!(index, 0);
                    table = &self.tables[index];
                    continue;
                }
                EntryValue::Item { name } => name,
            };
            let mut stack = ItemStack::new(items.id_of(name)?, 1);
            for function in &entry.functions {
                match function {
                    Function::Count { add, count } => {
                        assert!(!add);
                        stack.count = *count as i32;
                    }
                    Function::Potion { id } => stack.extended_mut().potion = Some(id.clone()),
                    Function::Damage { add, damage } => {
                        assert!(!add);
                        assert_eq!(damage.kind, "minecraft:uniform");
                        let remaining = damage.min
                            + (damage.max - damage.min)
                                * ((rng.next_u64() >> 40) as f64 / (1_u64 << 24) as f64);
                        let max = facts.get(name)?.max_damage?;
                        // Vanilla set_damage uses the remaining durability fraction.
                        stack.damage = Some(((1.0 - remaining) * f64::from(max)).floor() as i32);
                    }
                    Function::Enchant { levels, options } => {
                        assert_eq!(options, "#minecraft:on_random_loot");
                        if !crate::stations::random_loot_enchantments(
                            items,
                            &mut stack,
                            *levels as i32,
                            rng.next_u64(),
                        ) {
                            return None;
                        }
                    }
                }
            }
            return Some(stack);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fishing_catalog_requires_every_source_from_its_selected_root() {
        let vanilla = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
        let loaded = FishingLootTables::load(&vanilla).unwrap();
        assert_eq!(loaded.tables.len(), SOURCES.len());
        let directory = tempfile::tempdir().unwrap();
        let error = FishingLootTables::load(directory.path())
            .err()
            .expect("another catalog cannot replace the missing source");
        assert!(error.contains(SOURCES[0].1), "{error}");
        let path = directory.path().join(SOURCES[0].1);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();
        let error = FishingLootTables::load(directory.path())
            .err()
            .expect("invalid source is rejected before serving");
        assert!(error.contains(&path.display().to_string()), "{error}");
    }
}
