use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::{Identifier, read_json_file, visit_json_files};

mod block_aliases;
mod block_components;
mod block_state_copy;
pub mod chest_26_1_2;
mod context;
mod embedded;
mod embedded_block_components;
mod embedded_enchantments;
pub mod entity_26_1_2;
pub mod fishing_26_1_2;

pub use context::{
    BlockLootContext, LootContextError, LootContextItem, LootEnchantments, LootExplosion,
    LootRandomBinding, MAX_LOOT_CONTEXT_ENTRIES, MAX_LOOT_ENCHANTMENT_LEVEL,
};

#[cfg(test)]
mod context_tests;

const BUILTIN_SURVIVAL_LOOT: &str = include_str!("../data/survival_loot.json");

#[derive(Debug, Error)]
pub enum LootError {
    #[error("loot file {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid loot identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error("filesystem error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Default)]
pub struct LootTables {
    block_drops: BTreeMap<Identifier, Vec<LootDrop>>,
    block_loot: BTreeMap<Identifier, BlockLoot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LootDrop {
    pub item: Identifier,
    pub count: LootCount,
    pub components: Option<Box<mc_nbt::Tag>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LootCount {
    Fixed(u32),
    UniformInclusive { min: u32, max: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockLoot {
    random_sequence: Option<Identifier>,
    silk_touch_drops: Vec<BlockLootDrop>,
    tool_drops: Option<(Identifier, Vec<BlockLootDrop>)>,
    regular_drops: Vec<BlockLootDrop>,
    conditional_pools: Vec<BlockStateLootPool>,
}

impl BlockLoot {
    #[must_use]
    pub fn random_sequence(&self) -> Option<&Identifier> {
        self.random_sequence.as_ref()
    }

    #[must_use]
    fn drops_for_tool(&self, silk_touch: bool, tool: Option<&Identifier>) -> &[BlockLootDrop] {
        if let Some((required, drops)) = &self.tool_drops
            && tool == Some(required)
        {
            return drops;
        }
        if silk_touch && !self.silk_touch_drops.is_empty() {
            &self.silk_touch_drops
        } else {
            &self.regular_drops
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct BlockStateLootPool {
    predicate: BlockStatePredicate,
    entries: Vec<BlockStateLootEntry>,
}

#[derive(Debug, Clone, PartialEq)]
struct BlockStateLootEntry {
    predicate: BlockStatePredicate,
    drop: BlockLootDrop,
    counts: Vec<BlockStateCount>,
}

#[derive(Debug, Clone, PartialEq)]
struct BlockStateCount {
    predicate: BlockStatePredicate,
    count: LootCount,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BlockStatePredicate {
    block: Option<Identifier>,
    properties: BTreeMap<String, String>,
}

impl BlockStatePredicate {
    fn matches(&self, block: &Identifier, properties: &[(String, String)]) -> bool {
        self.block.as_ref().is_none_or(|expected| expected == block)
            && self.properties.iter().all(|(key, value)| {
                properties
                    .iter()
                    .any(|(actual_key, actual_value)| actual_key == key && actual_value == value)
            })
    }

    fn matches_context(&self, context: &BlockLootContext<'_>) -> bool {
        self.block
            .as_ref()
            .is_none_or(|expected| expected == context.block())
            && self
                .properties
                .iter()
                .all(|(key, value)| context.property(key) == Some(value.as_str()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockLootDrop {
    pub drop: LootDrop,
    pub fortune_bonus: Option<FortuneBonus>,
    random_chance: Option<f32>,
    fortune_chances: Vec<f32>,
    survives_explosion: bool,
    explosion_decay: bool,
    rolls: LootCount,
    bonus_rolls: LootCount,
    copy_components: Vec<block_components::CopyComponents>,
    copy_state: Vec<block_state_copy::CopyState>,
    dynamic_sherds: bool,
}

impl BlockLootDrop {
    #[must_use]
    pub fn plain(drop: LootDrop) -> Self {
        Self {
            drop,
            fortune_bonus: None,
            random_chance: None,
            fortune_chances: Vec::new(),
            survives_explosion: false,
            explosion_decay: false,
            rolls: LootCount::Fixed(1),
            bonus_rolls: LootCount::Fixed(0),
            copy_components: Vec::new(),
            copy_state: Vec::new(),
            dynamic_sherds: false,
        }
    }

    fn sample_simple_explosion_count(
        &self,
        explosion_radius: Option<f32>,
        next_float: &mut impl FnMut() -> f32,
        input_count: LootCount,
    ) -> Option<u32> {
        let count = self.simple_explosion_input_count(explosion_radius.is_some(), input_count)?;
        let Some(explosion_radius) = explosion_radius else {
            return Some(count);
        };
        if !explosion_radius.is_finite() || explosion_radius <= 0.0 {
            return None;
        }

        let probability = 1.0 / explosion_radius;
        if self.survives_explosion && next_float() > probability {
            return Some(0);
        }
        if self.explosion_decay {
            return Some((0..count).filter(|_| next_float() <= probability).count() as u32);
        }
        Some(count)
    }

    fn simple_explosion_input_count(
        &self,
        require_explosion_modifier: bool,
        input_count: LootCount,
    ) -> Option<u32> {
        // Explosions have no tool, so ore/uniform Fortune bonuses are identities
        // at level zero. Binomial bonuses can still roll extra rounds at zero.
        if matches!(
            self.fortune_bonus,
            Some(FortuneBonus::BinomialWithBonusCount { .. })
        ) || self.random_chance.is_some()
            || !self.fortune_chances.is_empty()
            || (require_explosion_modifier && !self.survives_explosion && !self.explosion_decay)
        {
            return None;
        }
        match input_count {
            LootCount::Fixed(count) => Some(count),
            LootCount::UniformInclusive { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FortuneBonus {
    OreDrops,
    UniformBonusCount { bonus_multiplier: u32 },
    BinomialWithBonusCount { extra_rounds: u32, probability: f32 },
}

pub const MAX_BLOCK_BONUS_ROLLS: u32 = 4_096;
pub const MAX_BLOCK_POOL_ROLLS: u32 = 4_096;
pub const MAX_BLOCK_OUTPUT_ITEMS: u64 = 65_536;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BlockLootEvaluationError {
    #[error("block loot random sequence mismatch: expected {expected:?}, got {actual:?}")]
    RandomSequenceMismatch {
        expected: Option<Identifier>,
        actual: Option<Identifier>,
    },
    #[error("block loot bonus rolls exceed the limit: {actual} > {maximum}")]
    BonusRollLimitExceeded { actual: u64, maximum: u64 },
    #[error("block loot probability must be finite")]
    InvalidProbability,
    #[error("block loot arithmetic overflow while {operation}")]
    ArithmeticOverflow { operation: &'static str },
    #[error("block loot explosion-decay input exceeds the limit: {actual} > {maximum}")]
    ExplosionDecayInputLimitExceeded { actual: u64, maximum: u64 },
    #[error("block loot output items exceed the limit: {actual} > {maximum}")]
    OutputItemLimitExceeded { actual: u64, maximum: u64 },
    #[error("block loot block entity: {reason}")]
    InvalidBlockEntity { reason: &'static str },
}

impl FortuneBonus {
    fn try_apply(
        self,
        count: u32,
        fortune_level: u32,
        random: &mut context::LootRandom,
    ) -> Result<u32, BlockLootEvaluationError> {
        match self {
            Self::OreDrops => {
                if fortune_level == 0 {
                    return Ok(count);
                }
                let width = u64::from(fortune_level).checked_add(2).ok_or(
                    BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "calculating ore Fortune range",
                    },
                )?;
                let roll = random.next_u64() % width;
                let multiplier = if roll == 0 { 1 } else { roll };
                u64::from(count)
                    .checked_mul(multiplier)
                    .and_then(|count| u32::try_from(count).ok())
                    .ok_or(BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "applying ore Fortune bonus",
                    })
            }
            Self::UniformBonusCount { bonus_multiplier } => {
                if fortune_level == 0 {
                    return Ok(count);
                }
                let max_bonus = u64::from(bonus_multiplier)
                    .checked_mul(u64::from(fortune_level))
                    .ok_or(BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "calculating uniform Fortune range",
                    })?;
                let width = max_bonus.checked_add(1).ok_or(
                    BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "calculating uniform Fortune range",
                    },
                )?;
                let bonus = random.next_u64() % width;
                u64::from(count)
                    .checked_add(bonus)
                    .and_then(|count| u32::try_from(count).ok())
                    .ok_or(BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "applying uniform Fortune bonus",
                    })
            }
            Self::BinomialWithBonusCount {
                extra_rounds,
                probability,
            } => {
                if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
                    return Err(BlockLootEvaluationError::InvalidProbability);
                }
                let rounds = u64::from(fortune_level)
                    .checked_add(u64::from(extra_rounds))
                    .ok_or(BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "calculating binomial Fortune rounds",
                    })?;
                if rounds > u64::from(MAX_BLOCK_BONUS_ROLLS) {
                    return Err(BlockLootEvaluationError::BonusRollLimitExceeded {
                        actual: rounds,
                        maximum: u64::from(MAX_BLOCK_BONUS_ROLLS),
                    });
                }
                if probability == 0.0 {
                    return Ok(count);
                }
                if probability == 1.0 {
                    return (0..rounds).try_fold(count, |count, _| {
                        count
                            .checked_add(1)
                            .ok_or(BlockLootEvaluationError::ArithmeticOverflow {
                                operation: "applying binomial Fortune bonus",
                            })
                    });
                }
                if fortune_level == 0 {
                    return Ok(count);
                }
                let mut count = count;
                for _ in 0..rounds {
                    if random.next_float() < probability {
                        count = count.checked_add(1).ok_or(
                            BlockLootEvaluationError::ArithmeticOverflow {
                                operation: "applying binomial Fortune bonus",
                            },
                        )?;
                    }
                }
                Ok(count)
            }
        }
    }
}

impl LootCount {
    pub fn try_sample(self, roll: u64) -> Result<u32, BlockLootEvaluationError> {
        match self {
            Self::Fixed(count) => Ok(count),
            Self::UniformInclusive { min, max } => {
                let width = u64::from(max.checked_sub(min).ok_or(
                    BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "calculating loot count range",
                    },
                )?)
                .checked_add(1)
                .ok_or(BlockLootEvaluationError::ArithmeticOverflow {
                    operation: "calculating loot count range",
                })?;
                let offset = u32::try_from(roll % width).map_err(|_| {
                    BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "converting loot count offset",
                    }
                })?;
                min.checked_add(offset)
                    .ok_or(BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "sampling loot count",
                    })
            }
        }
    }
}

impl LootDrop {
    #[must_use]
    pub fn single(item: Identifier) -> Self {
        Self {
            item,
            count: LootCount::Fixed(1),
            components: None,
        }
    }

    #[must_use]
    pub fn fixed(item: Identifier, count: u32) -> Self {
        Self {
            item,
            count: LootCount::Fixed(count),
            components: None,
        }
    }

    #[must_use]
    pub fn uniform(item: Identifier, min: u32, max: u32) -> Self {
        debug_assert!(min <= max);
        Self {
            item,
            count: LootCount::UniformInclusive { min, max },
            components: None,
        }
    }
}

impl LootTables {
    #[must_use]
    pub fn from_maps(block_drops: BTreeMap<Identifier, Identifier>) -> Self {
        Self::from_drop_maps(
            block_drops
                .into_iter()
                .map(|(source, item)| (source, LootDrop::single(item)))
                .collect(),
        )
    }

    #[must_use]
    pub fn from_drop_maps(block_drops: BTreeMap<Identifier, LootDrop>) -> Self {
        Self::from_drop_lists(
            block_drops
                .into_iter()
                .map(|(source, drop)| (source, vec![drop]))
                .collect(),
        )
    }

    #[must_use]
    pub fn from_drop_lists(block_drops: BTreeMap<Identifier, Vec<LootDrop>>) -> Self {
        Self {
            block_drops,
            block_loot: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn block_drop(&self, block: &Identifier) -> Option<&Identifier> {
        self.block_drop_stack(block).map(|drop| &drop.item)
    }

    #[must_use]
    pub fn block_drop_stack(&self, block: &Identifier) -> Option<&LootDrop> {
        self.block_drop_stacks(block)?.first()
    }

    #[must_use]
    pub fn block_drop_stacks(&self, block: &Identifier) -> Option<&[LootDrop]> {
        self.block_drops
            .get(block_aliases::canonical(block))
            .map(Vec::as_slice)
    }

    #[must_use]
    pub fn block_loot(&self, block: &Identifier) -> Option<&BlockLoot> {
        self.block_loot.get(block_aliases::canonical(block))
    }

    #[must_use]
    pub fn block_random_sequence(&self, block: &Identifier) -> Option<&Identifier> {
        self.block_loot(block).and_then(BlockLoot::random_sequence)
    }

    #[must_use]
    pub fn block_needs_entity(&self, block: &Identifier) -> bool {
        self.block_loot(block).is_some_and(|rule| {
            rule.regular_drops
                .iter()
                .chain(&rule.silk_touch_drops)
                .chain(
                    rule.conditional_pools
                        .iter()
                        .flat_map(|pool| pool.entries.iter().map(|entry| &entry.drop)),
                )
                .any(|drop| !drop.copy_components.is_empty() || drop.dynamic_sherds)
        })
    }

    #[must_use]
    pub fn block_copies_component(&self, block: &Identifier, component: &str) -> bool {
        self.block_loot(block).is_some_and(|rule| {
            rule.regular_drops
                .iter()
                .chain(&rule.silk_touch_drops)
                .chain(
                    rule.conditional_pools
                        .iter()
                        .flat_map(|pool| pool.entries.iter().map(|entry| &entry.drop)),
                )
                .any(|drop| {
                    drop.copy_components
                        .iter()
                        .any(|function| function.copies(component))
                })
        })
    }

    pub fn evaluate_block(
        &self,
        context: &BlockLootContext<'_>,
    ) -> Result<Option<Vec<LootDrop>>, BlockLootEvaluationError> {
        let expected_sequence = self.block_random_sequence(context.block());
        let actual_sequence = context.random_binding().sequence();
        if expected_sequence != actual_sequence {
            return Err(BlockLootEvaluationError::RandomSequenceMismatch {
                expected: expected_sequence.cloned(),
                actual: actual_sequence.cloned(),
            });
        }
        let mut random = context.random();
        let Some(rule) = self.block_loot(context.block()) else {
            let Some(drops) = self.block_drop_stacks(context.block()) else {
                return Ok(None);
            };
            let mut output_items = 0_u64;
            let mut sampled = Vec::new();
            for drop in drops {
                if let Some(explosion) = context.explosion()
                    && !survives_explosion(explosion, &mut random)
                {
                    continue;
                }
                let count = sample_loot_count(drop.count, &mut random)?;
                if count == 0 {
                    continue;
                }
                output_items = output_items.checked_add(u64::from(count)).ok_or(
                    BlockLootEvaluationError::ArithmeticOverflow {
                        operation: "adding block output items",
                    },
                )?;
                if output_items > MAX_BLOCK_OUTPUT_ITEMS {
                    return Err(BlockLootEvaluationError::OutputItemLimitExceeded {
                        actual: output_items,
                        maximum: MAX_BLOCK_OUTPUT_ITEMS,
                    });
                }
                sampled.push(LootDrop::fixed(drop.item.clone(), count));
            }
            return Ok(Some(sampled));
        };

        let mut drops: Vec<(&BlockLootDrop, &[BlockStateCount])> = rule
            .drops_for_tool(context.tool().silk_touch_level() > 0, context.tool().item())
            .iter()
            .map(|drop| (drop, &[][..]))
            .collect();
        for pool in &rule.conditional_pools {
            if !pool.predicate.matches_context(context) {
                continue;
            }
            if let Some(entry) = pool
                .entries
                .iter()
                .find(|entry| entry.predicate.matches_context(context))
            {
                drops.push((&entry.drop, &entry.counts));
            }
        }
        let mut sampled = Vec::new();
        let mut output_items = 0_u64;
        for (drop, counts) in drops {
            let roll_count =
                sample_block_pool_roll_count(&mut random, drop.rolls, drop.bonus_rolls)?;
            for _ in 0..roll_count {
                let chance = if drop.fortune_chances.is_empty() {
                    drop.random_chance
                } else {
                    let level = (context.tool().fortune_level() as usize)
                        .min(drop.fortune_chances.len() - 1);
                    Some(drop.fortune_chances[level])
                };
                if let Some(chance) = chance {
                    if !chance.is_finite() || !(0.0..=1.0).contains(&chance) {
                        return Err(BlockLootEvaluationError::InvalidProbability);
                    }
                    if chance == 0.0 || (chance < 1.0 && random.next_float() >= chance) {
                        continue;
                    }
                }
                if let Some(explosion) = context.explosion()
                    && drop.survives_explosion
                    && !survives_explosion(explosion, &mut random)
                {
                    continue;
                }
                if drop.dynamic_sherds {
                    let sherds = block_components::sherds(context, &drop.drop.item)?;
                    output_items = output_items.checked_add(sherds.len() as u64).ok_or(
                        BlockLootEvaluationError::ArithmeticOverflow {
                            operation: "adding dynamic block output items",
                        },
                    )?;
                    if output_items > MAX_BLOCK_OUTPUT_ITEMS {
                        return Err(BlockLootEvaluationError::OutputItemLimitExceeded {
                            actual: output_items,
                            maximum: MAX_BLOCK_OUTPUT_ITEMS,
                        });
                    }
                    sampled.extend(sherds.into_iter().map(LootDrop::single));
                    continue;
                }

                let mut baseline = sample_loot_count(drop.drop.count, &mut random)?;
                for function in counts {
                    if function.predicate.matches_context(context) {
                        baseline = sample_loot_count(function.count, &mut random)?;
                    }
                }
                let mut count = match drop.fortune_bonus {
                    Some(bonus) => {
                        bonus.try_apply(baseline, context.tool().fortune_level(), &mut random)?
                    }
                    None => baseline,
                };
                if let Some(explosion) = context.explosion()
                    && drop.explosion_decay
                {
                    if u64::from(count) > MAX_BLOCK_OUTPUT_ITEMS {
                        return Err(BlockLootEvaluationError::ExplosionDecayInputLimitExceeded {
                            actual: u64::from(count),
                            maximum: MAX_BLOCK_OUTPUT_ITEMS,
                        });
                    }
                    let survivors = (0..count)
                        .filter(|_| survives_explosion(explosion, &mut random))
                        .count();
                    count = u32::try_from(survivors).map_err(|_| {
                        BlockLootEvaluationError::ArithmeticOverflow {
                            operation: "converting explosion-decay survivors",
                        }
                    })?;
                }
                if count > 0 {
                    output_items = output_items.checked_add(u64::from(count)).ok_or(
                        BlockLootEvaluationError::ArithmeticOverflow {
                            operation: "adding block output items",
                        },
                    )?;
                    if output_items > MAX_BLOCK_OUTPUT_ITEMS {
                        return Err(BlockLootEvaluationError::OutputItemLimitExceeded {
                            actual: output_items,
                            maximum: MAX_BLOCK_OUTPUT_ITEMS,
                        });
                    }
                    let mut sampled_drop = LootDrop::fixed(drop.drop.item.clone(), count);
                    let mut components = Vec::new();
                    for function in &drop.copy_components {
                        function.apply(context, &mut components)?;
                    }
                    for function in &drop.copy_state {
                        function.apply(context, &mut components);
                    }
                    if !components.is_empty() {
                        sampled_drop.components = Some(Box::new(mc_nbt::Tag::Compound(components)));
                    }
                    sampled.push(sampled_drop);
                }
            }
        }
        Ok(Some(sampled))
    }

    /// Samples the exact simple block-explosion rules used by common fixed-count tables.
    /// `None` models `DESTROY`, where vanilla omits the explosion radius and explosion
    /// modifiers are identities. `Some(radius)` models `DESTROY_WITH_DECAY`.
    /// Complex counts, non-identity Fortune bonuses at level zero, and
    /// random-chance entries fail closed.
    pub fn block_explosion_drops(
        &self,
        block: &Identifier,
        properties: &[(String, String)],
        explosion_radius: Option<f32>,
        mut next_float: impl FnMut() -> f32,
    ) -> Option<Vec<LootDrop>> {
        let common_fallbacks;
        let drops = if let Some(rule) = self.block_loot(block) {
            let mut selected: Vec<_> = rule
                .drops_for_tool(false, None)
                .iter()
                .map(|drop| (drop, drop.drop.count))
                .collect();
            for pool in &rule.conditional_pools {
                if !pool.predicate.matches(block, properties) {
                    continue;
                }
                if let Some(entry) = pool
                    .entries
                    .iter()
                    .find(|entry| entry.predicate.matches(block, properties))
                {
                    let mut count = entry.drop.drop.count;
                    for function in &entry.counts {
                        if function.predicate.matches(block, properties) {
                            // This simple API cannot sample count providers.
                            if !matches!(function.count, LootCount::Fixed(_)) {
                                return None;
                            }
                            count = function.count;
                        }
                    }
                    selected.push((&entry.drop, count));
                }
            }
            selected
        } else {
            let fallback_drops = self
                .block_drop_stacks(block)
                .map(<[LootDrop]>::to_vec)
                .or_else(|| {
                    (block.as_str() == "minecraft:dirt")
                        .then(|| vec![LootDrop::single(block.clone())])
                })?;
            common_fallbacks = fallback_drops
                .into_iter()
                .map(|drop| BlockLootDrop {
                    drop,
                    fortune_bonus: None,
                    random_chance: None,
                    fortune_chances: Vec::new(),
                    survives_explosion: true,
                    explosion_decay: false,
                    rolls: LootCount::Fixed(1),
                    bonus_rolls: LootCount::Fixed(0),
                    copy_components: Vec::new(),
                    copy_state: Vec::new(),
                    dynamic_sherds: false,
                })
                .collect::<Vec<_>>();
            common_fallbacks
                .iter()
                .map(|drop| (drop, drop.drop.count))
                .collect()
        };

        if drops.iter().any(|(drop, count)| {
            drop.simple_explosion_input_count(explosion_radius.is_some(), *count)
                .is_none()
        }) {
            return None;
        }

        let mut sampled = Vec::new();
        for (drop, input_count) in drops {
            let count =
                drop.sample_simple_explosion_count(explosion_radius, &mut next_float, input_count)?;
            if count > 0 {
                sampled.push(LootDrop {
                    item: drop.drop.item.clone(),
                    count: LootCount::Fixed(count),
                    components: None,
                });
            }
        }
        Some(sampled)
    }

    #[must_use]
    pub fn total_drops(&self) -> usize {
        self.block_drops.values().map(Vec::len).sum()
    }

    pub fn fill_missing_from(&mut self, fallback: &Self) {
        for (source, drops) in &fallback.block_drops {
            if !self.block_drops.contains_key(source) && !self.block_loot.contains_key(source) {
                self.block_drops.insert(source.clone(), drops.clone());
                if let Some(rule) = fallback.block_loot.get(source) {
                    self.block_loot.insert(source.clone(), rule.clone());
                }
            }
        }
        for (source, rule) in &fallback.block_loot {
            if !self.block_drops.contains_key(source) && !self.block_loot.contains_key(source) {
                self.block_loot.insert(source.clone(), rule.clone());
            }
        }
    }
}

fn sample_loot_count(
    count: LootCount,
    random: &mut context::LootRandom,
) -> Result<u32, BlockLootEvaluationError> {
    match count {
        LootCount::Fixed(count) => Ok(count),
        LootCount::UniformInclusive { .. } => count.try_sample(random.next_u64()),
    }
}

fn survives_explosion(explosion: LootExplosion, random: &mut context::LootRandom) -> bool {
    let probability = 1.0 / explosion.radius();
    if probability >= 1.0 {
        return true;
    }
    if probability <= 0.0 {
        return false;
    }
    explosion.survives(random.next_float())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLootTables {
    #[serde(default)]
    blocks: BTreeMap<String, embedded::RawBlockDrops>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawDropList {
    Single(String),
    Multiple(Vec<String>),
}

impl RawDropList {
    fn into_items(self) -> Vec<String> {
        match self {
            Self::Single(item) => vec![item],
            Self::Multiple(items) => items,
        }
    }
}

/// Solaris survival tables with the vanilla mining-enchantment and
/// block-entity-custody rules compiled explicitly from `root`.
pub fn load_builtin(root: &Path) -> Result<LootTables, LootError> {
    let mut tables = from_str(
        BUILTIN_SURVIVAL_LOOT,
        Path::new("crates/mc-data/data/survival_loot.json"),
    )
    .expect("built-in Solaris survival loot JSON is valid");
    embedded_enchantments::apply(&mut tables, root)?;
    embedded_block_components::apply(&mut tables, root)?;
    Ok(tables)
}

#[cfg(test)]
pub(crate) fn builtin() -> LootTables {
    static BUILTIN: std::sync::LazyLock<LootTables> = std::sync::LazyLock::new(|| {
        load_builtin(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"))
            .expect("repo vanilla fixture loads builtin loot")
    });
    BUILTIN.clone()
}

pub fn load(path: impl AsRef<Path>) -> Result<LootTables, LootError> {
    let path = path.as_ref();
    let bytes = crate::sidecar::read_string(path).map_err(|source| LootError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    from_str(&bytes, path)
}

pub fn load_vanilla_subset(root: impl AsRef<Path>) -> Result<LootTables, LootError> {
    let root = root.as_ref();
    if !root.is_dir() {
        return Ok(LootTables::default());
    }

    let blocks = load_vanilla_blocks(&root.join("blocks"))?;
    Ok(LootTables {
        block_drops: blocks.drops,
        block_loot: blocks.rules,
    })
}

struct LoadedBlockLoot {
    drops: BTreeMap<Identifier, Vec<LootDrop>>,
    rules: BTreeMap<Identifier, BlockLoot>,
}

fn load_vanilla_blocks(dir: &Path) -> Result<LoadedBlockLoot, LootError> {
    if !dir.is_dir() {
        return Ok(LoadedBlockLoot {
            drops: BTreeMap::new(),
            rules: BTreeMap::new(),
        });
    }

    let mut paths = Vec::new();
    visit_json_files(
        dir,
        &mut |path| {
            paths.push(path);
            Ok(())
        },
        &|path, source| LootError::Io { path, source },
    )?;
    paths.sort();

    let mut drops = BTreeMap::new();
    let mut rules = BTreeMap::new();
    for path in paths {
        let source = id_from_file(dir, &path)?;
        let value: serde_json::Value = read_json_file(
            &path,
            &|path, source| LootError::Io { path, source },
            &|path, source| LootError::Malformed { path, source },
        )?;
        if let Some(table_drops) = simple_drops_from_table(&path, &value, true)? {
            drops.insert(source.clone(), table_drops);
        }
        if let Some(rule) = block_loot_from_table(&path, &value)? {
            rules.insert(source, rule);
        }
    }
    Ok(LoadedBlockLoot { drops, rules })
}

fn id_from_file(root: &Path, path: &Path) -> Result<Identifier, LootError> {
    let rel = path
        .strip_prefix(root)
        .expect("walk yields paths under loot root")
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

fn simple_drops_from_table(
    path: &Path,
    value: &serde_json::Value,
    allow_uniform_counts: bool,
) -> Result<Option<Vec<LootDrop>>, LootError> {
    let Some(pools) = value.get("pools").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };
    if pools.iter().any(has_unsupported_pool_rolls) {
        return Ok(None);
    }

    let mut drops = Vec::new();
    for pool in pools {
        if has_unsupported_conditions(pool) {
            continue;
        }
        let Some(pool_count) = supported_count_from_functions(pool, allow_uniform_counts) else {
            continue;
        };
        let Some(entries) = pool.get("entries").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for entry in entries {
            if let Some(mut drop) = simple_drop_from_entry(path, entry, allow_uniform_counts)? {
                if let Some(count) = pool_count {
                    drop.count = count;
                }
                drops.push(drop);
                break;
            }
        }
    }
    Ok((!drops.is_empty()).then_some(drops))
}

fn simple_drop_from_entry(
    path: &Path,
    entry: &serde_json::Value,
    allow_uniform_counts: bool,
) -> Result<Option<LootDrop>, LootError> {
    match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("minecraft:item") => {
            if has_unsupported_conditions(entry) {
                return Ok(None);
            }
            let Some(count) = supported_count_from_functions(entry, allow_uniform_counts) else {
                return Ok(None);
            };
            let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
                return Ok(None);
            };
            parse_id(path, name.to_string()).map(|item| {
                Some(LootDrop {
                    item,
                    count: count.unwrap_or(LootCount::Fixed(1)),
                    components: None,
                })
            })
        }
        Some("minecraft:alternatives") => {
            if has_unsupported_conditions(entry) || entry.get("features").is_some() {
                return Ok(None);
            }
            let Some(children) = entry.get("children").and_then(serde_json::Value::as_array) else {
                return Ok(None);
            };
            for child in children {
                if let Some(drop) = simple_drop_from_entry(path, child, allow_uniform_counts)? {
                    return Ok(Some(drop));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn block_loot_from_table(
    path: &Path,
    value: &serde_json::Value,
) -> Result<Option<BlockLoot>, LootError> {
    let random_sequence = value
        .get("random_sequence")
        .and_then(serde_json::Value::as_str)
        .map(|sequence| parse_id(path, sequence.to_string()))
        .transpose()?;
    let pools: &[serde_json::Value] = match value.get("pools") {
        Some(pools) => match pools.as_array() {
            Some(pools) => pools,
            None => return Ok(None),
        },
        // Vanilla represents intentional no-drop tables without `pools`.
        // Preserve their presence instead of letting survival synthesize an item.
        None if value.get("type").and_then(serde_json::Value::as_str)
            == Some("minecraft:block") =>
        {
            &[]
        }
        None => return Ok(None),
    };
    if pools.iter().any(has_unsupported_pool_rolls) {
        return Ok(None);
    }

    let mut silk_touch_drops = Vec::new();
    let mut regular_drops = Vec::new();
    let mut conditional_pools = Vec::new();
    for pool in pools {
        if contains_block_state_condition(pool) {
            let Some((pool_rolls, pool_bonus_rolls)) = parse_pool_rolls(pool) else {
                return Ok(None);
            };
            if let Some(pool) = block_state_loot_pool(path, pool, pool_rolls, pool_bonus_rolls)? {
                conditional_pools.push(pool);
            }
            continue;
        }
        // Glass, ice, and similar blocks put their Silk Touch predicate on
        // the pool rather than on an alternative entry. Keep the empty
        // non-Silk branch so callers do not fall back to dropping the block.
        if has_silk_touch_condition(pool) {
            let Some((pool_rolls, pool_bonus_rolls)) = parse_pool_rolls(pool) else {
                return Ok(None);
            };
            let Some(pool_count) = supported_count_from_functions(pool, true) else {
                return Ok(None);
            };
            let Some(entries) = pool.get("entries").and_then(serde_json::Value::as_array) else {
                return Ok(None);
            };
            let [entry] = entries.as_slice() else {
                return Ok(None);
            };
            let Some(mut drop) =
                contextual_regular_drop(path, entry, pool_rolls, pool_bonus_rolls)?
            else {
                return Ok(None);
            };
            if let Some(count) = pool_count {
                drop.drop.count = count;
            }
            drop.explosion_decay |= has_explosion_decay_function(pool);
            silk_touch_drops.push(drop);
            continue;
        }
        if has_unsupported_conditions(pool) {
            continue;
        }
        let Some((pool_rolls, pool_bonus_rolls)) = parse_pool_rolls(pool) else {
            return Ok(None);
        };
        let Some(pool_count) = supported_count_from_functions(pool, true) else {
            continue;
        };
        let pool_survives_explosion = has_survives_explosion_condition(pool);
        let pool_explosion_decay = has_explosion_decay_function(pool);
        let Some(entries) = pool.get("entries").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for entry in entries {
            let Some((mut silk, mut regular)) =
                block_loot_from_entry(path, entry, pool_rolls, pool_bonus_rolls)?
            else {
                continue;
            };
            if let Some(count) = pool_count {
                if let Some(drop) = silk.as_mut() {
                    drop.drop.count = count;
                }
                regular.drop.count = count;
            }
            if let Some(drop) = silk.as_mut() {
                drop.survives_explosion |= pool_survives_explosion;
                drop.explosion_decay |= pool_explosion_decay;
            }
            regular.survives_explosion |= pool_survives_explosion;
            regular.explosion_decay |= pool_explosion_decay;
            if let Some(drop) = silk {
                silk_touch_drops.push(drop);
            }
            regular_drops.push(regular);
            break;
        }
    }

    Ok((!silk_touch_drops.is_empty()
        || !regular_drops.is_empty()
        || !conditional_pools.is_empty()
        || pools.is_empty())
    .then_some(BlockLoot {
        random_sequence,
        silk_touch_drops,
        tool_drops: None,
        regular_drops,
        conditional_pools,
    }))
}

fn contains_block_state_condition(value: &serde_json::Value) -> bool {
    value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition
                    .get("condition")
                    .and_then(serde_json::Value::as_str)
                    == Some("minecraft:block_state_property")
            })
        })
        || ["entries", "children", "functions"]
            .into_iter()
            .any(|field| {
                value
                    .get(field)
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|values| values.iter().any(contains_block_state_condition))
            })
}

fn block_state_loot_pool(
    path: &Path,
    pool: &serde_json::Value,
    pool_rolls: LootCount,
    pool_bonus_rolls: LootCount,
) -> Result<Option<BlockStateLootPool>, LootError> {
    let Some(predicate) = block_state_predicate(path, pool, false)? else {
        return Ok(None);
    };
    let Some(pool_count) = supported_count_from_functions(pool, true) else {
        return Ok(None);
    };
    let Some(entries) = pool.get("entries").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };
    let Some(entry) = entries.first() else {
        return Ok(None);
    };
    let mut parsed = Vec::new();
    match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("minecraft:item") => {
            if let Some(entry) =
                block_state_loot_entry(path, entry, pool_count, pool_rolls, pool_bonus_rolls)?
            {
                parsed.push(entry);
            }
        }
        Some("minecraft:alternatives") if entry.get("features").is_none() => {
            let Some(children) = entry.get("children").and_then(serde_json::Value::as_array) else {
                return Ok(None);
            };
            for child in children {
                let Ok(Some(entry)) =
                    block_state_loot_entry(path, child, pool_count, pool_rolls, pool_bonus_rolls)
                else {
                    return Ok(None);
                };
                parsed.push(entry);
            }
        }
        _ => return Ok(None),
    }
    for entry in &mut parsed {
        entry.drop.survives_explosion |= has_survives_explosion_condition(pool);
        entry.drop.explosion_decay |= has_explosion_decay_function(pool);
    }
    Ok((!parsed.is_empty()).then_some(BlockStateLootPool {
        predicate,
        entries: parsed,
    }))
}

fn block_state_loot_entry(
    path: &Path,
    entry: &serde_json::Value,
    pool_count: Option<LootCount>,
    pool_rolls: LootCount,
    pool_bonus_rolls: LootCount,
) -> Result<Option<BlockStateLootEntry>, LootError> {
    let dynamic_sherds = entry.get("type").and_then(serde_json::Value::as_str)
        == Some("minecraft:dynamic")
        && entry.get("name").and_then(serde_json::Value::as_str) == Some("minecraft:sherds")
        && entry.as_object().is_some_and(|fields| {
            fields
                .keys()
                .all(|key| matches!(key.as_str(), "type" | "name" | "conditions"))
        });
    if !dynamic_sherds
        && entry.get("type").and_then(serde_json::Value::as_str) != Some("minecraft:item")
    {
        return Ok(None);
    }
    let Some(predicate) = block_state_predicate(path, entry, true)? else {
        return Ok(None);
    };
    let Some(random_chance) = random_chance_condition(entry) else {
        return Ok(None);
    };
    let mut unconditional = entry.clone();
    unconditional
        .as_object_mut()
        .expect("loot entry is an object")
        .remove("conditions");
    // set_count conditions gate the count function, never the item entry.
    // Keep all count assignments in order, including unconditional ones.
    let mut counts = Vec::new();
    if let Some(functions) = unconditional
        .get_mut("functions")
        .and_then(serde_json::Value::as_array_mut)
    {
        for function in functions.iter().filter(|function| {
            function.get("function").and_then(serde_json::Value::as_str)
                == Some("minecraft:set_count")
        }) {
            let Some(fields) = function.as_object() else {
                return Ok(None);
            };
            if fields
                .keys()
                .any(|key| !matches!(key.as_str(), "function" | "conditions" | "count" | "add"))
                || function
                    .get("add")
                    .is_some_and(|add| add.as_bool() != Some(false))
                || function
                    .get("conditions")
                    .is_some_and(|conditions| !conditions.is_array())
                || function
                    .get("conditions")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|conditions| {
                        conditions.iter().any(|condition| {
                            condition
                                .get("condition")
                                .and_then(serde_json::Value::as_str)
                                != Some("minecraft:block_state_property")
                        })
                    })
            {
                return Ok(None);
            }
            let Some(predicate) = block_state_predicate(path, function, false)? else {
                return Ok(None);
            };
            let Some(count) = function
                .get("count")
                .and_then(|count| supported_count_provider(count, true))
            else {
                return Ok(None);
            };
            counts.push(BlockStateCount { predicate, count });
        }
        functions.retain(|function| {
            function.get("function").and_then(serde_json::Value::as_str)
                != Some("minecraft:set_count")
        });
    }
    let mut drop = if dynamic_sherds {
        let mut drop = BlockLootDrop::plain(LootDrop::single(
            Identifier::parse("minecraft:brick").expect("vanilla brick identifier"),
        ));
        drop.rolls = pool_rolls;
        drop.bonus_rolls = pool_bonus_rolls;
        drop.dynamic_sherds = true;
        drop
    } else {
        let Some(drop) =
            contextual_regular_drop(path, &unconditional, pool_rolls, pool_bonus_rolls)?
        else {
            return Ok(None);
        };
        drop
    };
    if let Some(count) = pool_count {
        counts.push(BlockStateCount {
            predicate: BlockStatePredicate::default(),
            count,
        });
    }
    drop.random_chance = random_chance;
    drop.survives_explosion |= has_survives_explosion_condition(entry);
    drop.explosion_decay |= has_explosion_decay_function(entry);
    Ok(Some(BlockStateLootEntry {
        predicate,
        drop,
        counts,
    }))
}

fn random_chance_condition(value: &serde_json::Value) -> Option<Option<f32>> {
    let Some(conditions) = value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
    else {
        return Some(None);
    };
    let mut chance = None;
    for condition in conditions {
        if condition
            .get("condition")
            .and_then(serde_json::Value::as_str)
            != Some("minecraft:random_chance")
        {
            continue;
        }
        if chance.is_some()
            || condition
                .as_object()?
                .keys()
                .any(|key| !matches!(key.as_str(), "condition" | "chance"))
        {
            return None;
        }
        let value = condition.get("chance")?.as_f64()?;
        if !(0.0..=1.0).contains(&value) {
            return None;
        }
        chance = Some(value as f32);
    }
    Some(chance)
}

fn block_state_predicate(
    path: &Path,
    value: &serde_json::Value,
    allow_random_chance: bool,
) -> Result<Option<BlockStatePredicate>, LootError> {
    let Some(conditions) = value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(Some(BlockStatePredicate::default()));
    };
    let mut predicate = BlockStatePredicate::default();
    for condition in conditions {
        match condition
            .get("condition")
            .and_then(serde_json::Value::as_str)
        {
            Some("minecraft:survives_explosion") => {}
            Some("minecraft:random_chance") if allow_random_chance => {}
            Some("minecraft:block_state_property") => {
                let Some(block) = condition.get("block").and_then(serde_json::Value::as_str) else {
                    return Ok(None);
                };
                let block = parse_id(path, block.to_string())?;
                if predicate
                    .block
                    .as_ref()
                    .is_some_and(|expected| expected != &block)
                {
                    return Ok(None);
                }
                predicate.block = Some(block);
                let Some(properties) = condition
                    .get("properties")
                    .and_then(serde_json::Value::as_object)
                else {
                    return Ok(None);
                };
                for (key, value) in properties {
                    let Some(value) = value.as_str() else {
                        return Ok(None);
                    };
                    if predicate
                        .properties
                        .insert(key.clone(), value.to_string())
                        .is_some()
                    {
                        return Ok(None);
                    }
                }
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(predicate))
}

fn block_loot_from_entry(
    path: &Path,
    entry: &serde_json::Value,
    pool_rolls: LootCount,
    pool_bonus_rolls: LootCount,
) -> Result<Option<(Option<BlockLootDrop>, BlockLootDrop)>, LootError> {
    match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("minecraft:item") => {
            Ok(
                contextual_regular_drop(path, entry, pool_rolls, pool_bonus_rolls)?
                    .map(|drop| (None, drop)),
            )
        }
        Some("minecraft:alternatives") => {
            if has_unsupported_conditions(entry) || entry.get("features").is_some() {
                return Ok(None);
            }
            let Some(children) = entry.get("children").and_then(serde_json::Value::as_array) else {
                return Ok(None);
            };
            let mut silk_touch_drop = None;
            let mut regular_drop = None;
            for child in children {
                if silk_touch_drop.is_none() {
                    silk_touch_drop =
                        silk_touch_drop_from_entry(path, child, pool_rolls, pool_bonus_rolls)?;
                    if silk_touch_drop.is_some() {
                        continue;
                    }
                }
                if regular_drop.is_none() {
                    regular_drop =
                        contextual_regular_drop(path, child, pool_rolls, pool_bonus_rolls)?;
                }
            }
            Ok(regular_drop.map(|regular| (silk_touch_drop, regular)))
        }
        _ => Ok(None),
    }
}

fn silk_touch_drop_from_entry(
    path: &Path,
    entry: &serde_json::Value,
    pool_rolls: LootCount,
    pool_bonus_rolls: LootCount,
) -> Result<Option<BlockLootDrop>, LootError> {
    if !has_silk_touch_condition(entry) {
        return Ok(None);
    }
    let mut unconditional = entry.clone();
    unconditional
        .as_object_mut()
        .expect("loot entry with conditions is an object")
        .remove("conditions");
    contextual_regular_drop(path, &unconditional, pool_rolls, pool_bonus_rolls)
}

fn contextual_regular_drop(
    path: &Path,
    entry: &serde_json::Value,
    rolls: LootCount,
    bonus_rolls: LootCount,
) -> Result<Option<BlockLootDrop>, LootError> {
    if entry
        .get("conditions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                !matches!(
                    condition
                        .get("condition")
                        .and_then(serde_json::Value::as_str),
                    Some("minecraft:random_chance" | "minecraft:survives_explosion")
                )
            })
        })
    {
        return Ok(None);
    }
    let Some(random_chance) = random_chance_condition(entry) else {
        return Ok(None);
    };
    let mut unconditional = entry.clone();
    unconditional
        .as_object_mut()
        .expect("loot item entry is an object")
        .remove("conditions");
    let Some(drop) = simple_drop_from_entry(path, &unconditional, true)? else {
        return Ok(None);
    };
    let Some(fortune_bonus) = fortune_bonus_from_functions(entry) else {
        return Ok(None);
    };
    let Some(copy_components) = block_components::functions(entry) else {
        return Ok(None);
    };
    let Some(copy_state) = block_state_copy::functions(entry) else {
        return Ok(None);
    };
    Ok(Some(BlockLootDrop {
        drop,
        fortune_bonus,
        random_chance,
        fortune_chances: Vec::new(),
        survives_explosion: has_survives_explosion_condition(entry),
        explosion_decay: has_explosion_decay_function(entry),
        rolls,
        bonus_rolls,
        copy_components,
        copy_state,
        dynamic_sherds: false,
    }))
}

fn has_survives_explosion_condition(value: &serde_json::Value) -> bool {
    value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition
                    .get("condition")
                    .and_then(serde_json::Value::as_str)
                    == Some("minecraft:survives_explosion")
            })
        })
}

fn has_explosion_decay_function(value: &serde_json::Value) -> bool {
    value
        .get("functions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|functions| {
            functions.iter().any(|function| {
                function.get("function").and_then(serde_json::Value::as_str)
                    == Some("minecraft:explosion_decay")
            })
        })
}

fn has_silk_touch_condition(entry: &serde_json::Value) -> bool {
    let Some(conditions) = entry
        .get("conditions")
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    if conditions.len() != 1 {
        return false;
    }
    let condition = &conditions[0];
    if condition
        .get("condition")
        .and_then(serde_json::Value::as_str)
        != Some("minecraft:match_tool")
    {
        return false;
    }
    condition
        .pointer("/predicate/predicates/minecraft:enchantments")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|enchantments| {
            enchantments.iter().any(|enchantment| {
                enchantment
                    .get("enchantments")
                    .and_then(serde_json::Value::as_str)
                    == Some("minecraft:silk_touch")
                    && enchantment
                        .pointer("/levels/min")
                        .and_then(supported_count_bound)
                        .is_some_and(|min| min >= 1)
            })
        })
}

