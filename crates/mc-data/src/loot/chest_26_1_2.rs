//! Compiled `minecraft:chest` and `minecraft:advancement_reward` loot tables.
//! Supported tables preserve random enchantments as canonical item components.
//! Unsupported functions and contexts fail explicitly rather than losing data.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::items::ItemRegistry;
use crate::{Identifier, ItemEnchantment, ItemStack};

/// Rejects chest-table sources larger than this before JSON parsing.
pub const MAX_CHEST_TABLE_BYTES: usize = 65_536;
/// Bounds pools per `minecraft:chest` table.
pub const MAX_CHEST_POOLS: usize = 16;
/// Bounds entries per pool.
pub const MAX_CHEST_ENTRIES: usize = 64;
/// Bounds constant/uniform rolls and counts (vanilla chest values are small).
pub const MAX_CHEST_ROLLS: u32 = 64;
/// Bounds stack counts produced by `minecraft:set_count`.
pub const MAX_CHEST_COUNT: u32 = 64;
/// Bounds a single entry weight; the per-pool total is `u64`-summed.
pub const MAX_CHEST_WEIGHT: u64 = 1_000_000;
/// Slot count of a vanilla chest block entity; rolls scatter into these.
pub const CHEST_SLOT_COUNT: u64 = 27;
#[derive(Debug, Error)]
pub enum ChestLootError {
    #[error("chest loot table at {path} is not valid UTF-8")]
    NonUtf8 { path: PathBuf },
    #[error("chest loot table {id} is missing at {path}")]
    MissingTable { id: Identifier, path: PathBuf },
    #[error("filesystem error reading chest loot table {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("chest loot table at {path} exceeds {max} bytes")]
    TooLarge { path: PathBuf, max: usize },
    #[error("chest loot table at {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("chest loot table {id} is unsupported: {reason}")]
    Unsupported { id: Identifier, reason: String },
}

/// Deterministic SplitMix64 stream for chest rolls and slot scatter.
///
/// The stream matches the `LootRandom` generator in `loot::context` so chest
/// loot shares the repo's single deterministic RNG shape; one stream drives
/// both the drop roll and the slot scatter from a single per-chest seed.
#[derive(Debug, Clone)]
pub struct ChestRng {
    state: u64,
}

