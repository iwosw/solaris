//! Immutable vanilla 26.1.2 enchanting rules. Load once before sharing the catalog;
//! offer generation and application never open files or mutate persistent state.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mc_data::{Identifier, ItemEnchantment, ItemStack};
use serde::Deserialize;
use serde_json::Value;

use super::ItemRules;

/// Vanilla's linear enchantment cost, evaluated with Java integer arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct EnchantmentCost {
    pub base: i32,
    pub per_level_above_first: i32,
}

impl EnchantmentCost {
    #[must_use]
    pub fn at(self, level: i32) -> i32 {
        self.base.wrapping_add(
            self.per_level_above_first
                .wrapping_mul(level.wrapping_sub(1)),
        )
    }
}

#[derive(Debug, Clone)]
struct ItemSet {
    tag: Option<Identifier>,
    members: BTreeSet<Identifier>,
}

impl ItemSet {
    fn contains(&self, rules: &ItemRules<'_>, stack: &ItemStack) -> bool {
        let Some(item) = super::anvil::logical_item(rules, stack) else {
            return false;
        };
        if rules.items.name_of(stack.item_id) != Some(item) {
            return self.members.contains(item);
        }
        if let Some(tag) = &self.tag
            && let Some(members) = rules
                .tags
                .registries
                .get("minecraft:item")
                .and_then(|tags| tags.get(tag))
            && !members.is_empty()
        {
            return i32::try_from(stack.item_id).is_ok_and(|id| members.contains(&id));
        }
        self.members.contains(item)
    }
}

/// Resolved definition. Protocol IDs use the complete dynamic registry, not
/// the smaller enchanting-table pool. Supported and primary item sets differ
/// (for example axes support Sharpness on anvils, but not at the table).
#[derive(Debug, Clone)]
pub struct EnchantmentDefinition {
    pub id: Identifier,
    pub protocol_id: i32,
    pub max_level: i32,
    pub anvil_cost: i32,
    pub curse: bool,
    pub weight: i32,
    pub min_cost: EnchantmentCost,
    pub max_cost: EnchantmentCost,
    supported: ItemSet,
    primary: Option<ItemSet>,
    exclusive: BTreeSet<Identifier>,
}

impl EnchantmentDefinition {
    #[must_use]
    pub fn min_cost(&self, level: i32) -> i32 {
        self.min_cost.at(level)
    }

    #[must_use]
    pub fn max_cost(&self, level: i32) -> i32 {
        self.max_cost.at(level)
    }
}

/// Startup-owned catalog, including recursively resolved tags and item
/// enchantability components. Its table pool retains tag order for weighted
/// selection; its definitions retain full network-registry order for clues.
#[derive(Debug, Clone)]
pub struct EnchantmentCatalog {
    definitions: Vec<EnchantmentDefinition>,
    by_name: BTreeMap<Identifier, usize>,
    table: Vec<usize>,
    enchantability: BTreeMap<Identifier, i32>,
    book: Identifier,
    enchanted_book: Identifier,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum HolderSet {
    Name(String),
    Names(Vec<String>),
}

#[derive(Deserialize)]
struct RawDefinition {
    max_level: i32,
    anvil_cost: i32,
    weight: i32,
    min_cost: EnchantmentCost,
    max_cost: EnchantmentCost,
    supported_items: HolderSet,
    primary_items: Option<HolderSet>,
    exclusive_set: Option<HolderSet>,
}

#[derive(Deserialize)]
struct RawTag {
    values: Vec<TagEntry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TagEntry {
    Name(String),
    Object {
        id: String,
        #[serde(default = "required_tag_entry")]
        required: bool,
    },
}

const fn required_tag_entry() -> bool {
    true
}

impl EnchantmentCatalog {
    /// Read the vanilla sidecar root (normally `data/vanilla`) at startup.
    /// Dynamic enchantments normally have no entry in `reports/registries.json`;
    /// in that case use the same complete sorted entry order as `mc_data::load`.
    /// If a report is supplied, enchantment protocol IDs come from it.
    pub fn load(vanilla_root: &Path) -> Result<Self, String> {
        let data = vanilla_root.join("data/minecraft");
        let report = read_value(&vanilla_root.join("reports/registries.json"))?;
        Self::from_sources(
            read_directory(&data.join("enchantment"))?,
            read_directory(&data.join("tags/enchantment"))?,
            read_directory(&data.join("tags/item"))?,
            read_directory(&vanilla_root.join("reports/minecraft/components/item"))?,
            Some(&report),
        )
    }

