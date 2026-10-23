use super::{ChestRng, Identifier, ItemRegistry, ItemStack, parse_id};
use crate::ItemEnchantment;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct Resources {
    root: Option<PathBuf>,
    definitions: BTreeMap<Identifier, Value>,
}
impl Resources {
    pub(super) fn load(root: &Path) -> Result<Self, String> {
        let mut result = Self {
            root: Some(root.to_owned()),
            definitions: BTreeMap::new(),
        };
        let directory = root.join("minecraft/enchantment");
        // Small authored test roots need not provide unused enchantment resources.
        if !directory.exists() {
            return Ok(result);
        }
        for entry in
            std::fs::read_dir(&directory).map_err(|e| format!("{}: {e}", directory.display()))?
        {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("non-UTF8 enchantment path")?;
            let id = Identifier::parse(format!("minecraft:{name}")).map_err(|e| e.to_string())?;
            let value = read_json(&path)?;
            result.definitions.insert(id, value);
        }
        Ok(result)
    }
    pub(super) fn options(&self, value: &Value, registry: &str) -> Result<Vec<Identifier>, String> {
        self.resolve(value, registry, 0)
    }
    fn resolve(
        &self,
        value: &Value,
        registry: &str,
        depth: usize,
    ) -> Result<Vec<Identifier>, String> {
        if depth > 16 {
            return Err(format!("{registry} tag cycle/depth exceeds 16"));
        }
        let mut out = Vec::new();
        if let Some(values) = value.as_array() {
            for value in values {
                out.extend(self.resolve(value, registry, depth + 1)?);
            }
        } else if let Some(tag) = value.as_str().and_then(|s| s.strip_prefix('#')) {
            let id = Identifier::parse(tag).map_err(|e| e.to_string())?;
            let root = self
                .root
                .as_ref()
                .ok_or_else(|| format!("tag {tag} needs vanilla resource root"))?;
            let path = root
                .join(id.namespace())
                .join("tags")
                .join(registry)
                .join(format!("{}.json", id.path()));
            let source = read_json(&path)?;
            out.extend(self.resolve(&source["values"], registry, depth + 1)?);
        } else {
            out.push(parse_id(value)?);
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
    pub(super) fn candidates(
        &self,
        item: &Identifier,
        options: Option<&Value>,
        levels: bool,
        compatible: bool,
    ) -> Result<Vec<Candidate>, String> {
        let default = Value::String("#minecraft:on_random_loot".into());
        let options = options.unwrap_or(&default);
        if self.definitions.is_empty() {
            if levels {
                return Err("enchant_with_levels requires vanilla enchantment definitions; load through ChestLootCatalog".into());
            }
            let tag = options
                .as_str()
                .and_then(|s| s.strip_prefix('#'))
                .map(Identifier::parse)
                .transpose()
                .map_err(|e| e.to_string())?;
            let mut values =
                crate::stations::random_loot_enchantment_candidates(item, tag.as_ref());
            if tag.is_none() {
                let names = self.options(options, "enchantment")?;
                values.retain(|(id, _)| names.contains(id));
                if !compatible {
                    values = crate::stations::random_loot_enchantment_candidates(
                        &Identifier::parse("minecraft:book").expect("static id"),
                        None,
                    );
                    values.retain(|(id, _)| names.contains(id));
                }
                if values.len() != names.len() && !compatible {
                    return Err("unknown enchant_randomly options".into());
                }
            }
            if values.is_empty() {
                return Err(format!("no compatible enchant_randomly options for {item}"));
            }
            return Ok(values
                .into_iter()
                .map(|(id, max_level)| Candidate {
                    id,
                    max_level,
                    weight: 1,
                    min: [0, 0],
                    max: [0, 0],
                    excludes: Vec::new(),
                })
                .collect());
        }
        let names = self.options(options, "enchantment")?;
        let mut out = Vec::new();
        for id in names {
            let def = self
                .definitions
                .get(&id)
                .ok_or_else(|| format!("unknown enchantment {id}"))?;
            let supported = if levels {
                def.get("primary_items").unwrap_or(&def["supported_items"])
            } else {
                &def["supported_items"]
            };
            if compatible
                && !matches!(item.path(), "book" | "enchanted_book")
                && !self.options(supported, "item")?.contains(item)
            {
                continue;
            }
            let max_level = def["max_level"]
                .as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .filter(|v| (1..=255).contains(v))
                .ok_or("invalid enchantment max_level")?;
            let weight = def["weight"]
                .as_u64()
                .filter(|w| (1..=1024).contains(w))
                .ok_or("invalid enchantment weight")?;
            let cost = |key: &str| -> Result<[i32; 2], String> {
                let base = def[key]["base"]
                    .as_i64()
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or_else(|| format!("invalid {key}.base"))?;
                let step = def[key]["per_level_above_first"]
                    .as_i64()
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or_else(|| format!("invalid {key}.per_level_above_first"))?;
                Ok([base, step])
            };
            let excludes = def
                .get("exclusive_set")
                .map(|v| self.options(v, "enchantment"))
                .transpose()?
                .unwrap_or_default();
            out.push(Candidate {
                id,
                max_level,
                weight,
                min: cost("min_cost")?,
                max: cost("max_cost")?,
                excludes,
            });
        }
        if out.len() > 64 {
            return Err("more than 64 enchantment candidates".into());
        }
        // Empty compatibility sets legitimately leave an item unenchanted.
        Ok(out)
    }
}
fn read_json(path: &Path) -> Result<Value, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&source).map_err(|e| format!("{}: {e}", path.display()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub(super) id: Identifier,
    pub(super) max_level: i32,
    weight: u64,
    min: [i32; 2],
    max: [i32; 2],
    excludes: Vec<Identifier>,
}

/// Material enchantability (wiki Enchanting mechanics); no enchantment identities
/// or level/cost tables are hardcoded here: those come from the selected data root.
pub(super) fn enchantability(item: &Identifier) -> u32 {
    let path = item.path();
    let armor = ["_helmet", "_chestplate", "_leggings", "_boots"]
        .iter()
        .any(|suffix| path.ends_with(suffix));
    if armor {
        if path.starts_with("leather_") || path.starts_with("netherite_") {
            15
        } else if path.starts_with("chainmail_") {
            12
        } else if path.starts_with("iron_") || path == "turtle_helmet" {
            9
        } else if path.starts_with("golden_") {
            25
        } else if path.starts_with("diamond_") {
            10
        } else if path.starts_with("copper_") {
            8
        } else {
            0
        }
    } else if path.starts_with("wooden_") || path.starts_with("netherite_") {
        15
    } else if path.starts_with("stone_") {
        5
    } else if path.starts_with("iron_") {
        14
    } else if path.starts_with("golden_") {
        22
    } else if path.starts_with("diamond_") {
        10
    } else if path.starts_with("copper_") {
        13
    } else if matches!(path, "mace" | "trident") {
        15
    } else if matches!(
        path,
        "book" | "enchanted_book" | "bow" | "crossbow" | "fishing_rod"
    ) {
        1
    } else {
        0
    }
}

pub(super) fn put(
    stack: &mut ItemStack,
    items: &ItemRegistry,
    id: &Identifier,
    level: i32,
    add: bool,
) -> Result<(), String> {
    let name = items
        .name_of(stack.item_id)
        .ok_or("unregistered enchanted item")?;
    let book = matches!(name.path(), "book" | "enchanted_book");
    if book {
        static BOOK: std::sync::LazyLock<Identifier> = std::sync::LazyLock::new(|| {
            Identifier::parse("minecraft:enchanted_book").expect("static id")
        });
        stack.item_id = items.id_of(&BOOK).ok_or("unregistered enchanted_book")?;
    }
    let enchantments = if book {
        &mut stack.extended_mut().stored_enchantments
    } else {
        &mut stack.enchantments
    };
    if let Some(existing) = enchantments.iter_mut().find(|e| e.id == *id) {
        existing.level = if add {
            existing.level.saturating_add(level).clamp(0, 255)
        } else {
            level
        };
    } else if level > 0 {
        enchantments.push(ItemEnchantment {
            id: id.clone(),
            level,
        });
    }
    enchantments.retain(|e| e.level > 0);
    enchantments.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(())
}

pub(super) fn random(
    candidates: &[Candidate],
    stack: &mut ItemStack,
    items: &ItemRegistry,
    rng: &mut ChestRng,
) -> Result<(), String> {
    if candidates.is_empty() {
        return Ok(());
    }
    let choice = &candidates[rng.next_bounded(candidates.len() as u64) as usize];
    put(
        stack,
        items,
        &choice.id,
        rng.next_range_inclusive(1, choice.max_level as u32) as i32,
        false,
    )
}
pub(super) fn with_levels(
    candidates: &[Candidate],
    enchantability: u32,
    mut level: i32,
    stack: &mut ItemStack,
    items: &ItemRegistry,
    rng: &mut ChestRng,
) -> Result<(), String> {
    if enchantability == 0 {
        return Ok(());
    }
    level += 1
        + rng.next_range_inclusive(0, enchantability / 4) as i32
        + rng.next_range_inclusive(0, enchantability / 4) as i32;
    level = ((f64::from(level) * (1.0 + (rng.fraction() + rng.fraction() - 1.0) * 0.15)).round()
        as i32)
        .max(1);
    let mut eligible = [0_i32; 64];
    for (candidate, slot) in candidates.iter().zip(&mut eligible) {
        *slot = (1..=candidate.max_level)
            .rev()
            .find(|l| {
                let cost = |c: [i32; 2]| c[0].saturating_add(c[1].saturating_mul(l - 1));
                cost(candidate.min) <= level && level <= cost(candidate.max)
            })
            .unwrap_or(0);
    }
    loop {
        let total: u64 = candidates
            .iter()
            .zip(eligible)
            .filter(|(_, l)| *l > 0)
            .map(|(c, _)| c.weight)
            .sum();
        if total == 0 {
            break;
        }
        let mut pick = rng.next_bounded(total);
        let mut chosen = 0;
        for (i, candidate) in candidates.iter().enumerate() {
            if eligible[i] == 0 {
                continue;
            }
            if pick < candidate.weight {
                chosen = i;
                break;
            }
            pick -= candidate.weight;
        }
        let selected = &candidates[chosen];
        put(stack, items, &selected.id, eligible[chosen], false)?;
        for (candidate, slot) in candidates.iter().zip(&mut eligible) {
            if candidate.id == selected.id
                || candidate.excludes.contains(&selected.id)
                || selected.excludes.contains(&candidate.id)
            {
                *slot = 0;
            }
        }
        if rng.next_bounded(50) > level as u64 {
            break;
        }
        level /= 2;
    }
    Ok(())
}
