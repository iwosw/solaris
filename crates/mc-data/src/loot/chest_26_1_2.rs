//! Startup-compiled vanilla container loot. No JSON parsing or filesystem reads
//! occur while rolling. Unsupported constructs report their table and JSON path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use thiserror::Error;

use crate::items::ItemRegistry;
use crate::{Identifier, ItemStack};

#[path = "chest_enchantments.rs"]
mod enchantments;
#[path = "chest_functions.rs"]
mod functions;
#[path = "chest_numbers.rs"]
mod numbers;

pub use functions::ExplorationMap;
use functions::{ChestFunction, Condition};
use numbers::Number;

pub const MAX_CHEST_TABLE_BYTES: usize = 65_536;
pub const MAX_CHEST_POOLS: usize = 16;
pub const MAX_CHEST_ENTRIES: usize = 64;
pub const MAX_CHEST_ROLLS: u32 = 64;
pub const MAX_CHEST_COUNT: u32 = 64;
pub const MAX_CHEST_WEIGHT: u64 = 1_000_000;
pub const CHEST_SLOT_COUNT: u64 = 27;
const MAX_REFERENCE_DEPTH: usize = 16;
const MAX_OUTPUT_STACKS: usize = 4096;

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

fn unsupported(id: &Identifier, reason: impl Into<String>) -> ChestLootError {
    ChestLootError::Unsupported {
        id: id.clone(),
        reason: reason.into(),
    }
}

/// One SplitMix64 stream drives all conditions, providers, functions and scatter.
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
    #[must_use]
    pub fn next_bounded(&mut self, bound: u64) -> u64 {
        assert!(bound > 0, "chest RNG bound must be non-zero");
        self.next_u64() % bound
    }
    #[must_use]
    pub fn next_range_inclusive(&mut self, min: u32, max: u32) -> u32 {
        debug_assert!(min <= max);
        min + self.next_bounded(u64::from(max - min) + 1) as u32
    }
    fn fraction(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1_u64 << 53) as f64
    }
}

/// The native saved-map owner captures origin, dimension and mutable map storage.
/// `None` means a completed lookup found no destination: omit that map drop.
pub type ExplorationMapResolver<'a> =
    dyn FnMut(&ExplorationMap) -> Result<Option<ItemStack>, String> + 'a;