    /// Fixture convenience: cached catalog over the checkout's local vanilla
    /// sidecar. Production threads the `CoreRegistry` handle instead.
    #[cfg(test)]
    #[must_use]
    pub fn runtime() -> Self {
        static RUNTIME: std::sync::LazyLock<EnchantmentCatalog> = std::sync::LazyLock::new(|| {
            EnchantmentCatalog::load(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
            )
            .expect("local vanilla enchanting data loads")
        });
        RUNTIME.clone()
    }

    #[must_use]
    pub fn get(&self, id: &Identifier) -> Option<&EnchantmentDefinition> {
        self.by_name.get(id).map(|&index| &self.definitions[index])
    }

    /// Anvil applicability, using supported rather than primary items. Books
    /// are handled by the caller; they do not support arbitrary enchantments.
    #[must_use]
    pub fn supports(&self, rules: &ItemRules<'_>, stack: &ItemStack, id: &Identifier) -> bool {
        !stack.is_empty()
            && self
                .get(id)
                .is_some_and(|definition| definition.supported.contains(rules, stack))
    }

    /// Compatibility is symmetric even when only one definition lists the
    /// other in its exclusive set. The same enchantment is never compatible.
    #[must_use]
    pub fn compatible(&self, a: &Identifier, b: &Identifier) -> bool {
        a != b
            && self.get(a).zip(self.get(b)).is_some_and(|(a_def, b_def)| {
                !a_def.exclusive.contains(b) && !b_def.exclusive.contains(a)
            })
    }

    fn is_book(&self, rules: &ItemRules<'_>, stack: &ItemStack) -> bool {
        super::anvil::logical_item(rules, stack) == Some(&self.book)
    }

    fn enchantability(&self, rules: &ItemRules<'_>, stack: &ItemStack) -> Option<i32> {
        if stack.is_empty() || !stack.enchantments.is_empty() {
            return None;
        }
        // ItemStack's current model does not represent per-stack ENCHANTABLE
        // overrides. A custom provider model must not inherit its carrier's
        // vanilla table enchantability without such a component.
        if let Some(model) = stack.item_model.as_deref()
            && rules.facts.custom(model).is_some()
        {
            return None;
        }
        self.enchantability
            .get(super::anvil::logical_item(rules, stack)?)
            .copied()
    }

