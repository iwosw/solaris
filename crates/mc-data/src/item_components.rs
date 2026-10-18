use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::food::FoodEntry;
use crate::{Identifier, read_json_file, visit_json_files};

pub use crate::item_profiles::*;

const REQUIRED_ITEM_COMPONENTS: &str = include_str!("../data/required_item_components.json");
const DEFAULT_CONSUME_SECONDS: f32 = 1.6;

#[derive(Debug, Error)]
pub enum ItemComponentsError {
    #[error("item component reports io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("item component report parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid item component report filename at {path}")]
    InvalidPath { path: PathBuf },
    #[error("invalid item identifier {value:?} in component report {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
}

#[derive(Debug, Clone, Default)]
pub struct ItemFactsTable {
    items: BTreeMap<Identifier, ItemFacts>,
    custom_items: BTreeMap<Identifier, CustomItemDefinition>,
    work_recipes: BTreeMap<Identifier, crate::recipes::work::WorkRecipe>,
    has_physical_profiles: bool,
}

/// A server-owned item identity whose wire representation uses a vanilla carrier.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomItemDefinition {
    pub id: Identifier,
    pub carrier: Identifier,
    pub name: String,
    pub crafting_ingredient: Option<Identifier>,
    pub facts: ItemFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CustomItemError {
    #[error("custom item identity already declared: {0}")]
    Duplicate(Identifier),
    #[error("custom item carrier is not registered: {0}")]
    UnknownCarrier(Identifier),
    #[error("custom item recipe ingredient is not registered: {0}")]
    UnknownRecipeIngredient(Identifier),
    #[error("custom item identity conflicts with vanilla: {0}")]
    VanillaIdentity(Identifier),
    #[error("custom item definition is invalid: {0}")]
    InvalidDefinition(Identifier),
}

impl ItemFactsTable {
    /// Admit immutable authored physical definitions before sharing startup facts.
    pub fn with_work_recipes(
        mut self,
        definitions: impl IntoIterator<Item = crate::recipes::work::WorkRecipe>,
        items: &crate::items::ItemRegistry,
    ) -> Result<Self, String> {
        for definition in definitions {
            definition.validate(items, &self).map_err(str::to_owned)?;
            let id = definition.id.clone();
            if self.work_recipes.insert(id.clone(), definition).is_some() {
                return Err(format!("duplicate native work recipe {id}"));
            }
        }
        Ok(self)
    }

    #[must_use]
    pub fn work_recipe(&self, id: &Identifier) -> Option<&crate::recipes::work::WorkRecipe> {
        self.work_recipes.get(id)
    }

    #[must_use]
    pub fn from_entries(entries: impl IntoIterator<Item = (Identifier, ItemFacts)>) -> Self {
        Self {
            items: entries.into_iter().collect(),
            custom_items: BTreeMap::new(),
            work_recipes: BTreeMap::new(),
            has_physical_profiles: false,
        }
    }

    #[must_use]
    pub fn get(&self, item: &Identifier) -> Option<&ItemFacts> {
        self.items.get(item)
    }