fn fortune_bonus_from_functions(value: &serde_json::Value) -> Option<Option<FortuneBonus>> {
    let Some(functions) = value.get("functions").and_then(serde_json::Value::as_array) else {
        return Some(None);
    };
    let mut bonus = None;
    for function in functions {
        if function.get("function").and_then(serde_json::Value::as_str)
            != Some("minecraft:apply_bonus")
        {
            continue;
        }
        if function
            .get("enchantment")
            .and_then(serde_json::Value::as_str)
            != Some("minecraft:fortune")
            || bonus.is_some()
        {
            return None;
        }
        bonus = match function.get("formula").and_then(serde_json::Value::as_str) {
            Some("minecraft:ore_drops") if function.get("parameters").is_none() => {
                Some(FortuneBonus::OreDrops)
            }
            Some("minecraft:uniform_bonus_count") => {
                let parameters = function.get("parameters")?.as_object()?;
                if parameters.keys().any(|key| key != "bonusMultiplier") {
                    return None;
                }
                let bonus_multiplier = supported_count_bound(parameters.get("bonusMultiplier")?)?;
                Some(FortuneBonus::UniformBonusCount { bonus_multiplier })
            }
            Some("minecraft:binomial_with_bonus_count") => {
                let parameters = function.get("parameters")?.as_object()?;
                if parameters
                    .keys()
                    .any(|key| !matches!(key.as_str(), "extra" | "probability"))
                {
                    return None;
                }
                let extra_rounds = supported_count_bound(parameters.get("extra")?)?;
                let probability = parameters.get("probability")?.as_f64()?;
                if !(0.0..=1.0).contains(&probability) {
                    return None;
                }
                Some(FortuneBonus::BinomialWithBonusCount {
                    extra_rounds,
                    probability: probability as f32,
                })
            }
            _ => return None,
        };
    }
    Some(bonus)
}

