//! Immutable authored extensions of the native recipe catalog.

use serde::{Deserialize, Serialize};

use crate::{Identifier, ItemStack};

/// Exact native component predicate/result. `custom_data` is canonical NBT,
/// interpreted only by its author; native crafting does not interpret product tiers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkRecipeComponents {
    pub damage: Option<i32>,
    pub enchantments: Vec<WorkRecipeEnchantment>,
    pub custom_name: Option<String>,
    pub item_model: Option<Identifier>,
    pub stew_effects: Vec<crate::item_stack::StewEffect>,
    #[serde(
        default,
        deserialize_with = "crate::item_stack::deserialize_max_damage"
    )]
    pub max_damage: Option<u32>,
    #[serde(
        default,
        deserialize_with = "crate::item_stack::deserialize_custom_data"
    )]
    pub custom_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkRecipeEnchantment {
    pub id: Identifier,
    pub level: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WorkRecipeAlternative {
    Item(Identifier),
    Tag(Identifier),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkRecipeInput {
    pub item: Option<Identifier>,
    pub tag: Option<Identifier>,
    /// Additional native OR alternatives; absent for a single authored identity.
    #[serde(default)]
    pub alternatives: Vec<WorkRecipeAlternative>,
    pub count: u32,
    /// `None` uses the existing ingredient identity semantics; `Some` matches
    /// every native component exactly, without substituting an unequal stack.
    pub components: Option<WorkRecipeComponents>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkRecipeStack {
    pub item: Identifier,
    pub count: u32,
    pub components: WorkRecipeComponents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkRecipeTool {
    pub item: Identifier,
    pub components: Option<WorkRecipeComponents>,
    pub wear: u32,
}

/// Startup-authored physical data, not a workflow or a product policy engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkRecipe {
    pub id: Identifier,
    pub revision: String,
    pub station: Identifier,
    pub duration_ticks: u64,
    pub inputs: Vec<WorkRecipeInput>,
    pub outputs: Vec<WorkRecipeStack>,
    pub remainders: Vec<WorkRecipeStack>,
    pub tools: Vec<WorkRecipeTool>,
}

impl WorkRecipeComponents {
    #[must_use]
    pub fn matches(&self, stack: &ItemStack) -> bool {
        self.damage == stack.damage
            && self.max_damage == stack.extended().max_damage
            && self.custom_name.as_deref() == stack.custom_name.as_deref()
            && self.item_model.as_ref() == stack.item_model.as_deref()
            && self.stew_effects == stack.stew_effects
            && self.custom_data == stack.extended().custom_data
            && self.enchantments.len() == stack.enchantments.len()
            && self
                .enchantments
                .iter()
                .zip(&stack.enchantments)
                .all(|(expected, actual)| {
                    expected.id == actual.id && expected.level == actual.level
                })
    }

    #[must_use]
    pub fn of(stack: &ItemStack) -> Self {
        Self {
            damage: stack.damage,
            enchantments: stack
                .enchantments
                .iter()
                .map(|value| WorkRecipeEnchantment {
                    id: value.id.clone(),
                    level: value.level,
                })
                .collect(),
            custom_name: stack.custom_name.as_deref().map(str::to_owned),
            item_model: stack.item_model.as_deref().cloned(),
            stew_effects: stack.stew_effects.clone(),
            max_damage: stack.extended().max_damage,
            custom_data: stack.extended().custom_data.clone(),
        }
    }

    pub fn apply(&self, stack: &mut ItemStack) {
        stack.damage = self.damage;
        stack.enchantments = self
            .enchantments
            .iter()
            .map(|value| crate::ItemEnchantment {
                id: value.id.clone(),
                level: value.level,
            })
            .collect();
        stack.custom_name = self.custom_name.clone().map(String::into_boxed_str);
        stack.item_model = self.item_model.clone().map(std::sync::Arc::new);
        stack.stew_effects = self.stew_effects.clone();
        if self.max_damage.is_some() || self.custom_data.is_some() || stack.components.is_some() {
            let components = stack.extended_mut();
            components.max_damage = self.max_damage;
            components.custom_data = self.custom_data.clone();
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.damage.is_some_and(|value| value < 0)
            || self
                .max_damage
                .is_some_and(|value| crate::item_stack::validate_max_damage(value).is_err())
            || self.enchantments.len() > 16
            || self
                .custom_name
                .as_ref()
                .is_some_and(|value| value.len() > 256)
            || self.custom_data.as_ref().is_some_and(|value| {
                !crate::item_stack::canonicalize_custom_data(value)
                    .is_ok_and(|canonical| canonical == *value)
            })
            || self
                .enchantments
                .iter()
                .any(|value| !(1..=255).contains(&value.level))
        {
            return Err("invalid native recipe components");
        }
        Ok(())
    }
}

impl WorkRecipeStack {
    pub fn to_stack(
        &self,
        items: &crate::items::ItemRegistry,
        facts: &crate::item_components::ItemFactsTable,
    ) -> Option<ItemStack> {
        let result = super::RecipeResult {
            item: self.item.clone(),
            count: self.count,
            stew_effects: Vec::new(),
        };
        let mut stack = result.to_stack(items, facts)?;
        // Custom item identity comes from the existing native catalog. An authored
        // component overlay cannot replace its carrier/model identity.
        let identity_model = stack.item_model.clone();
        let identity_name = stack.custom_name.clone();
        self.components.apply(&mut stack);
        if identity_model.is_some() {
            if stack.item_model.is_some() && stack.item_model != identity_model {
                return None;
            }
            stack.item_model = identity_model;
            if stack.custom_name.is_none() {
                stack.custom_name = identity_name;
            }
        }
        Some(stack)
    }
}

impl WorkRecipe {
    pub fn validate(
        &self,
        items: &crate::items::ItemRegistry,
        facts: &crate::item_components::ItemFactsTable,
    ) -> Result<(), &'static str> {
        if self.revision.is_empty()
            || self.revision.len() > 64
            || self.duration_ticks == 0
            || self.duration_ticks > 1_728_000
            || self.inputs.is_empty()
            || self.inputs.len() > 16
            || self.outputs.is_empty()
            || self.outputs.len() + self.remainders.len() > 16
            || self.tools.len() > 8
            || !matches!(
                self.station.as_str(),
                "minecraft:crafting_table" | "minecraft:stonecutter" | "minecraft:campfire"
            )
        {
            return Err("invalid bounded native work recipe");
        }
        for input in &self.inputs {
            if (input.item.is_none() && input.tag.is_none() && input.alternatives.is_empty())
                || (input.item.is_some() && input.tag.is_some())
                || input.alternatives.len() > 16
                || input.count == 0
                || input.count > 4096
            {
                return Err("invalid native work ingredient");
            }
            if let Some(item) = &input.item
                && items.id_of(item).is_none()
                && facts.custom(item).is_none()
            {
                return Err("unknown recipe ingredient");
            }
            for alternative in &input.alternatives {
                if let WorkRecipeAlternative::Item(item) = alternative
                    && items.id_of(item).is_none()
                    && facts.custom(item).is_none()
                {
                    return Err("unknown recipe ingredient alternative");
                }
            }
            if let Some(components) = &input.components {
                components.validate()?;
            }
        }
        for output in self.outputs.iter().chain(&self.remainders) {
            output.components.validate()?;
            let stack = output
                .to_stack(items, facts)
                .ok_or("unknown recipe result")?;
            if stack.count <= 0 || stack.count > 4096 {
                return Err("invalid recipe result count");
            }
            if stack.extended().max_damage.is_some()
                && crate::item_semantics_26_1_2::max_stack_for_stack(facts, items, &stack) != 1
            {
                return Err("damageable recipe result must be unstackable");
            }
        }
        for tool in &self.tools {
            if tool.wear == 0
                || tool.wear > 32767
                || (items.id_of(&tool.item).is_none() && facts.custom(&tool.item).is_none())
            {
                return Err("invalid recipe tool wear");
            }
            if let Some(components) = &tool.components {
                components.validate()?;
            }
        }
        Ok(())
    }
}