impl ChestRng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[must_use]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    /// Uniform value in `0..bound`. Panics on a zero bound; callers only pass
    /// validated non-zero totals and the chest slot count.
    #[must_use]
    pub fn next_bounded(&mut self, bound: u64) -> u64 {
        assert!(bound > 0, "chest RNG bound must be non-zero");
        self.next_u64() % bound
    }

    #[must_use]
    pub fn next_range_inclusive(&mut self, min: u32, max: u32) -> u32 {
        debug_assert!(min <= max, "chest RNG range must be ordered");
        min + (self.next_bounded(u64::from(max - min) + 1) as u32)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChestLootEntry {
    Empty {
        weight: u64,
        quality: i32,
    },
    Item {
        weight: u64,
        quality: i32,
        item: Identifier,
        count_min: u32,
        count_max: u32,
        enchantments: Vec<(Identifier, i32)>,
    },
}

impl ChestLootEntry {
    #[must_use]
    pub fn weight(&self) -> u64 {
        match self {
            Self::Empty { weight, .. } | Self::Item { weight, .. } => *weight,
        }
    }

    fn effective_weight(&self, luck: f32) -> u64 {
        let quality = match self {
            Self::Empty { quality, .. } | Self::Item { quality, .. } => *quality,
        };
        (self.weight() as f32 + quality as f32 * luck)
            .floor()
            .max(0.0) as u64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChestLootPool {
    pub rolls_min: u32,
    pub rolls_max: u32,
    pub entries: Vec<ChestLootEntry>,
    pub total_weight: u64,
    pub bonus_rolls: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChestLootTable {
    id: Identifier,
    pools: Vec<ChestLootPool>,
}

impl ChestLootTable {
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    #[must_use]
    pub fn pools(&self) -> &[ChestLootPool] {
        &self.pools
    }

    /// Compile a vanilla `minecraft:chest` table source. Fails closed on any
    /// construct outside the surface documented at the top of this module.
    ///
    /// # Errors
    ///
    /// Returns [`ChestLootError::Unsupported`] when the source is not a
    /// supported chest table.
    pub fn compile(id: Identifier, raw: &str) -> Result<Self, ChestLootError> {
        Self::compile_with_type(id, raw, "minecraft:chest")
    }

    /// Compile reward loot using the same bounded pools and stack evaluator as
    /// chest loot, without accepting reward contexts through [`Self::compile`].
    pub fn compile_advancement_reward(id: Identifier, raw: &str) -> Result<Self, ChestLootError> {
        Self::compile_with_type(id, raw, "minecraft:advancement_reward")
    }

    fn compile_with_type(
        id: Identifier,
        raw: &str,
        expected_type: &'static str,
    ) -> Result<Self, ChestLootError> {
        let unsupported = |reason: String| ChestLootError::Unsupported {
            id: id.clone(),
            reason,
        };
        if raw.len() > MAX_CHEST_TABLE_BYTES {
            return Err(ChestLootError::TooLarge {
                path: PathBuf::from(id.as_str()),
                max: MAX_CHEST_TABLE_BYTES,
            });
        }
        let value: Value =
            serde_json::from_str(raw).map_err(|source| ChestLootError::Malformed {
                path: PathBuf::from(id.as_str()),
                source,
            })?;
        let root = value
            .as_object()
            .ok_or_else(|| unsupported("chest table root must be a JSON object".to_string()))?;
        let table_type = root
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| unsupported("chest table is missing $.type".to_string()))?;
        if table_type != expected_type {
            return Err(unsupported(format!(
                "expected $.type {expected_type:?}, found {table_type:?}"
            )));
        }
        let pools = root
            .get("pools")
            .and_then(Value::as_array)
            .ok_or_else(|| unsupported("chest table is missing $.pools".to_string()))?;
        if pools.is_empty() || pools.len() > MAX_CHEST_POOLS {
            return Err(unsupported(format!(
                "chest table must declare 1..={} pools, found {}",
                MAX_CHEST_POOLS,
                pools.len()
            )));
        }
        let mut compiled = Vec::with_capacity(pools.len());
        for (pool_index, pool) in pools.iter().enumerate() {
            compiled.push(compile_pool(&id, pool_index, pool)?);
        }
        Ok(Self {
            id,
            pools: compiled,
        })
    }

    /// Roll with the opening player's luck (zero for structure generation).
    /// The same seed and context produce identical canonical stacks.
    pub fn roll(
        &self,
        rng: &mut ChestRng,
        items: &ItemRegistry,
        luck: f32,
    ) -> Result<Vec<ItemStack>, ChestLootError> {
        if !luck.is_finite() {
            return Err(ChestLootError::Unsupported {
                id: self.id.clone(),
                reason: "non-finite chest luck".into(),
            });
        }
        // Reserve once for the bounded maximum output; luck never allocates
        // candidates or grows the output buffer inside a roll.
        let capacity = self
            .pools
            .iter()
            .map(|pool| {
                (f64::from(pool.rolls_max) + f64::from((pool.bonus_rolls * luck).floor()))
                    .clamp(0.0, f64::from(MAX_CHEST_ROLLS)) as usize
            })
            .sum();
        let mut out = Vec::with_capacity(capacity);
        for pool in &self.pools {
            let bonus = f64::from((pool.bonus_rolls * luck).floor());
            let rolls = (f64::from(rng.next_range_inclusive(pool.rolls_min, pool.rolls_max))
                + bonus)
                .max(0.0);
            if rolls > f64::from(MAX_CHEST_ROLLS) {
                return Err(ChestLootError::Unsupported {
                    id: self.id.clone(),
                    reason: "chest luck exceeds roll budget".into(),
                });
            }
            let mut weights = [0_u64; MAX_CHEST_ENTRIES];
            let mut total_weight = 0_u64;
            for (entry, weight) in pool.entries.iter().zip(&mut weights) {
                *weight = entry.effective_weight(luck);
                total_weight = total_weight.checked_add(*weight).ok_or_else(|| {
                    ChestLootError::Unsupported {
                        id: self.id.clone(),
                        reason: "chest luck exceeds weight budget".into(),
                    }
                })?;
            }
            if total_weight == 0 {
                continue;
            }
            for _ in 0..rolls as u32 {
                let mut pick = rng.next_bounded(total_weight);
                let mut chosen: Option<&ChestLootEntry> = None;
                for (entry, &weight) in pool.entries.iter().zip(&weights) {
                    if pick < weight {
                        chosen = Some(entry);
                        break;
                    }
                    pick -= weight;
                }
                let Some(entry) = chosen else {
                    continue;
                };
                if let ChestLootEntry::Item {
                    item,
                    count_min,
                    count_max,
                    enchantments,
                    ..
                } = entry
                {
                    let count = rng.next_range_inclusive(*count_min, *count_max);
                    if count > 0 {
                        let item_id =
                            items
                                .id_of(item)
                                .ok_or_else(|| ChestLootError::Unsupported {
                                    id: self.id.clone(),
                                    reason: format!("unregistered loot item {item}"),
                                })?;
                        let mut stack = ItemStack::new(item_id, count as i32);
                        if !enchantments.is_empty() {
                            let (enchantment, max_level) =
                                &enchantments[rng.next_bounded(enchantments.len() as u64) as usize];
                            let level = rng.next_range_inclusive(1, *max_level as u32) as i32;
                            if item.path() == "book" {
                                stack.item_id = items
                                    .id_of(
                                        &Identifier::parse("minecraft:enchanted_book")
                                            .expect("vanilla identifier"),
                                    )
                                    .ok_or_else(|| ChestLootError::Unsupported {
                                        id: self.id.clone(),
                                        reason: "unregistered enchanted book".into(),
                                    })?;
                                stack
                                    .extended_mut()
                                    .stored_enchantments
                                    .push(ItemEnchantment {
                                        id: enchantment.clone(),
                                        level,
                                    });
                            } else {
                                stack.enchantments.push(ItemEnchantment {
                                    id: enchantment.clone(),
                                    level,
                                });
                            }
                        }
                        out.push(stack);
                    }
                }
            }
        }
        Ok(out)
    }
}

fn compile_pool(
    id: &Identifier,
    pool_index: usize,
    pool: &Value,
) -> Result<ChestLootPool, ChestLootError> {
    let unsupported = |reason: String| ChestLootError::Unsupported {
        id: id.clone(),
        reason,
    };
    let root = pool.as_object().ok_or_else(|| {
        unsupported(format!(
            "chest table pool {pool_index} must be a JSON object"
        ))
    })?;
    if root.contains_key("conditions") {
        return Err(unsupported(format!(
            "chest table pool {pool_index} has unsupported conditions"
        )));
    }
    if root.contains_key("functions") {
        return Err(unsupported(format!(
            "chest table pool {pool_index} has unsupported functions"
        )));
    }
    let rolls = root
        .get("rolls")
        .ok_or_else(|| unsupported(format!("chest table pool {pool_index} is missing rolls")))?;
    let (rolls_min, rolls_max) = compile_range(rolls, MAX_CHEST_ROLLS).map_err(|detail| {
        unsupported(format!(
            "chest table pool {pool_index} has invalid rolls: {detail}"
        ))
    })?;
    let bonus_rolls = root
        .get("bonus_rolls")
        .map_or(Some(0.0), Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0 && *value <= f64::from(MAX_CHEST_ROLLS))
        .ok_or_else(|| {
            unsupported(format!(
                "chest table pool {pool_index} has unsupported bonus_rolls"
            ))
        })?;
    let entries = root
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| unsupported(format!("chest table pool {pool_index} is missing entries")))?;
    if entries.is_empty() || entries.len() > MAX_CHEST_ENTRIES {
        return Err(unsupported(format!(
            "chest table pool {pool_index} must declare 1..={} entries, found {}",
            MAX_CHEST_ENTRIES,
            entries.len()
        )));
    }
    let mut compiled = Vec::with_capacity(entries.len());
    let mut total_weight = 0_u64;
    for (entry_index, entry) in entries.iter().enumerate() {
        let entry = compile_entry(id, pool_index, entry_index, entry)?;
        total_weight = total_weight.checked_add(entry.weight()).ok_or_else(|| {
            unsupported(format!("chest table pool {pool_index} overflows weights"))
        })?;
        if total_weight > MAX_CHEST_ENTRIES as u64 * MAX_CHEST_WEIGHT {
            return Err(unsupported(format!(
                "chest table pool {pool_index} overflows weights"
            )));
        }
        compiled.push(entry);
    }
    if total_weight == 0 {
        return Err(unsupported(format!(
            "chest table pool {pool_index} has zero total weight"
        )));
    }
    Ok(ChestLootPool {
        rolls_min,
        rolls_max,
        entries: compiled,
        total_weight,
        bonus_rolls: bonus_rolls as f32,
    })
}

fn compile_entry(
    id: &Identifier,
    pool_index: usize,
    entry_index: usize,
    entry: &Value,
) -> Result<ChestLootEntry, ChestLootError> {
    let unsupported = |reason: String| ChestLootError::Unsupported {
        id: id.clone(),
        reason,
    };
    let root = entry.as_object().ok_or_else(|| {
        unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} must be a JSON object"
        ))
    })?;
    if root.contains_key("conditions") {
        return Err(unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} has unsupported conditions"
        )));
    }
    let entry_type = root.get("type").and_then(Value::as_str).ok_or_else(|| {
        unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} is missing a type"
        ))
    })?;
    let weight = match root.get("weight") {
        None => 1_u64,
        Some(value) => integer_value(value, 0, MAX_CHEST_WEIGHT).map_err(|detail| {
            unsupported(format!(
                "chest table pool {pool_index} entry {entry_index} has invalid weight: {detail}"
            ))
        })?,
    };
    let quality = root
        .get("quality")
        .map_or(Some(0), Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| {
            unsupported(format!(
                "chest table pool {pool_index} entry {entry_index} has invalid quality"
            ))
        })?;
    match entry_type {
        "minecraft:empty" => Ok(ChestLootEntry::Empty { weight, quality }),
        "minecraft:item" => {
            let name = root.get("name").and_then(Value::as_str).ok_or_else(|| {
                unsupported(format!(
                    "chest table pool {pool_index} entry {entry_index} is missing a name"
                ))
            })?;
            let item = Identifier::parse(name.to_string()).map_err(|_| {
                unsupported(format!(
                    "chest table pool {pool_index} entry {entry_index} has invalid name {name:?}"
                ))
            })?;
            let ChestEntryFunctions {
                count_min,
                count_max,
                enchantments,
            } = compile_functions(root.get("functions"), pool_index, entry_index, id, &item)?;
            Ok(ChestLootEntry::Item {
                weight,
                quality,
                item,
                count_min,
                count_max,
                enchantments,
            })
        }
        other => Err(unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} has unsupported type {other:?}"
        ))),
    }
}