fn has_unsupported_pool_rolls(pool: &serde_json::Value) -> bool {
    parse_pool_rolls(pool).is_none()
}

fn parse_pool_rolls(pool: &serde_json::Value) -> Option<(LootCount, LootCount)> {
    let rolls = pool
        .get("rolls")
        .map_or(Some(LootCount::Fixed(1)), parse_supported_count_bound)?;
    let bonus_rolls = pool
        .get("bonus_rolls")
        .map_or(Some(LootCount::Fixed(0)), parse_supported_count_bound)?;
    Some((rolls, bonus_rolls))
}

fn parse_supported_count_bound(value: &serde_json::Value) -> Option<LootCount> {
    if let Some(value) = supported_count_bound(value) {
        (value <= MAX_BLOCK_POOL_ROLLS).then_some(LootCount::Fixed(value))
    } else {
        let fields = value.as_object()?;
        if fields
            .keys()
            .any(|key| !matches!(key.as_str(), "type" | "min" | "max"))
        {
            return None;
        }
        if value.get("type").and_then(serde_json::Value::as_str) != Some("minecraft:uniform") {
            return None;
        }
        let min = supported_count_bound(value.get("min")?)?;
        let max = supported_count_bound(value.get("max")?)?;
        (min <= max && max <= MAX_BLOCK_POOL_ROLLS)
            .then_some(LootCount::UniformInclusive { min, max })
    }
}