    fn from_sources(
        raw_definitions: BTreeMap<Identifier, Value>,
        raw_enchantment_tags: BTreeMap<Identifier, Value>,
        raw_item_tags: BTreeMap<Identifier, Value>,
        item_components: BTreeMap<Identifier, Value>,
        report: Option<&Value>,
    ) -> Result<Self, String> {
        if raw_definitions.is_empty() {
            return Err("enchantment registry is empty".into());
        }
        let enchantment_tags = parse_tags(raw_enchantment_tags)?;
        let item_tags = parse_tags(raw_item_tags)?;
        let enchantment_names: BTreeSet<_> = raw_definitions.keys().cloned().collect();
        let curse = resolve_tag(
            &identifier("minecraft:curse")?,
            &enchantment_tags,
            Some(&enchantment_names),
            &mut Vec::new(),
        )?;
        let table_names = resolve_tag(
            &identifier("minecraft:in_enchanting_table")?,
            &enchantment_tags,
            Some(&enchantment_names),
            &mut Vec::new(),
        )?;
        let report_entries = report
            .and_then(|report| report.get("minecraft:enchantment"))
            .and_then(|registry| registry.get("entries"));
        let mut definitions = Vec::with_capacity(raw_definitions.len());
        let mut by_name = BTreeMap::new();
        let mut protocol_ids = BTreeSet::new();
        for (id, value) in raw_definitions {
            let raw: RawDefinition = serde_json::from_value(value)
                .map_err(|error| format!("enchantment {id}: {error}"))?;
            if !(1..=255).contains(&raw.max_level) || raw.weight <= 0 || raw.anvil_cost < 0 {
                return Err(format!(
                    "enchantment {id}: invalid level, weight or anvil cost"
                ));
            }
            let protocol_id = if let Some(entries) = report_entries {
                entries
                    .get(id.as_str())
                    .and_then(|entry| entry.get("protocol_id"))
                    .and_then(Value::as_i64)
                    .and_then(|id| i32::try_from(id).ok())
                    .filter(|id| *id >= 0)
                    .ok_or_else(|| format!("enchantment {id}: missing protocol ID"))?
            } else {
                i32::try_from(definitions.len()).map_err(|_| "enchantment registry is too large")?
            };
            if !protocol_ids.insert(protocol_id) {
                return Err(format!("duplicate enchantment protocol ID {protocol_id}"));
            }
            let supported = resolve_item_set(raw.supported_items, &item_tags)?;
            let primary = raw
                .primary_items
                .map(|set| resolve_item_set(set, &item_tags))
                .transpose()?;
            let exclusive = raw
                .exclusive_set
                .map(|set| resolve_holder_set(set, &enchantment_tags, Some(&enchantment_names)))
                .transpose()?
                .unwrap_or_default()
                .into_iter()
                .collect();
            by_name.insert(id.clone(), definitions.len());
            definitions.push(EnchantmentDefinition {
                curse: curse.contains(&id),
                id,
                protocol_id,
                max_level: raw.max_level,
                anvil_cost: raw.anvil_cost,
                weight: raw.weight,
                min_cost: raw.min_cost,
                max_cost: raw.max_cost,
                supported,
                primary,
                exclusive,
            });
        }
        let table: Vec<_> = table_names.iter().map(|id| by_name[id]).collect();
        if table
            .iter()
            .map(|&index| i64::from(definitions[index].weight))
            .sum::<i64>()
            > i64::from(i32::MAX)
        {
            return Err("enchantment table weights exceed Java's integer range".into());
        }
        let mut enchantability = BTreeMap::new();
        for (item, report) in item_components {
            if let Some(component) = report.pointer("/components/minecraft:enchantable") {
                let value = component
                    .get("value")
                    .and_then(Value::as_i64)
                    .and_then(|value| i32::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or_else(|| format!("item {item}: invalid enchantability"))?;
                if report
                    .pointer("/components/minecraft:enchantments")
                    .is_some()
                {
                    enchantability.insert(item, value);
                }
            }
        }
        Ok(Self {
            definitions,
            by_name,
            table,
            enchantability,
            book: identifier("minecraft:book")?,
            enchanted_book: identifier("minecraft:enchanted_book")?,
        })
    }
}

/// Java LegacyRandomSource/BitRandomSource's exact 48-bit LCG. Integer seeds
/// are sign-extended before XOR; bounded draws include Java's signed-overflow
/// rejection step. Only the methods consumed by enchanting are exposed here.
#[derive(Debug, Clone)]
pub struct LegacyRandom {
    state: u64,
}

impl LegacyRandom {
    #[must_use]
    pub fn new(seed: i64) -> Self {
        Self {
            state: ((seed as u64) ^ 25_214_903_917) & ((1_u64 << 48) - 1),
        }
    }

    pub fn set_seed(&mut self, seed: i64) {
        self.state = ((seed as u64) ^ 25_214_903_917) & ((1_u64 << 48) - 1);
    }

    fn next(&mut self, bits: u32) -> u32 {
        self.state = self.state.wrapping_mul(25_214_903_917).wrapping_add(11) & ((1_u64 << 48) - 1);
        (self.state >> (48 - bits)) as u32
    }