struct ChestEntryFunctions {
    count_min: u32,
    count_max: u32,
    enchantments: Vec<(Identifier, i32)>,
}

/// Compile count and random-enchantment functions without discarding components.
fn compile_functions(
    functions: Option<&Value>,
    pool_index: usize,
    entry_index: usize,
    id: &Identifier,
    item: &Identifier,
) -> Result<ChestEntryFunctions, ChestLootError> {
    let unsupported = |reason: String| ChestLootError::Unsupported {
        id: id.clone(),
        reason,
    };
    let Some(functions) = functions else {
        return Ok(ChestEntryFunctions {
            count_min: 1,
            count_max: 1,
            enchantments: Vec::new(),
        });
    };
    let functions = functions.as_array().ok_or_else(|| {
        unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} has malformed functions"
        ))
    })?;
    let mut count = (1_u32, 1_u32);
    let mut enchantments = Vec::new();
    for function in functions {
        let root = function.as_object().ok_or_else(|| {
            unsupported(format!(
                "chest table pool {pool_index} entry {entry_index} has malformed functions"
            ))
        })?;
        if root.contains_key("conditions") {
            return Err(unsupported(format!(
                "chest table pool {pool_index} entry {entry_index} has unsupported function conditions"
            )));
        }
        let name = root
            .get("function")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                unsupported(format!(
                "chest table pool {pool_index} entry {entry_index} has a function without a name"
            ))
            })?;
        match name {
            "minecraft:set_count" => {
                let add = root.get("add").and_then(Value::as_bool).unwrap_or(false);
                if add {
                    return Err(unsupported(format!(
                        "chest table pool {pool_index} entry {entry_index} uses additive set_count"
                    )));
                }
                let value = root.get("count").ok_or_else(|| {
                    unsupported(format!(
                        "chest table pool {pool_index} entry {entry_index} set_count is missing a count"
                    ))
                })?;
                count = compile_range(value, MAX_CHEST_COUNT).map_err(|detail| {
                    unsupported(format!(
                        "chest table pool {pool_index} entry {entry_index} has invalid set_count: {detail}"
                    ))
                })?;
            }
            "minecraft:enchant_randomly" => {
                let options = root
                    .get("options")
                    .map(|options| {
                        let name = options
                            .as_str()
                            .and_then(|name| name.strip_prefix('#'))
                            .ok_or_else(|| {
                                unsupported(
                                    "random enchantment options must be a canonical tag".into(),
                                )
                            })?;
                        Identifier::parse(name)
                            .map_err(|_| unsupported(format!("invalid enchantment tag {name}")))
                    })
                    .transpose()?;
                enchantments =
                    crate::stations::random_loot_enchantment_candidates(item, options.as_ref());
                if enchantments.is_empty() {
                    return Err(unsupported(format!(
                        "no compatible random enchantments for {item}"
                    )));
                }
            }
            other => {
                return Err(unsupported(format!(
                    "chest table pool {pool_index} entry {entry_index} has unsupported function {other:?}"
                )));
            }
        }
    }
    Ok(ChestEntryFunctions {
        count_min: count.0,
        count_max: count.1,
        enchantments,
    })
}