fn sample_block_pool_roll_count(
    random: &mut context::LootRandom,
    rolls: LootCount,
    bonus_rolls: LootCount,
) -> Result<u64, BlockLootEvaluationError> {
    let rolls = sample_loot_count(rolls, random)?;
    let bonus_rolls = sample_loot_count(bonus_rolls, random)?;
    let total = u64::from(rolls).checked_add(u64::from(bonus_rolls)).ok_or(
        BlockLootEvaluationError::ArithmeticOverflow {
            operation: "adding block pool rolls",
        },
    )?;
    if total > u64::from(MAX_BLOCK_POOL_ROLLS) {
        return Err(BlockLootEvaluationError::BonusRollLimitExceeded {
            actual: total,
            maximum: u64::from(MAX_BLOCK_POOL_ROLLS),
        });
    }
    Ok(total)
}

fn has_unsupported_conditions(value: &serde_json::Value) -> bool {
    value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                !matches!(
                    condition
                        .get("condition")
                        .and_then(serde_json::Value::as_str),
                    Some("minecraft:survives_explosion")
                )
            })
        })
}

fn supported_count_from_functions(
    value: &serde_json::Value,
    allow_uniform_counts: bool,
) -> Option<Option<LootCount>> {
    let Some(functions) = value.get("functions").and_then(serde_json::Value::as_array) else {
        return Some(None);
    };
    let mut count = None;
    for function in functions {
        let fields = function.as_object()?;
        match function.get("function").and_then(serde_json::Value::as_str) {
            Some("minecraft:set_count") => {
                if fields
                    .keys()
                    .any(|key| !matches!(key.as_str(), "function" | "count" | "add"))
                    || function
                        .get("add")
                        .is_some_and(|add| add.as_bool() != Some(false))
                {
                    return None;
                }
                count = Some(supported_count_provider(
                    function.get("count")?,
                    allow_uniform_counts,
                )?);
            }
            // With no looting enchantment these functions do not change the count.
            Some("minecraft:enchanted_count_increase") => {}
            // Solaris currently evaluates baseline raw drops. A conditional smelt
            // therefore leaves the item unchanged when no burning context is supplied.
            Some("minecraft:furnace_smelt") if function.get("conditions").is_some() => {}
            // The baseline block-loot context has Fortune level zero and no explosion.
            // Both functions are therefore identity operations.
            Some("minecraft:apply_bonus")
                if supported_zero_fortune_bonus_function(fields, function) => {}
            Some("minecraft:explosion_decay") if fields.len() == 1 => {}
            Some("minecraft:copy_components")
                if block_components::CopyComponents::supported(function) => {}
            Some("minecraft:copy_state") if block_state_copy::CopyState::supported(function) => {}
            _ => return None,
        }
    }
    Some(count)
}