    /// Register a complete, validated startup catalog before sharing the table.
    pub fn with_custom_items(
        mut self,
        definitions: impl IntoIterator<Item = CustomItemDefinition>,
        items: &crate::items::ItemRegistry,
    ) -> Result<Self, CustomItemError> {
        for definition in definitions {
            if definition.id.namespace() == "minecraft" || items.id_of(&definition.id).is_some() {
                return Err(CustomItemError::VanillaIdentity(definition.id));
            }
            if items.id_of(&definition.carrier).is_none() {
                return Err(CustomItemError::UnknownCarrier(definition.carrier));
            }
            let facts = &definition.facts;
            if facts.physical_profile.as_ref().is_some_and(|profile| {
                profile
                    .validate(items, facts.max_damage, facts.equippable_slot.as_deref())
                    .is_err()
                    || facts.max_stack_size != Some(1)
            }) {
                return Err(CustomItemError::InvalidDefinition(definition.id));
            }
            if definition.name.is_empty()
                || definition.name.len() > 128
                || !matches!(facts.max_stack_size, Some(1..=64))
                || facts
                    .max_damage
                    .is_some_and(|damage| damage == 0 || damage > 32_767)
                || (facts.max_damage.is_some() && facts.max_stack_size != Some(1))
                || facts.equippable_slot.as_ref().is_some_and(|slot| {
                    !matches!(slot.as_str(), "head" | "chest" | "legs" | "feet")
                        || facts.max_stack_size != Some(1)
                })
                || facts
                    .attack_damage_modifier
                    .is_some_and(|value| !value.is_finite())
                || facts
                    .attack_speed_modifier
                    .is_some_and(|value| !value.is_finite())
            {
                return Err(CustomItemError::InvalidDefinition(definition.id));
            }
            self.has_physical_profiles |= facts.physical_profile.is_some();
            let id = definition.id.clone();
            if self.custom_items.insert(id.clone(), definition).is_some() {
                return Err(CustomItemError::Duplicate(id));
            }
        }
        for definition in self.custom_items.values() {
            if let Some(ingredient) = &definition.crafting_ingredient
                && items.id_of(ingredient).is_none()
                && !self.custom_items.contains_key(ingredient)
            {
                return Err(CustomItemError::UnknownRecipeIngredient(ingredient.clone()));
            }
            if let Some(profile) = &definition.facts.physical_profile {
                for consumption in profile
                    .attacks
                    .iter()
                    .filter_map(|attack| attack.consumption.as_ref())
                {
                    if consumption.item.namespace() != "minecraft"
                        && consumption.item.namespace() != definition.id.namespace()
                    {
                        return Err(CustomItemError::UnknownRecipeIngredient(
                            consumption.item.clone(),
                        ));
                    }
                    if items.id_of(&consumption.item).is_none()
                        && !self.custom_items.contains_key(&consumption.item)
                    {
                        return Err(CustomItemError::UnknownRecipeIngredient(
                            consumption.item.clone(),
                        ));
                    }
                }
            }
        }
        Ok(self)
    }

    #[must_use]
    pub fn into_custom_items(self) -> Vec<CustomItemDefinition> {
        self.custom_items.into_values().collect()
    }

    #[must_use]
    pub fn custom(&self, id: &Identifier) -> Option<&CustomItemDefinition> {
        self.custom_items.get(id)
    }

    #[must_use]
    pub const fn has_physical_profiles(&self) -> bool {
        self.has_physical_profiles
    }

    #[must_use]
    pub fn custom_for_stack(
        &self,
        stack: &crate::ItemStack,
        items: &crate::items::ItemRegistry,
    ) -> Option<&CustomItemDefinition> {
        let model = stack.item_model.as_deref()?;
        self.custom_for_carrier(model.as_str(), items.name_of(stack.item_id)?.as_str())
    }

    /// Borrowed native custody projection of the very same model/carrier gate.
    /// No alternate registry or opaque component lookup participates.
    pub fn custom_for_carrier(&self, model: &str, carrier: &str) -> Option<&CustomItemDefinition> {
        let definition = self.custom_items.get(model)?;
        (definition.carrier.as_str() == carrier).then_some(definition)
    }