/// Compile a constant (`3.0`) or `minecraft:uniform` `{min, max}` range into
/// an ordered inclusive `(min, max)` pair bounded by `max`.
fn compile_range(value: &Value, max: u32) -> Result<(u32, u32), String> {
    if let Some(number) = value.as_f64() {
        let single = integer_in_range(number, 0, max)
            .ok_or_else(|| format!("expected an integral constant in 0..={max}, found {number}"))?;
        return Ok((single, single));
    }
    let root = value
        .as_object()
        .ok_or_else(|| "expected a constant or uniform range".to_string())?;
    if root.get("type").and_then(Value::as_str) != Some("minecraft:uniform") {
        return Err(format!(
            "expected range type \"minecraft:uniform\", found {:?}",
            root.get("type")
        ));
    }
    let min = root
        .get("min")
        .and_then(Value::as_f64)
        .and_then(|number| integer_in_range(number, 0, max))
        .ok_or_else(|| format!("expected integral min in 0..={max}"))?;
    let high = root
        .get("max")
        .and_then(Value::as_f64)
        .and_then(|number| integer_in_range(number, 0, max))
        .ok_or_else(|| format!("expected integral max in 0..={max}"))?;
    if min > high {
        return Err(format!("unordered range {min}..={high}"));
    }
    Ok((min, high))
}