    pub fn next_i32(&mut self) -> i32 {
        self.next(32) as i32
    }

    pub fn next_int(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "LegacyRandom bound must be positive");
        if bound & (bound - 1) == 0 {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let sample = self.next(31) as i32;
            let modulo = sample % bound;
            if sample.wrapping_sub(modulo).wrapping_add(bound - 1) >= 0 {
                return modulo;
            }
        }
    }

    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * (1.0 / 16_777_216.0)
    }
}

/// Pure deterministic seed advancement for callers without an independent
/// player RNG. Vanilla `Player.onEnchantmentPerformed` instead draws a fresh
/// int from the player's unrelated RNG; callers with that RNG should use it.
#[must_use]
pub fn next_seed(seed: i32) -> i32 {
    LegacyRandom::new(i64::from(seed)).next_i32()
}

/// The nine EnchantmentMenu properties. Missing clues/levels are -1, disabled
/// costs are zero. Costs are required player levels, not the 1/2/3 level and
/// lapis amounts charged on a successful purchase (those belong to the caller).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableOffers {
    pub costs: [i32; 3],
    pub clues: [i32; 3],
    pub levels: [i32; 3],
}

impl Default for TableOffers {
    fn default() -> Self {
        Self {
            costs: [0; 3],
            clues: [-1; 3],
            levels: [-1; 3],
        }
    }
}

impl TableOffers {
    #[must_use]
    pub fn generate(
        catalog: &EnchantmentCatalog,
        rules: &ItemRules<'_>,
        input: &ItemStack,
        seed: i32,
        bookshelves: i32,
    ) -> Self {
        let mut offers = Self::default();
        let Some(enchantability) = catalog.enchantability(rules, input) else {
            return offers;
        };
        let mut random = LegacyRandom::new(i64::from(seed));
        offers.costs = offer_costs(&mut random, bookshelves);
        let mut available = Vec::with_capacity(catalog.table.len());
        let mut selected = Vec::with_capacity(catalog.table.len());
        for offer in 0..3 {
            if offers.costs[offer] > 0 {
                select_offer(
                    catalog,
                    rules,
                    input,
                    seed,
                    offer,
                    offers.costs[offer],
                    enchantability,
                    &mut random,
                    &mut available,
                    &mut selected,
                );
                if !selected.is_empty() {
                    let clue = selected[random.next_int(selected.len() as i32) as usize];
                    offers.clues[offer] = catalog.definitions[clue.index].protocol_id;
                    offers.levels[offer] = clue.level;
                }
            }
        }
        offers
    }
}

/// Apply precisely the enchantments behind the matching clue. Does not charge
/// XP/lapis, advance seeds, or check a player's level/creative mode. Invalid or
/// empty offers return None. Input components survive book conversion.
#[must_use]
pub fn apply_offer(
    catalog: &EnchantmentCatalog,
    rules: &ItemRules<'_>,
    input: &ItemStack,
    seed: i32,
    bookshelves: i32,
    offer: usize,
) -> Option<ItemStack> {
    if offer >= 3 {
        return None;
    }
    let enchantability = catalog.enchantability(rules, input)?;
    let mut random = LegacyRandom::new(i64::from(seed));
    let cost = offer_costs(&mut random, bookshelves)[offer];
    if cost <= 0 {
        return None;
    }
    let mut available = Vec::with_capacity(catalog.table.len());
    let mut selected = Vec::with_capacity(catalog.table.len());
    select_offer(
        catalog,
        rules,
        input,
        seed,
        offer,
        cost,
        enchantability,
        &mut random,
        &mut available,
        &mut selected,
    );
    if selected.is_empty() {
        return None;
    }
    let mut result = input.clone();
    let book = catalog.is_book(rules, input);
    if book {
        result.item_id = rules.items.id_of(&catalog.enchanted_book)?;
    }
    let enchantments = if book {
        &mut result.extended_mut().stored_enchantments
    } else {
        &mut result.enchantments
    };
    enchantments.reserve(selected.len());
    for selection in selected {
        let id = &catalog.definitions[selection.index].id;
        if let Some(existing) = enchantments.iter_mut().find(|existing| &existing.id == id) {
            existing.level = existing.level.max(selection.level);
        } else {
            enchantments.push(ItemEnchantment {
                id: id.clone(),
                level: selection.level,
            });
        }
    }
    enchantments.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    Some(result)
}