    #[must_use]
    pub fn facts_for_stack<'a>(
        &'a self,
        stack: &crate::ItemStack,
        items: &crate::items::ItemRegistry,
    ) -> Option<&'a ItemFacts> {
        if let Some(model) = stack.item_model.as_deref()
            && self.custom(model).is_some()
        {
            return self.custom_for_stack(stack, items).map(|item| &item.facts);
        }
        self.get(items.name_of(stack.item_id)?)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemFacts {
    pub max_stack_size: Option<u32>,
    pub max_damage: Option<u32>,
    /// Default `minecraft:entity_data.id`, authored by the item report.
    pub spawn_entity: Option<Identifier>,
    /// Default `damage_resistant` includes the vanilla `is_fire` damage tag.
    pub fire_resistant: bool,
    pub food: Option<FoodEntry>,
    pub use_duration_ticks: Option<u32>,
    pub use_action: Option<UseAction>,
    pub use_remainder: Option<Identifier>,
    pub consume_effects: Vec<crate::food::ConsumeEffect>,
    pub tool: Option<ToolFacts>,
    pub equippable_slot: Option<String>,
    /// `None` uses the component codec's default (`true`).
    pub equippable_swappable: Option<bool>,
    /// Entity ids or a `#tag`; absent means any entity.
    pub equippable_allowed_entities: Option<Vec<String>>,
    pub weapon: bool,
    pub weapon_damage_per_attack: Option<u32>,
    pub weapon_disable_blocking_seconds: Option<f32>,
    pub blocks_attacks_disable_cooldown_scale: Option<f32>,
    pub attack_damage_modifier: Option<f32>,
    pub attack_speed_modifier: Option<f32>,
    /// Complete equipment modifiers, retaining vanilla ids and operations.
    pub attribute_modifiers: Vec<ItemAttributeModifier>,
    pub attack_range: Option<AttackRangeFacts>,
    pub armor: Option<ItemArmorFacts>,
    /// Provider-authored mechanics; only exact admitted model/carrier stacks use these.
    pub physical_profile: Option<Box<PhysicalItemProfile>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ItemAttributeModifier {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub id: String,
    pub amount: f64,
    pub operation: String,
    pub slot: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackRangeFacts {
    pub min_reach: f32,
    pub max_reach: f32,
    pub min_creative_reach: f32,
    pub max_creative_reach: f32,
    pub hitbox_margin: f32,
    pub mob_factor: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseAction {
    Eat,
    Drink,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolFacts {
    pub default_mining_speed: Option<f32>,
    pub damage_per_block: Option<u32>,
    pub rules: Vec<ToolRuleFacts>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolRuleFacts {
    pub blocks: Vec<String>,
    pub speed: Option<f32>,
    pub correct_for_drops: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemArmorFacts {
    pub slot: String,
    pub armor: f32,
    pub toughness: f32,
}

pub fn load_item_facts(
    report_dir: impl AsRef<Path>,
) -> Result<ItemFactsTable, ItemComponentsError> {
    let report_dir = report_dir.as_ref();
    if !report_dir.is_dir() {
        return Ok(ItemFactsTable::default());
    }

    let mut paths = Vec::new();
    visit_json_files(
        report_dir,
        &mut |path| {
            paths.push(path);
            Ok(())
        },
        &|path, source| ItemComponentsError::Io { path, source },
    )?;
    paths.sort();

    let mut items = BTreeMap::new();
    for path in paths {
        let id = id_from_path(report_dir, &path)?;
        let facts = load_one(&path)?;
        items.insert(id, facts);
    }
    Ok(ItemFactsTable::from_entries(items))
}

#[must_use]
pub fn solaris_required_item_facts() -> ItemFactsTable {
    let raw: BTreeMap<String, RawEmbeddedItemFacts> =
        serde_json::from_str(REQUIRED_ITEM_COMPONENTS)
            .expect("embedded required item component JSON is valid");
    ItemFactsTable::from_entries(raw.into_iter().map(|(id, facts)| {
        (
            Identifier::parse(id).expect("embedded required item component id is valid"),
            facts.into_facts(),
        )
    }))
}

fn id_from_path(root: &Path, path: &Path) -> Result<Identifier, ItemComponentsError> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| ItemComponentsError::InvalidPath {
            path: path.to_path_buf(),
        })?
        .with_extension("");
    let mut value = String::from("minecraft:");
    for component in rel.components() {
        if !value.ends_with(':') {
            value.push('/');
        }
        value.push_str(component.as_os_str().to_string_lossy().as_ref());
    }
    Identifier::parse(value.clone()).map_err(|_| ItemComponentsError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

fn load_one(path: &Path) -> Result<ItemFacts, ItemComponentsError> {
    let raw: RawItemComponents = read_json_file(
        path,
        &|path, source| ItemComponentsError::Io { path, source },
        &|path, source| ItemComponentsError::Parse { path, source },
    )?;
    Ok(raw.into_facts())
}

#[derive(Deserialize)]
struct RawItemComponents {
    components: RawComponents,
}

#[derive(Default, Deserialize)]
struct RawComponents {
    #[serde(rename = "minecraft:max_stack_size")]
    max_stack_size: Option<u32>,
    #[serde(rename = "minecraft:max_damage")]
    max_damage: Option<u32>,
    #[serde(rename = "minecraft:entity_data")]
    entity_data: Option<RawEntityData>,
    #[serde(rename = "minecraft:damage_resistant")]
    damage_resistant: Option<RawDamageResistant>,
    #[serde(rename = "minecraft:food")]
    food: Option<RawFood>,
    #[serde(rename = "minecraft:consumable")]
    consumable: Option<RawConsumable>,
    #[serde(rename = "minecraft:use_remainder")]
    use_remainder: Option<RawUseRemainder>,
    #[serde(rename = "minecraft:tool")]
    tool: Option<RawTool>,
    #[serde(rename = "minecraft:equippable")]
    equippable: Option<RawEquippable>,
    #[serde(rename = "minecraft:weapon")]
    weapon: Option<RawWeapon>,
    #[serde(rename = "minecraft:blocks_attacks")]
    blocks_attacks: Option<RawBlocksAttacks>,
    #[serde(rename = "minecraft:attack_range")]
    attack_range: Option<RawAttackRange>,
    #[serde(default, rename = "minecraft:attribute_modifiers")]
    attribute_modifiers: Vec<RawAttributeModifier>,
}

impl RawItemComponents {
    fn into_facts(self) -> ItemFacts {
        let RawComponents {
            max_stack_size,
            max_damage,
            entity_data,
            damage_resistant,
            food,
            consumable,
            use_remainder,
            tool,
            mut equippable,
            weapon,
            blocks_attacks,
            attack_range,
            attribute_modifiers,
        } = self.components;
        let attack_damage_modifier =
            find_mainhand_add_modifier(&attribute_modifiers, "minecraft:attack_damage");
        let attack_speed_modifier =
            find_mainhand_add_modifier(&attribute_modifiers, "minecraft:attack_speed");
        let armor = equippable.as_ref().and_then(|raw| {
            let armor = find_slot_add_modifier(&attribute_modifiers, "minecraft:armor", &raw.slot)?;
            let toughness = find_slot_add_modifier(
                &attribute_modifiers,
                "minecraft:armor_toughness",
                &raw.slot,
            )
            .unwrap_or(0.0);
            Some(ItemArmorFacts {
                slot: raw.slot.clone(),
                armor,
                toughness,
            })
        });
        let use_duration_ticks = consumable.as_ref().map(|raw| {
            ((raw.consume_seconds.unwrap_or(DEFAULT_CONSUME_SECONDS) * 20.0).round()) as u32
        });
        let use_action = consumable
            .as_ref()
            .map(|raw| match raw.animation.as_deref() {
                None | Some("eat") | Some("minecraft:eat") => UseAction::Eat,
                Some("drink") | Some("minecraft:drink") => UseAction::Drink,
                Some(_) => UseAction::Other,
            });
        ItemFacts {
            max_stack_size,
            max_damage,
            spawn_entity: entity_data.map(|data| data.id),
            fire_resistant: damage_resistant
                .is_some_and(|value| value.types == "#minecraft:is_fire"),
            food: food.filter(|_| consumable.is_some()).map(|raw| FoodEntry {
                food: raw.nutrition,
                saturation: raw.saturation,
                can_always_eat: raw.can_always_eat,
            }),
            use_duration_ticks,
            use_action,
            use_remainder: use_remainder.map(|raw| raw.id),
            consume_effects: consumable.map_or_else(Vec::new, |raw| raw.on_consume_effects),
            tool: tool.map(RawTool::into_facts),
            equippable_swappable: equippable.as_ref().and_then(|raw| raw.swappable),
            equippable_allowed_entities: equippable
                .as_mut()
                .and_then(|raw| raw.allowed_entities.take())
                .map(RawToolBlocks::into_vec),
            equippable_slot: equippable.map(|raw| raw.slot),
            weapon: weapon.is_some(),
            weapon_damage_per_attack: weapon
                .as_ref()
                .map(|raw| raw.item_damage_per_attack.unwrap_or(1)),
            weapon_disable_blocking_seconds: weapon
                .as_ref()
                .and_then(|raw| raw.disable_blocking_for_seconds),
            blocks_attacks_disable_cooldown_scale: blocks_attacks
                .as_ref()
                .map(|raw| raw.disable_cooldown_scale.unwrap_or(1.0)),
            attack_damage_modifier,
            attack_speed_modifier,
            attribute_modifiers,
            attack_range: attack_range.map(RawAttackRange::into_facts),
            armor,
            physical_profile: None,
        }
    }
}

#[derive(Deserialize)]
struct RawFood {
    nutrition: i32,
    saturation: f32,
    #[serde(default)]
    can_always_eat: bool,
}

#[derive(Deserialize)]
struct RawUseRemainder {
    id: Identifier,
}

#[derive(Deserialize)]
struct RawEntityData {
    id: Identifier,
}

#[derive(Deserialize)]
struct RawDamageResistant {
    types: String,
}

#[derive(Default, Deserialize)]
struct RawEmbeddedItemFacts {
    max_stack_size: Option<u32>,
    max_damage: Option<u32>,
    spawn_entity: Option<Identifier>,
    #[serde(default)]
    fire_resistant: bool,
    food: Option<FoodEntry>,
    use_duration_ticks: Option<u32>,
    use_action: Option<RawEmbeddedUseAction>,
    use_remainder: Option<Identifier>,
    #[serde(default)]
    consume_effects: Vec<crate::food::ConsumeEffect>,
    weapon: Option<bool>,
    weapon_damage_per_attack: Option<u32>,
    weapon_disable_blocking_seconds: Option<f32>,
    blocks_attacks_disable_cooldown_scale: Option<f32>,
    attack_damage_modifier: Option<f32>,
    attack_speed_modifier: Option<f32>,
    attack_range: Option<RawAttackRange>,
    tool: Option<RawTool>,
    equippable_slot: Option<String>,
    equippable_swappable: Option<bool>,
    equippable_allowed_entities: Option<RawToolBlocks>,
    #[serde(default)]
    attribute_modifiers: Vec<RawAttributeModifier>,
}

impl RawEmbeddedItemFacts {
    fn into_facts(self) -> ItemFacts {
        let mut attribute_modifiers = self.attribute_modifiers;
        if attribute_modifiers.is_empty() {
            attribute_modifiers = [
                ("attack_damage", self.attack_damage_modifier),
                ("attack_speed", self.attack_speed_modifier),
            ]
            .into_iter()
            .filter_map(|(kind, amount)| {
                amount.map(|amount| ItemAttributeModifier {
                    kind: format!("minecraft:{kind}"),
                    id: format!("minecraft:base_{kind}"),
                    amount: (f64::from(amount) * 1.0e6).round() / 1.0e6,
                    operation: "add_value".into(),
                    slot: Some("mainhand".into()),
                })
            })
            .collect();
        }
        let armor = self.equippable_slot.as_ref().and_then(|slot| {
            Some(ItemArmorFacts {
                slot: slot.clone(),
                armor: find_slot_add_modifier(&attribute_modifiers, "minecraft:armor", slot)?,
                toughness: find_slot_add_modifier(
                    &attribute_modifiers,
                    "minecraft:armor_toughness",
                    slot,
                )
                .unwrap_or(0.0),
            })
        });
        ItemFacts {
            max_stack_size: self.max_stack_size,
            max_damage: self.max_damage,
            spawn_entity: self.spawn_entity,
            fire_resistant: self.fire_resistant,
            food: self.food,
            use_duration_ticks: self.use_duration_ticks,
            use_action: self.use_action.map(RawEmbeddedUseAction::into_use_action),
            use_remainder: self.use_remainder,
            consume_effects: self.consume_effects,
            tool: self.tool.map(RawTool::into_facts),
            equippable_swappable: self.equippable_swappable,
            equippable_allowed_entities: self
                .equippable_allowed_entities
                .map(RawToolBlocks::into_vec),
            equippable_slot: self.equippable_slot,
            weapon: self.weapon.unwrap_or(false),
            weapon_damage_per_attack: self.weapon_damage_per_attack,
            weapon_disable_blocking_seconds: self.weapon_disable_blocking_seconds,
            blocks_attacks_disable_cooldown_scale: self.blocks_attacks_disable_cooldown_scale,
            attack_damage_modifier: self.attack_damage_modifier,
            attack_speed_modifier: self.attack_speed_modifier,
            attribute_modifiers,
            attack_range: self.attack_range.map(RawAttackRange::into_facts),
            armor,
            physical_profile: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawEmbeddedUseAction {
    Eat,
    Drink,
    Other,
}

impl RawEmbeddedUseAction {
    const fn into_use_action(self) -> UseAction {
        match self {
            Self::Eat => UseAction::Eat,
            Self::Drink => UseAction::Drink,
            Self::Other => UseAction::Other,
        }
    }
}

#[derive(Default, Deserialize)]
struct RawConsumable {
    consume_seconds: Option<f32>,
    animation: Option<String>,
    #[serde(default)]
    on_consume_effects: Vec<crate::food::ConsumeEffect>,
}

#[derive(Deserialize)]
struct RawTool {
    default_mining_speed: Option<f32>,
    damage_per_block: Option<u32>,
    #[serde(default)]
    rules: Vec<RawToolRule>,
}

impl RawTool {
    fn into_facts(self) -> ToolFacts {
        ToolFacts {
            default_mining_speed: self.default_mining_speed,
            damage_per_block: self.damage_per_block,
            rules: self
                .rules
                .into_iter()
                .map(|rule| ToolRuleFacts {
                    blocks: rule.blocks.into_vec(),
                    speed: rule.speed,
                    correct_for_drops: rule.correct_for_drops,
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
struct RawToolRule {
    blocks: RawToolBlocks,
    speed: Option<f32>,
    correct_for_drops: Option<bool>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawToolBlocks {
    One(String),
    Many(Vec<String>),
}

impl RawToolBlocks {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(block) => vec![block],
            Self::Many(blocks) => blocks,
        }
    }
}

#[derive(Deserialize)]
struct RawEquippable {
    slot: String,
    swappable: Option<bool>,
    allowed_entities: Option<RawToolBlocks>,
}

#[derive(Deserialize)]
struct RawWeapon {
    item_damage_per_attack: Option<u32>,
    disable_blocking_for_seconds: Option<f32>,
}

#[derive(Deserialize)]
struct RawBlocksAttacks {
    disable_cooldown_scale: Option<f32>,
}

#[derive(Deserialize)]
struct RawAttackRange {
    #[serde(default)]
    min_reach: f32,
    #[serde(default = "default_attack_max_reach")]
    max_reach: f32,
    #[serde(default)]
    min_creative_reach: f32,
    #[serde(default = "default_attack_creative_max_reach")]
    max_creative_reach: f32,
    #[serde(default = "default_attack_hitbox_margin")]
    hitbox_margin: f32,
    #[serde(default = "default_attack_mob_factor")]
    mob_factor: f32,
}

impl RawAttackRange {
    fn into_facts(self) -> AttackRangeFacts {
        AttackRangeFacts {
            min_reach: self.min_reach,
            max_reach: self.max_reach,
            min_creative_reach: self.min_creative_reach,
            max_creative_reach: self.max_creative_reach,
            hitbox_margin: self.hitbox_margin,
            mob_factor: self.mob_factor,
        }
    }
}

const fn default_attack_max_reach() -> f32 {
    3.0
}

const fn default_attack_creative_max_reach() -> f32 {
    5.0
}

const fn default_attack_hitbox_margin() -> f32 {
    0.3
}

const fn default_attack_mob_factor() -> f32 {
    1.0
}

type RawAttributeModifier = ItemAttributeModifier;

fn find_mainhand_add_modifier(modifiers: &[RawAttributeModifier], kind: &str) -> Option<f32> {
    find_slot_add_modifier(modifiers, kind, "mainhand")
}

fn find_slot_add_modifier(
    modifiers: &[RawAttributeModifier],
    kind: &str,
    slot: &str,
) -> Option<f32> {
    modifiers
        .iter()
        .find(|modifier| {
            normalize_id(&modifier.kind) == normalize_id(kind)
                && normalize_id(&modifier.operation) == "add_value"
                && modifier
                    .slot
                    .as_deref()
                    .is_some_and(|value| normalize_id(value) == normalize_id(slot))
        })
        .map(|modifier| modifier.amount as f32)
}

fn normalize_id(value: &str) -> &str {
    value.strip_prefix("minecraft:").unwrap_or(value)
}

#[cfg(test)]
#[path = "item_components_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "item_components_spawn_egg_tests.rs"]
mod spawn_egg_tests;

#[cfg(test)]
#[path = "item_components_fire_tests.rs"]
mod fire_tests;