#[derive(Default)]
pub struct ChestLootContext<'a> {
    pub luck: f32,
    pub exploration_map: Option<&'a mut ExplorationMapResolver<'a>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChestLootEntry {
    Empty {
        weight: u64,
        quality: i32,
        conditions: Vec<Condition>,
        functions: Vec<ChestFunction>,
    },
    Item {
        weight: u64,
        quality: i32,
        item: Identifier,
        conditions: Vec<Condition>,
        functions: Vec<ChestFunction>,
    },
    Table {
        weight: u64,
        quality: i32,
        table: Arc<ChestLootTable>,
        conditions: Vec<Condition>,
        functions: Vec<ChestFunction>,
    },
}
impl ChestLootEntry {
    #[must_use]
    pub fn weight(&self) -> u64 {
        match self {
            Self::Empty { weight, .. } | Self::Item { weight, .. } | Self::Table { weight, .. } => {
                *weight
            }
        }
    }
    fn effective_weight(&self, luck: f32) -> u64 {
        let quality = match self {
            Self::Empty { quality, .. }
            | Self::Item { quality, .. }
            | Self::Table { quality, .. } => *quality,
        };
        (self.weight() as f32 + quality as f32 * luck)
            .floor()
            .max(0.0) as u64
    }
    fn conditions(&self) -> &[Condition] {
        match self {
            Self::Empty { conditions, .. }
            | Self::Item { conditions, .. }
            | Self::Table { conditions, .. } => conditions,
        }
    }
    fn functions(&self) -> &[ChestFunction] {
        match self {
            Self::Empty { functions, .. }
            | Self::Item { functions, .. }
            | Self::Table { functions, .. } => functions,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChestLootPool {
    pub rolls_min: u32,
    pub rolls_max: u32,
    pub entries: Vec<ChestLootEntry>,
    pub total_weight: u64,
    rolls: Number,
    bonus_rolls: Number,
    conditions: Vec<Condition>,
    functions: Vec<ChestFunction>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ChestLootTable {
    id: Identifier,
    pools: Vec<ChestLootPool>,
    functions: Vec<ChestFunction>,
    exploration_maps: Vec<ExplorationMap>,
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
    /// Native openers must defer these tables until the saved-map owner resolves
    /// their structure destinations. Neither table nor seed should be consumed.
    #[must_use]
    pub fn requires_world_context(&self) -> bool {
        !self.exploration_maps.is_empty()
    }
    /// Deduplicated requests, including nested tables, for async first-open lookup.
    #[must_use]
    pub fn exploration_maps(&self) -> &[ExplorationMap] {
        &self.exploration_maps
    }

    /// Compile a standalone chest table. Resource-dependent level enchantments
    /// and table references require the startup catalog loader.
    pub fn compile(id: Identifier, raw: &str) -> Result<Self, ChestLootError> {
        Compiler::default().compile(id, raw, "minecraft:chest", 0)
    }
    pub fn compile_advancement_reward(id: Identifier, raw: &str) -> Result<Self, ChestLootError> {
        Compiler::default().compile(id, raw, "minecraft:advancement_reward", 0)
    }
    /// Same seed and context produce identical canonical stacks.
    pub fn roll(
        &self,
        rng: &mut ChestRng,
        items: &ItemRegistry,
        luck: f32,
    ) -> Result<Vec<ItemStack>, ChestLootError> {
        self.roll_with_context(
            rng,
            items,
            &mut ChestLootContext {
                luck,
                exploration_map: None,
            },
        )
    }
    pub fn roll_with_context(
        &self,
        rng: &mut ChestRng,
        items: &ItemRegistry,
        context: &mut ChestLootContext<'_>,
    ) -> Result<Vec<ItemStack>, ChestLootError> {
        if !context.luck.is_finite() {
            return Err(unsupported(&self.id, "non-finite chest luck"));
        }
        if self.requires_world_context() && context.exploration_map.is_none() {
            return Err(unsupported(
                &self.id,
                "minecraft:exploration_map requires a world-context saved-map resolver; retain unresolved table and seed",
            ));
        }
        let mut out =
            Vec::with_capacity(self.pools.iter().map(|pool| pool.rolls_max as usize).sum());
        self.roll_into(rng, items, context, &mut out)?;
        Ok(out)
    }
    fn roll_into(
        &self,
        rng: &mut ChestRng,
        items: &ItemRegistry,
        context: &mut ChestLootContext<'_>,
        out: &mut Vec<ItemStack>,
    ) -> Result<(), ChestLootError> {
        let table_start = out.len();
        for pool in &self.pools {
            if !functions::test_conditions(&pool.conditions, rng) {
                continue;
            }
            let rolls = (pool.rolls.int(rng) as f64
                + f64::from((pool.bonus_rolls.float(rng) as f32 * context.luck).floor()))
            .max(0.0);
            if rolls > f64::from(MAX_CHEST_ROLLS) {
                return Err(unsupported(&self.id, "chest luck exceeds roll budget"));
            }
            let pool_start = out.len();
            for _ in 0..rolls as u32 {
                let mut weights = [0_u64; MAX_CHEST_ENTRIES];
                let mut total = 0_u64;
                for (entry, weight) in pool.entries.iter().zip(&mut weights) {
                    if functions::test_conditions(entry.conditions(), rng) {
                        *weight = entry.effective_weight(context.luck);
                        total = total.checked_add(*weight).ok_or_else(|| {
                            unsupported(&self.id, "chest luck exceeds weight budget")
                        })?;
                    }
                }
                if total == 0 {
                    continue;
                }
                let mut pick = rng.next_bounded(total);
                for (entry, weight) in pool.entries.iter().zip(weights) {
                    if pick >= weight {
                        pick -= weight;
                        continue;
                    }
                    let start = out.len();
                    match entry {
                        ChestLootEntry::Empty { .. } => {}
                        ChestLootEntry::Item { item, .. } => {
                            if out.len() >= MAX_OUTPUT_STACKS {
                                return Err(unsupported(
                                    &self.id,
                                    "nested loot exceeds output budget",
                                ));
                            }
                            let item_id = items.id_of(item).ok_or_else(|| {
                                unsupported(&self.id, format!("unregistered loot item {item}"))
                            })?;
                            out.push(ItemStack::new(item_id, 1));
                        }
                        ChestLootEntry::Table { table, .. } => {
                            table.roll_into(rng, items, context, out)?
                        }
                    }
                    self.apply(entry.functions(), out, start, rng, items, context)?;
                    break;
                }
            }
            self.apply(&pool.functions, out, pool_start, rng, items, context)?;
        }
        self.apply(&self.functions, out, table_start, rng, items, context)?;
        // Remove empty outputs only after all enclosing functions have run.
        let mut index = table_start;
        while index < out.len() {
            if out[index].count <= 0 {
                out.remove(index);
            } else {
                index += 1;
            }
        }
        Ok(())
    }
    fn apply(
        &self,
        functions: &[ChestFunction],
        stacks: &mut Vec<ItemStack>,
        start: usize,
        rng: &mut ChestRng,
        items: &ItemRegistry,
        context: &mut ChestLootContext<'_>,
    ) -> Result<(), ChestLootError> {
        let mut index = start;
        while index < stacks.len() {
            let mut keep = true;
            for function in functions {
                if !function
                    .apply(&mut stacks[index], rng, items, context)
                    .map_err(|reason| unsupported(&self.id, reason))?
                {
                    keep = false;
                    break;
                }
            }
            if keep {
                index += 1;
            } else {
                stacks.remove(index);
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Compiler {
    sources: BTreeMap<Identifier, String>,
    compiled: BTreeMap<Identifier, Arc<ChestLootTable>>,
    resources: enchantments::Resources,
    data_root: Option<PathBuf>,
}
impl Compiler {
    fn compile(
        &mut self,
        id: Identifier,
        raw: &str,
        expected_type: &str,
        depth: usize,
    ) -> Result<ChestLootTable, ChestLootError> {
        let fail = |reason| unsupported(&id, reason);
        if depth > MAX_REFERENCE_DEPTH {
            return Err(fail(
                "loot_table reference cycle or depth exceeds 16".into(),
            ));
        }
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
            .ok_or_else(|| fail("root must be an object".into()))?;
        if root.get("type").and_then(Value::as_str) != Some(expected_type) {
            return Err(fail(format!(
                "expected $.type {expected_type}, found {:?}",
                root.get("type")
            )));
        }
        let functions = functions::compile(root.get("functions"), None, &self.resources)
            .map_err(|e| fail(format!("$.functions: {e}")))?;
        let pools = root
            .get("pools")
            .and_then(Value::as_array)
            .ok_or_else(|| fail("missing $.pools".into()))?;
        if pools.len() > MAX_CHEST_POOLS {
            return Err(fail("too many pools".into()));
        }
        let mut compiled = Vec::with_capacity(pools.len());
        for (p, pool) in pools.iter().enumerate() {
            compiled.push(
                self.pool(pool, depth)
                    .map_err(|reason| fail(format!("$.pools[{p}]: {reason}")))?,
            );
        }
        let mut exploration_maps = Vec::new();
        let mut add = |request: &ExplorationMap| {
            if !exploration_maps.contains(request) {
                exploration_maps.push(request.clone());
            }
        };
        for function in functions
            .iter()
            .chain(compiled.iter().flat_map(|pool| &pool.functions))
        {
            if let Some(request) = function.exploration_map() {
                add(request);
            }
        }
        for entry in compiled.iter().flat_map(|pool| &pool.entries) {
            for function in entry.functions() {
                if let Some(request) = function.exploration_map() {
                    add(request);
                }
            }
            if let ChestLootEntry::Table { table, .. } = entry {
                for request in &table.exploration_maps {
                    add(request);
                }
            }
        }
        Ok(ChestLootTable {
            id,
            pools: compiled,
            functions,
            exploration_maps,
        })
    }
    fn pool(&mut self, pool: &Value, depth: usize) -> Result<ChestLootPool, String> {
        let rolls = Number::compile(&pool["rolls"], 0.0, f64::from(MAX_CHEST_ROLLS))?;
        let bonus_rolls = Number::compile(
            pool.get("bonus_rolls").unwrap_or(&Value::from(0)),
            0.0,
            f64::from(MAX_CHEST_ROLLS),
        )?;
        let conditions = functions::conditions(pool.get("conditions"))?;
        let functions = functions::compile(pool.get("functions"), None, &self.resources)?;
        let entries = pool["entries"]
            .as_array()
            .ok_or("entries must be an array")?;
        if entries.is_empty() || entries.len() > MAX_CHEST_ENTRIES {
            return Err("entries must contain 1..=64 elements".into());
        }
        let mut compiled = Vec::with_capacity(entries.len());
        let mut total_weight = 0;
        for (e, entry) in entries.iter().enumerate() {
            let entry = self
                .entry(entry, depth)
                .map_err(|reason| format!("entries[{e}]: {reason}"))?;
            total_weight += entry.weight();
            compiled.push(entry);
        }
        if total_weight == 0 {
            return Err("zero total weight".into());
        }
        Ok(ChestLootPool {
            rolls_min: rolls.min() as u32,
            rolls_max: rolls.max() as u32,
            rolls,
            bonus_rolls,
            entries: compiled,
            total_weight,
            conditions,
            functions,
        })
    }
    fn entry(&mut self, entry: &Value, depth: usize) -> Result<ChestLootEntry, String> {
        let weight = entry
            .get("weight")
            .map_or(Some(1), Value::as_u64)
            .filter(|w| *w <= MAX_CHEST_WEIGHT)
            .ok_or("invalid weight")?;
        let quality = entry
            .get("quality")
            .map_or(Some(0), Value::as_i64)
            .and_then(|v| i32::try_from(v).ok())
            .ok_or("invalid quality")?;
        let conditions = functions::conditions(entry.get("conditions"))?;
        let item = if entry["type"] == "minecraft:item" {
            Some(parse_id(&entry["name"])?)
        } else {
            None
        };
        let functions = functions::compile(entry.get("functions"), item.as_ref(), &self.resources)?;
        match entry["type"].as_str() {
            Some("minecraft:empty") => Ok(ChestLootEntry::Empty {
                weight,
                quality,
                conditions,
                functions,
            }),
            Some("minecraft:item") => Ok(ChestLootEntry::Item {
                weight,
                quality,
                item: item.expect("parsed item"),
                conditions,
                functions,
            }),
            Some("minecraft:loot_table") => {
                let id = parse_id(&entry["value"])?;
                let table = if let Some(table) = self.compiled.get(&id) {
                    Arc::clone(table)
                } else {
                    let source = if let Some(source) = self.sources.get(&id) {
                        source.clone()
                    } else {
                        let root = self
                            .data_root
                            .as_ref()
                            .ok_or_else(|| format!("loot_table unresolved reference {id}"))?;
                        read_table_source(root, &id).map_err(|e| e.to_string())?
                    };
                    let table = Arc::new(
                        self.compile(id.clone(), &source, "minecraft:chest", depth + 1)
                            .map_err(|e| e.to_string())?,
                    );
                    self.compiled.insert(id, Arc::clone(&table));
                    table
                };
                Ok(ChestLootEntry::Table {
                    weight,
                    quality,
                    table,
                    conditions,
                    functions,
                })
            }
            other => Err(format!("unsupported entry type {other:?}")),
        }
    }
}
fn parse_id(value: &Value) -> Result<Identifier, String> {
    let name = value
        .as_str()
        .ok_or_else(|| format!("expected identifier, found {value}"))?;
    Identifier::parse(name).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Default)]
pub struct ChestLootCatalog {
    tables: BTreeMap<Identifier, ChestLootTable>,
}

/// Recursively load every chest table, plus archaeology and village hero gifts.
/// Missing references, cycles and unknown functions fail during startup.
pub fn load_vanilla(root: &Path) -> Result<ChestLootCatalog, String> {
    let data = root.join("data");
    let mut ids = Vec::new();
    for category in ["chests", "archaeology", "gameplay/hero_of_the_village"] {
        let directory = data.join("minecraft/loot_table").join(category);
        if category != "chests" && !directory.exists() {
            continue;
        }
        collect_tables(&directory, &data.join("minecraft/loot_table"), &mut ids)?;
    }
    if ids.is_empty() {
        return Err(format!("no chest loot tables in {}", root.display()));
    }
    ChestLootCatalog::load_vanilla_tables(data, &ids).map_err(|e| e.to_string())
}
fn collect_tables(directory: &Path, base: &Path, out: &mut Vec<Identifier>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|e| format!("loot directory {}: {e}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            collect_tables(&path, base, out)?;
        } else if path.extension().is_some_and(|e| e == "json") {
            let relative = path
                .strip_prefix(base)
                .map_err(|e| e.to_string())?
                .with_extension("");
            let name = relative.to_str().ok_or("non-UTF8 loot path")?;
            out.push(Identifier::parse(format!("minecraft:{name}")).map_err(|e| e.to_string())?);
        }
    }
    out.sort();
    Ok(())
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
    pub fn load_vanilla_tables(
        data_root: impl AsRef<Path>,
        ids: &[Identifier],
    ) -> Result<Self, ChestLootError> {
        let data_root = data_root.as_ref();
        let mut compiler = Compiler {
            data_root: Some(data_root.to_owned()),
            ..Compiler::default()
        };
        for id in ids {
            compiler
                .sources
                .insert(id.clone(), read_table_source(data_root, id)?);
        }
        if !ids.is_empty() {
            compiler.resources =
                enchantments::Resources::load(data_root).map_err(|e| unsupported(&ids[0], e))?;
        }
        let mut catalog = Self::new();
        for id in ids {
            let source = compiler.sources[id].clone();
            let expected = if id.path().starts_with("archaeology/") {
                "minecraft:archaeology"
            } else if id.path().starts_with("gameplay/hero_of_the_village/") {
                "minecraft:gift"
            } else {
                "minecraft:chest"
            };
            catalog.insert(compiler.compile(id.clone(), &source, expected, 0)?);
        }
        Ok(catalog)
    }
}

fn read_table_source(data_root: &Path, id: &Identifier) -> Result<String, ChestLootError> {
    let path = data_root
        .join(id.namespace())
        .join("loot_table")
        .join(format!("{}.json", id.path()));
    if !path.is_file() {
        return Err(ChestLootError::MissingTable {
            id: id.clone(),
            path,
        });
    }
    let bytes = std::fs::read(&path).map_err(|source| ChestLootError::Io {
        path: path.clone(),
        source,
    })?;
    if bytes.len() > MAX_CHEST_TABLE_BYTES {
        return Err(ChestLootError::TooLarge {
            path,
            max: MAX_CHEST_TABLE_BYTES,
        });
    }
    String::from_utf8(bytes).map_err(|_| ChestLootError::NonUtf8 { path })
}

#[cfg(test)]
#[path = "chest_26_1_2_tests.rs"]
mod tests;