fn offer_costs(random: &mut LegacyRandom, bookshelves: i32) -> [i32; 3] {
    // The menu counts nonnegative shelves; clamp hostile caller input before
    // the bounded RNG draw while retaining vanilla's 15-shelf cap.
    let shelves = bookshelves.clamp(0, 15);
    std::array::from_fn(|slot| {
        let selected = random.next_int(8) + 1 + (shelves >> 1) + random.next_int(shelves + 1);
        let cost = match slot {
            0 => (selected / 3).max(1),
            1 => selected * 2 / 3 + 1,
            _ => selected.max(shelves * 2),
        };
        if cost < slot as i32 + 1 { 0 } else { cost }
    })
}

#[derive(Clone, Copy)]
struct Selection {
    index: usize,
    level: i32,
}

// Work buffers are reused across all three offers; identifiers are only
// cloned when actually constructing a purchased output stack.
#[allow(clippy::too_many_arguments)]
fn select_offer(
    catalog: &EnchantmentCatalog,
    rules: &ItemRules<'_>,
    input: &ItemStack,
    seed: i32,
    offer: usize,
    cost: i32,
    enchantability: i32,
    random: &mut LegacyRandom,
    available: &mut Vec<Selection>,
    selected: &mut Vec<Selection>,
) {
    available.clear();
    selected.clear();
    random.set_seed(i64::from(seed.wrapping_add(offer as i32)));
    let bound = enchantability / 4 + 1;
    let cost = cost
        .wrapping_add(1)
        .wrapping_add(random.next_int(bound))
        .wrapping_add(random.next_int(bound));
    let span = (random.next_float() + random.next_float() - 1.0_f32) * 0.15_f32;
    let adjusted = cost as f32 + cost as f32 * span;
    // Math.round(float) rounds the represented float, not a separately
    // float-rounded addition of 0.5; promotion here also handles ties exactly.
    let mut cost = ((f64::from(adjusted) + 0.5).floor() as i32).max(1);
    let book = catalog.is_book(rules, input);
    for &index in &catalog.table {
        let definition = &catalog.definitions[index];
        if !book
            && (!definition.supported.contains(rules, input)
                || definition
                    .primary
                    .as_ref()
                    .is_some_and(|primary| !primary.contains(rules, input)))
        {
            continue;
        }
        for level in (1..=definition.max_level).rev() {
            if cost >= definition.min_cost(level) && cost <= definition.max_cost(level) {
                available.push(Selection { index, level });
                break;
            }
        }
    }
    if available.is_empty() {
        return;
    }
    selected.push(weighted_pick(catalog, random, available));
    while random.next_int(50) <= cost {
        let last = &catalog.definitions[selected.last().expect("initial pick").index].id;
        available
            .retain(|candidate| catalog.compatible(last, &catalog.definitions[candidate.index].id));
        if available.is_empty() {
            break;
        }
        selected.push(weighted_pick(catalog, random, available));
        cost /= 2;
    }
    if book && selected.len() > 1 {
        let removed = random.next_int(selected.len() as i32) as usize;
        selected.remove(removed);
    }
}

fn weighted_pick(
    catalog: &EnchantmentCatalog,
    random: &mut LegacyRandom,
    available: &[Selection],
) -> Selection {
    let total: i32 = available
        .iter()
        .map(|candidate| catalog.definitions[candidate.index].weight)
        .sum();
    let mut choice = random.next_int(total);
    for &candidate in available {
        choice -= catalog.definitions[candidate.index].weight;
        if choice < 0 {
            return candidate;
        }
    }
    unreachable!("positive validated weights exhaust the random choice")
}