fn integer_value(value: &Value, min: u64, max: u64) -> Result<u64, String> {
    let number = value
        .as_f64()
        .ok_or_else(|| format!("expected a number, found {value}"))?;
    if number.is_finite() && number.fract() == 0.0 && number >= min as f64 && number <= max as f64 {
        Ok(number as u64)
    } else {
        Err(format!(
            "expected an integral value in {min}..={max}, found {number}"
        ))
    }
}

fn integer_in_range(number: f64, min: u32, max: u32) -> Option<u32> {
    if number.is_finite()
        && number.fract() == 0.0
        && number >= f64::from(min)
        && number <= f64::from(max)
    {
        Some(number as u32)
    } else {
        None
    }
}

/// Compiled chest tables keyed by loot-table id, loaded from vanilla data.
#[derive(Debug, Clone, Default)]
pub struct ChestLootCatalog {
    tables: BTreeMap<Identifier, ChestLootTable>,
}

/// Load the tables used by production Solaris structures, saved vanilla chests
/// and saved container vehicles from the startup-selected content root.
/// Unknown saved tables are rejected explicitly by the materialization boundary.
///
/// # Errors
///
/// Returns the exact missing, unreadable or invalid table file.
pub fn load_vanilla(root: &Path) -> Result<ChestLootCatalog, String> {
    let mut catalog = ChestLootCatalog::new();
    for (name, relative) in [
        (
            "minecraft:chests/simple_dungeon",
            "data/minecraft/loot_table/chests/simple_dungeon.json",
        ),
        (
            "minecraft:chests/village/village_toolsmith",
            "data/minecraft/loot_table/chests/village/village_toolsmith.json",
        ),
        (
            "minecraft:chests/abandoned_mineshaft",
            "data/minecraft/loot_table/chests/abandoned_mineshaft.json",
        ),
    ] {
        let source = crate::runtime_resources::read_json(root, relative)?;
        let id = Identifier::parse(name).map_err(|error| format!("chest loot {name}: {error}"))?;
        catalog.insert(ChestLootTable::compile(id, &source).map_err(|error| {
            format!(
                "compiling chest loot {}: {error}",
                root.join(relative).display()
            )
        })?);
    }
    Ok(catalog)
}