fn supported_zero_fortune_bonus_function(
    fields: &serde_json::Map<String, serde_json::Value>,
    function: &serde_json::Value,
) -> bool {
    if fields.keys().any(|key| {
        !matches!(
            key.as_str(),
            "function" | "enchantment" | "formula" | "parameters"
        )
    }) || function
        .get("enchantment")
        .and_then(serde_json::Value::as_str)
        != Some("minecraft:fortune")
    {
        return false;
    }
    let formula = function.get("formula").and_then(serde_json::Value::as_str);
    if !matches!(
        formula,
        Some(
            "minecraft:ore_drops"
                | "minecraft:uniform_bonus_count"
                | "minecraft:binomial_with_bonus_count"
        )
    ) {
        return false;
    }
    function
        .get("parameters")
        .is_none_or(serde_json::Value::is_object)
}

fn supported_count_provider(
    value: &serde_json::Value,
    allow_uniform_counts: bool,
) -> Option<LootCount> {
    if let Some(count) = supported_count_bound(value) {
        return (count <= 64).then_some(LootCount::Fixed(count));
    }

    if !allow_uniform_counts {
        return None;
    }
    let fields = value.as_object()?;
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "type" | "min" | "max"))
        || value.get("type").and_then(serde_json::Value::as_str) != Some("minecraft:uniform")
    {
        return None;
    }
    let min = supported_count_bound(value.get("min")?)?;
    let max = supported_count_bound(value.get("max")?)?;
    (min <= max && max <= 64).then_some(LootCount::UniformInclusive { min, max })
}

fn supported_count_bound(value: &serde_json::Value) -> Option<u32> {
    if let Some(value) = value.as_u64() {
        return u32::try_from(value).ok();
    }
    let value = value.as_f64()?;
    (value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX))
        .then_some(value as u32)
}

fn from_str(raw: &str, path: &Path) -> Result<LootTables, LootError> {
    let raw: RawLootTables = serde_json::from_str(raw).map_err(|source| LootError::Malformed {
        path: path.to_path_buf(),
        source,
    })?;
    let blocks = embedded::parse_blocks(path, raw.blocks)?;
    Ok(LootTables {
        block_drops: blocks.drops,
        block_loot: blocks.rules,
    })
}

fn parse_id(path: &Path, value: String) -> Result<Identifier, LootError> {
    Identifier::parse(value.clone()).map_err(|_| LootError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

#[cfg(test)]
#[path = "loot_tests.rs"]
mod tests;