fn identifier(name: &str) -> Result<Identifier, String> {
    Identifier::parse(name).map_err(|error| format!("invalid identifier {name}: {error}"))
}

fn parse_tags(values: BTreeMap<Identifier, Value>) -> Result<BTreeMap<Identifier, RawTag>, String> {
    values
        .into_iter()
        .map(|(id, value)| {
            let tag =
                serde_json::from_value(value).map_err(|error| format!("tag {id}: {error}"))?;
            Ok((id, tag))
        })
        .collect()
}

fn resolve_item_set(
    set: HolderSet,
    tags: &BTreeMap<Identifier, RawTag>,
) -> Result<ItemSet, String> {
    let tag = match &set {
        HolderSet::Name(name) => name.strip_prefix('#').map(identifier).transpose()?,
        HolderSet::Names(_) => None,
    };
    let members = resolve_holder_set(set, tags, None)?.into_iter().collect();
    Ok(ItemSet { tag, members })
}

fn resolve_holder_set(
    set: HolderSet,
    tags: &BTreeMap<Identifier, RawTag>,
    registry: Option<&BTreeSet<Identifier>>,
) -> Result<Vec<Identifier>, String> {
    let names = match set {
        HolderSet::Name(name) => vec![name],
        HolderSet::Names(names) => names,
    };
    let mut members = Vec::new();
    for name in names {
        let resolved = if let Some(tag) = name.strip_prefix('#') {
            resolve_tag(&identifier(tag)?, tags, registry, &mut Vec::new())?
        } else {
            let id = identifier(&name)?;
            if registry.is_some_and(|registry| !registry.contains(&id)) {
                return Err(format!("unknown enchantment {id}"));
            }
            vec![id]
        };
        for id in resolved {
            if !members.contains(&id) {
                members.push(id);
            }
        }
    }
    Ok(members)
}

fn resolve_tag(
    id: &Identifier,
    tags: &BTreeMap<Identifier, RawTag>,
    registry: Option<&BTreeSet<Identifier>>,
    visiting: &mut Vec<Identifier>,
) -> Result<Vec<Identifier>, String> {
    if visiting.contains(id) {
        return Err(format!("cyclic tag reference {id}"));
    }
    let tag = tags.get(id).ok_or_else(|| format!("missing tag {id}"))?;
    visiting.push(id.clone());
    let mut members = Vec::new();
    for entry in &tag.values {
        let (name, required) = match entry {
            TagEntry::Name(name) => (name, true),
            TagEntry::Object { id, required } => (id, *required),
        };
        let referenced = identifier(name.strip_prefix('#').unwrap_or(name))?;
        let resolved = if name.starts_with('#') {
            if !required && !tags.contains_key(&referenced) {
                continue;
            }
            resolve_tag(&referenced, tags, registry, visiting)?
        } else {
            if registry.is_some_and(|registry| !registry.contains(&referenced)) {
                if !required {
                    continue;
                }
                return Err(format!("tag {id}: unknown enchantment {referenced}"));
            }
            vec![referenced]
        };
        for member in resolved {
            if !members.contains(&member) {
                members.push(member);
            }
        }
    }
    visiting.pop();
    Ok(members)
}

fn read_value(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn read_directory(root: &Path) -> Result<BTreeMap<Identifier, Value>, String> {
    fn visit(
        root: &Path,
        directory: &Path,
        output: &mut BTreeMap<Identifier, Value>,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?
        {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                visit(root, &path, output)?;
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                let name = path
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .with_extension("");
                let id = identifier(&format!(
                    "minecraft:{}",
                    name.to_string_lossy().replace('\\', "/")
                ))?;
                output.insert(id, read_value(&path)?);
            }
        }
        Ok(())
    }
    let mut values = BTreeMap::new();
    visit(root, root, &mut values)?;
    Ok(values)
}

#[cfg(test)]
#[path = "enchanting_tests.rs"]
mod tests;