impl ChestLootCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    #[must_use]
    pub fn get(&self, id: &Identifier) -> Option<&ChestLootTable> {
        self.tables.get(id)
    }

    pub fn insert(&mut self, table: ChestLootTable) {
        self.tables.insert(table.id.clone(), table);
    }

    /// Load the named tables from a vanilla data root, where table
    /// `namespace:path` is read from
    /// `<data_root>/<namespace>/loot_table/<path>.json`. Missing files fail
    /// closed so a misconfigured data directory can never silently fall back
    /// to fixed chest contents.
    ///
    /// # Errors
    ///
    /// Returns [`ChestLootError`] when a table file is missing, too large,
    /// malformed, or unsupported.
    pub fn load_vanilla_tables(
        data_root: impl AsRef<Path>,
        ids: &[Identifier],
    ) -> Result<Self, ChestLootError> {
        let mut catalog = Self::new();
        for id in ids {
            catalog.insert(Self::load_vanilla_table(data_root.as_ref(), id)?);
        }
        Ok(catalog)
    }

    fn load_vanilla_table(
        data_root: &Path,
        id: &Identifier,
    ) -> Result<ChestLootTable, ChestLootError> {
        let relative = Path::new(id.namespace())
            .join("loot_table")
            .join(format!("{}.json", id.path()));
        let path = data_root.join(&relative);
        if !path.is_file() {
            return Err(ChestLootError::MissingTable {
                id: id.clone(),
                path,
            });
        }
        let raw = std::fs::read(&path).map_err(|source| ChestLootError::Io {
            path: path.clone(),
            source,
        })?;
        if raw.len() > MAX_CHEST_TABLE_BYTES {
            return Err(ChestLootError::TooLarge {
                path,
                max: MAX_CHEST_TABLE_BYTES,
            });
        }
        let text =
            String::from_utf8(raw).map_err(|_| ChestLootError::NonUtf8 { path: path.clone() })?;
        ChestLootTable::compile(id.clone(), &text).map_err(|error| match error {
            ChestLootError::Unsupported { reason, .. } => ChestLootError::Unsupported {
                id: id.clone(),
                reason,
            },
            other => other,
        })
    }
}

#[cfg(test)]
#[path = "chest_26_1_2_tests.rs"]
mod tests;
