//! The startup a component deployment contributes.
//!
//! `configure` is the one phase of the component contract that answers something
//! other than commands: a `startup-contribution`. This module materializes that
//! answer into the world's startup contract, and it is the only place that does
//! so. The run path records what it produces here on the host, and
//! `check_deployment` refuses the same contribution with the same refusal, so a
//! deployment a check accepts is a deployment the run path materializes the same
//! way.
//!
//! The deployment's *own* declarations live here too: the client bundles and the
//! two worldgen profiles, settled by [`aggregate_deployment`] for the check and the
//! run path alike.
//!
//! Nothing here repairs a value. A `startup-contribution` field wider than the
//! startup field that has to hold it is refused rather than truncated or
//! saturated, because materializing rules other than the ones the package declared
//! would change the world fingerprint silently - which is the very decision the
//! fingerprint exists to make. A contribution that breaks a bound
//! [`GameplayRules::validate`] enforces is refused for the same reason: the native
//! owners downstream only bound what those checks let through.

use mc_data::Identifier;
use mc_data::item_components::{CustomItemDefinition, ItemFacts, ItemFactsTable};
use mc_data::items::{ItemRegistry, solaris_required_items};
use mc_data::tags::{TagsData, solaris_required_item_tags};
use std::collections::BTreeMap;
use std::sync::LazyLock;

use mc_script::{
    BiomeSpawns, ClayRule, ClientBundle, GameplayRules, GameplayRulesError,
    PluginWorldgenOreProfile, SpawnEntry, SpawnPlacement, TreeRule,
};

use crate::bindings::exports::solaris::plugin::lifecycle::{
    ClayRules, ItemDefinition, PlacementRules, SpawnGroup, SpawnRules, StartupContribution,
    TreeRules,
};
use crate::package::LoadedPackage;

static REQUIRED_ITEMS: LazyLock<ItemRegistry> = LazyLock::new(solaris_required_items);
static REQUIRED_ITEM_TAGS: LazyLock<TagsData> =
    LazyLock::new(|| solaris_required_item_tags(&REQUIRED_ITEMS));

/// A `startup-contribution` value the startup contract's own field cannot hold.
///
/// The field is named the way `lifecycle.wit` names it, so an operator can find
/// the declaration that has to change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("startup-contribution field {field} is {value}, the startup contract's bound is {max}")]
pub struct FieldOverflow {
    /// The WIT field that carried the value.
    pub field: &'static str,
    /// What the component declared.
    pub value: u64,
    /// The largest value the narrower contract field can hold.
    pub max: u64,
}

/// The deployment's own declarations: the client bundles the Loader stages and
/// the generic ore profile a world opens against.
///
/// They belong to the deployment rather than to one package, so both the check and
/// the run path settle them with [`aggregate_deployment`] - one function, one
/// policy - and a deployment a check accepts and a deployment that starts cannot
/// disagree about them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeploymentSurface {
    client_bundles: Vec<ClientBundle>,
    ore_profile: Option<(String, PluginWorldgenOreProfile)>,
}

impl DeploymentSurface {
    /// Every client bundle the deployment declared, in package order.
    #[must_use]
    pub fn client_bundles(&self) -> &[ClientBundle] {
        &self.client_bundles
    }

    /// The one ore profile the deployment declared, if any.
    #[must_use]
    pub fn ore_profile(&self) -> Option<PluginWorldgenOreProfile> {
        self.ore_profile.as_ref().map(|(_, profile)| *profile)
    }
}

/// Two packages that each declared the same worldgen profile.
///
/// A world opens against exactly one ore profile, so a
/// second declaration has no slot: it is refused rather than settled by keeping
/// the later one, which would silently change the world a package asked for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("plugins {first} and {second} both declare a worldgen {kind} profile")]
pub struct WorldgenConflict {
    /// Which profile both packages declared.
    pub kind: &'static str,
    /// The package whose declaration came first.
    pub first: String,
    /// The package that repeated it.
    pub second: String,
}

/// Settle the deployment-level declarations of `packages`.
///
/// The one place the catalog's bundles and the world's profiles are read from a
/// deployment, so the check and the run path decide duplicates under the same
/// refusal and cannot drift into a second policy.
pub fn aggregate_deployment(
    packages: &[LoadedPackage],
) -> Result<DeploymentSurface, WorldgenConflict> {
    let mut surface = DeploymentSurface::default();
    for package in packages {
        let id = package.manifest().plugin_id();
        if let Some(profile) = package.worldgen_ore_profile() {
            if let Some((first, _)) = &surface.ore_profile {
                return Err(WorldgenConflict {
                    kind: "ore",
                    first: first.clone(),
                    second: id.to_owned(),
                });
            }
            surface.ore_profile = Some((id.to_owned(), profile));
        }
        surface
            .client_bundles
            .extend(package.client_bundles().iter().cloned());
    }
    Ok(surface)
}

/// Why a component's `startup-contribution` did not become startup rules.
///
/// Every variant here is a refusal to materialize, not a warning: the
/// composition root fails startup closed on one, and a package whose `configure`
/// answered no contribution at all is not one of them (see
/// [`ContributionOutcome::NoContribution`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ContributionRefusal {
    /// A declared value does not fit the narrower startup field.
    #[error(transparent)]
    Overflow(#[from] FieldOverflow),
    /// Two `spawn-group`s in one `spawn-rules` share a name.
    ///
    /// The startup contract holds a biome's groups in a map keyed by group name,
    /// so a repeated name has no second slot: keeping either one would silently
    /// drop the other replacement the package asked for.
    #[error("spawn-rules for biome {biome:?} declares group {group:?} twice")]
    DuplicateSpawnGroup {
        /// The biome whose declaration repeats itself.
        biome: String,
        /// The group name declared twice.
        group: String,
    },
    /// The converted contribution breaks a bound the startup contract enforces.
    #[error(transparent)]
    Invalid(#[from] GameplayRulesError),
    #[error("item declaration {0}")]
    InvalidItem(String),
    #[error("work recipe declaration {0}")]
    InvalidWorkRecipe(String),
    #[error("crafting recipe declaration {0}")]
    InvalidCraftingRecipe(String),
    #[error("block tool declaration {0}")]
    InvalidBlockTool(String),
}

/// The distinct native startup domains one configure response contributes.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedStartupContribution {
    rules: Option<GameplayRules>,
    items: Vec<CustomItemDefinition>,
    work_recipes: Vec<mc_data::recipes::work::WorkRecipe>,
    crafting_recipes: Vec<mc_data::recipes::Recipe>,
    block_tools: Vec<mc_data::block_tools::BlockToolRule>,
}

impl ValidatedStartupContribution {
    #[must_use]
    pub fn rules(&self) -> Option<&GameplayRules> {
        self.rules.as_ref()
    }

    #[must_use]
    pub fn items(&self) -> &[CustomItemDefinition] {
        &self.items
    }

    /// Immutable native definitions decoded from configure's `work-recipes`.
    /// A provider may author at most 256 strict JSON definitions totaling 1 MiB;
    /// recipe identities belong to that provider and references must be registered.
    #[must_use]
    pub fn work_recipes(&self) -> &[mc_data::recipes::work::WorkRecipe] {
        &self.work_recipes
    }

    /// Crafting recipes whose results are this package's declared items.
    #[must_use]
    pub fn crafting_recipes(&self) -> &[mc_data::recipes::Recipe] {
        &self.crafting_recipes
    }

    /// Block-tool rules whose items are this package's declared items. Block
    /// names and properties are checked later, where the block registry exists.
    #[must_use]
    pub fn block_tools(&self) -> &[mc_data::block_tools::BlockToolRule] {
        &self.block_tools
    }
}

/// Convert a component's `configure` answer into the startup rule contract.
///
/// Total over the contract's shape: every category and every nested record is
/// carried across, and every value either fits its field exactly or the
/// contribution is refused. The result is validated, so the rules a caller
/// receives are rules a native owner can be handed.
pub fn convert_startup_contribution(
    plugin_id: &str,
    contribution: &StartupContribution,
) -> Result<ValidatedStartupContribution, ContributionRefusal> {
    // `option<list<..>>` and `list<..>` are different absences: `None` is "this
    // package sets no such category" and `Some(vec![])` is "it sets an empty
    // one", which `validate` refuses below because the plan then sets nothing.
    let spawning = contribution
        .spawning
        .iter()
        .flatten()
        .map(convert_spawn_rules)
        .collect::<Result<Vec<_>, _>>()?;
    let trees = contribution
        .trees
        .iter()
        .flatten()
        .map(convert_tree_rules)
        .collect::<Result<Vec<_>, _>>()?;
    let clay = contribution
        .clay
        .as_ref()
        .map(convert_clay_rules)
        .transpose()?;
    let placement = contribution
        .placement
        .as_ref()
        .map(convert_placement_rules)
        .transpose()?;

    let has_rules = contribution.spawning.is_some()
        || contribution.trees.is_some()
        || contribution.clay.is_some()
        || contribution.placement.is_some();
    let rules = if has_rules {
        let rules = GameplayRules::new(spawning, trees, clay, placement);
        rules.validate()?;
        Some(rules)
    } else {
        None
    };
    let items = contribution
        .items
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|item| convert_item_definition(plugin_id, item))
        .collect::<Result<Vec<_>, _>>()?;
    if items.len() > 16 {
        return Err(ContributionRefusal::InvalidItem(
            "more than 16 item definitions".to_owned(),
        ));
    }
    let items = ItemFactsTable::default()
        .with_custom_items(items, &REQUIRED_ITEMS)
        .map_err(|error| ContributionRefusal::InvalidItem(error.to_string()))?;
    let work_recipes = convert_work_recipes(plugin_id, contribution, &items)?;
    let crafting_recipes = convert_crafting_recipes(contribution, &items)?;
    let block_tools = convert_block_tools(plugin_id, contribution, &items)?;
    let items = items.into_custom_items();
    if rules.is_none() && items.is_empty() && work_recipes.is_empty() {
        return Err(ContributionRefusal::Invalid(GameplayRulesError::NoCategory));
    }
    Ok(ValidatedStartupContribution {
        rules,
        items,
        work_recipes,
        crafting_recipes,
        block_tools,
    })
}

/// Shaped/shapeless recipes a package authored for its own declared items. The
/// recipe identity is its result item, so a package cannot shadow a vanilla
/// recipe, and every ingredient must be a vanilla or owned item or a known tag.
fn convert_crafting_recipes(
    contribution: &StartupContribution,
    facts: &ItemFactsTable,
) -> Result<Vec<mc_data::recipes::Recipe>, ContributionRefusal> {
    let invalid = ContributionRefusal::InvalidCraftingRecipe;
    let definitions = contribution.crafting_recipes.as_deref().unwrap_or_default();
    if definitions.len() > 32
        || definitions
            .iter()
            .try_fold(0usize, |total, value| total.checked_add(value.len()))
            .is_none_or(|total| total > 65_536)
    {
        return Err(invalid("catalog exceeds 32 recipes or 64 KiB".to_owned()));
    }
    let mut recipes: Vec<mc_data::recipes::Recipe> = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let result = serde_json::from_str::<serde_json::Value>(definition)
            .map_err(|error| invalid(error.to_string()))?
            .pointer("/result/id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| invalid("recipe needs result.id".to_owned()))?;
        let id = Identifier::parse(result).map_err(|error| invalid(error.to_string()))?;
        if facts.custom(&id).is_none() {
            return Err(invalid(format!("result {id} is not a declared item")));
        }
        if recipes.iter().any(|recipe| recipe.id == id) {
            return Err(invalid(format!("duplicate recipe for {id}")));
        }
        let recipe =
            mc_data::recipes::parse_plugin_crafting_recipe(id, definition).map_err(invalid)?;
        let ingredients: Vec<&mc_data::recipes::Ingredient> = match &recipe.kind {
            mc_data::recipes::RecipeKind::Shaped(shaped) => shaped.key.values().collect(),
            mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
                shapeless.ingredients.iter().collect()
            }
            _ => Vec::new(),
        };
        let item_registry = Identifier::parse("minecraft:item").expect("static registry id");
        for alternative in ingredients
            .iter()
            .flat_map(|ingredient| &ingredient.alternatives)
        {
            let known = match alternative {
                mc_data::recipes::IngredientAlternative::Item(item) => {
                    REQUIRED_ITEMS.id_of(item).is_some() || facts.custom(item).is_some()
                }
                mc_data::recipes::IngredientAlternative::Tag(tag) => REQUIRED_ITEM_TAGS
                    .registries
                    .get(&item_registry)
                    .is_some_and(|registered| registered.contains_key(tag)),
            };
            if !known {
                return Err(invalid(format!(
                    "recipe {} has an unregistered ingredient",
                    recipe.id
                )));
            }
        }
        recipes.push(recipe);
    }
    Ok(recipes)
}

/// Block-tool rules a package authored for its own declared items; a `give`
/// must name a registered vanilla item.
fn convert_block_tools(
    plugin_id: &str,
    contribution: &StartupContribution,
    facts: &ItemFactsTable,
) -> Result<Vec<mc_data::block_tools::BlockToolRule>, ContributionRefusal> {
    let invalid = ContributionRefusal::InvalidBlockTool;
    let definitions = contribution.block_tools.as_deref().unwrap_or_default();
    let rules =
        mc_data::block_tools::parse_block_tool_rules(plugin_id, definitions).map_err(invalid)?;
    for rule in &rules {
        if facts.custom(&rule.item).is_none() {
            return Err(invalid(format!(
                "tool {} is not a declared item",
                rule.item
            )));
        }
        if rule
            .give
            .as_ref()
            .is_some_and(|give| REQUIRED_ITEMS.id_of(give).is_none())
        {
            return Err(invalid(format!(
                "rule for {} grants an unregistered item",
                rule.item
            )));
        }
    }
    Ok(rules)
}

/// Convert already-deserialized native work-recipe JSON through the exact
/// same validation the deployment path applies: parse each definition,
/// canonicalize `custom_data`, check ownership/duplicates/item and tag
/// references. Host-owned so the one conversion stays exact; native S1
/// startup authors its recipe JSON strings and hands them back here rather
/// than duplicating the validation.
pub fn convert_native_work_recipes(
    owner: &str,
    definitions: &[String],
    facts: &ItemFactsTable,
) -> Result<Vec<mc_data::recipes::work::WorkRecipe>, ContributionRefusal> {
    let invalid = |reason: String| ContributionRefusal::InvalidWorkRecipe(reason);
    convert_recipe_definitions(owner, definitions, facts, &invalid)
}

fn convert_work_recipes(
    plugin_id: &str,
    contribution: &StartupContribution,
    facts: &ItemFactsTable,
) -> Result<Vec<mc_data::recipes::work::WorkRecipe>, ContributionRefusal> {
    let definitions = contribution.work_recipes.as_deref().unwrap_or_default();
    let invalid = |reason: String| ContributionRefusal::InvalidWorkRecipe(reason);
    if definitions.len() > 256
        || definitions
            .iter()
            .try_fold(0usize, |total, value| total.checked_add(value.len()))
            .is_none_or(|total| total > 1_048_576)
    {
        return Err(invalid(
            "catalog exceeds 256 definitions or 1 MiB".to_owned(),
        ));
    }
    convert_recipe_definitions(plugin_id, definitions, facts, &invalid)
}

fn convert_recipe_definitions(
    owner: &str,
    definitions: &[String],
    facts: &ItemFactsTable,
    invalid: &impl Fn(String) -> ContributionRefusal,
) -> Result<Vec<mc_data::recipes::work::WorkRecipe>, ContributionRefusal> {
    let items = &*REQUIRED_ITEMS;
    let tags = &*REQUIRED_ITEM_TAGS;
    let item_registry = Identifier::parse("minecraft:item").expect("static registry id");
    let mut recipes = Vec::with_capacity(definitions.len());
    let mut ids = std::collections::BTreeSet::new();
    for definition in definitions {
        let mut recipe: mc_data::recipes::work::WorkRecipe =
            serde_json::from_str(definition).map_err(|error| invalid(error.to_string()))?;
        if recipe.id.namespace() != owner {
            return Err(invalid(format!("{} must be owned by {owner}", recipe.id)));
        }
        if !ids.insert(recipe.id.clone()) {
            return Err(invalid(format!("duplicate recipe {}", recipe.id)));
        }
        for components in recipe
            .inputs
            .iter_mut()
            .filter_map(|input| input.components.as_mut())
            .chain(
                recipe
                    .tools
                    .iter_mut()
                    .filter_map(|tool| tool.components.as_mut()),
            )
            .chain(
                recipe
                    .outputs
                    .iter_mut()
                    .chain(&mut recipe.remainders)
                    .map(|stack| &mut stack.components),
            )
        {
            if let Some(bytes) = components.custom_data.as_mut() {
                *bytes = mc_data::item_stack::canonicalize_custom_data(bytes)
                    .map_err(|error| invalid(format!("{}: {error}", recipe.id)))?;
            }
        }
        recipe
            .validate(items, facts)
            .map_err(|error| invalid(format!("{}: {error}", recipe.id)))?;
        for id in recipe
            .inputs
            .iter()
            .filter_map(|input| input.item.as_ref())
            .chain(
                recipe
                    .outputs
                    .iter()
                    .chain(&recipe.remainders)
                    .map(|stack| &stack.item),
            )
            .chain(recipe.tools.iter().map(|tool| &tool.item))
            .chain(
                recipe
                    .inputs
                    .iter()
                    .flat_map(|input| &input.alternatives)
                    .filter_map(|alternative| match alternative {
                        mc_data::recipes::work::WorkRecipeAlternative::Item(id) => Some(id),
                        mc_data::recipes::work::WorkRecipeAlternative::Tag(_) => None,
                    }),
            )
        {
            if id.namespace() != "minecraft" && id.namespace() != owner {
                return Err(invalid(format!(
                    "recipe {} references item {id} not owned by {owner}",
                    recipe.id
                )));
            }
        }
        for tag in recipe
            .inputs
            .iter()
            .filter_map(|input| input.tag.as_ref())
            .chain(
                recipe
                    .inputs
                    .iter()
                    .flat_map(|input| &input.alternatives)
                    .filter_map(|alternative| match alternative {
                        mc_data::recipes::work::WorkRecipeAlternative::Tag(id) => Some(id),
                        mc_data::recipes::work::WorkRecipeAlternative::Item(_) => None,
                    }),
            )
        {
            if !tags
                .registries
                .get(&item_registry)
                .is_some_and(|registered| registered.contains_key(tag))
            {
                return Err(invalid(format!(
                    "recipe {} references unregistered item tag {tag}",
                    recipe.id
                )));
            }
        }
        recipes.push(recipe);
    }
    Ok(recipes)
}

fn convert_item_definition(
    plugin_id: &str,
    item: &ItemDefinition,
) -> Result<CustomItemDefinition, ContributionRefusal> {
    let id = Identifier::parse(item.id.clone())
        .map_err(|error| ContributionRefusal::InvalidItem(error.to_string()))?;
    let carrier = Identifier::parse(item.carrier.clone())
        .map_err(|error| ContributionRefusal::InvalidItem(error.to_string()))?;
    if id.namespace() != plugin_id || carrier.namespace() != "minecraft" {
        return Err(ContributionRefusal::InvalidItem(format!(
            "{} must be owned by {plugin_id} and use a vanilla carrier",
            item.id
        )));
    }
    let crafting_ingredient = item
        .crafting_ingredient
        .as_ref()
        .map(|ingredient| {
            let id = Identifier::parse(ingredient.clone())
                .map_err(|error| ContributionRefusal::InvalidItem(error.to_string()))?;
            if id.namespace() != "minecraft" && id.namespace() != plugin_id {
                return Err(ContributionRefusal::InvalidItem(format!(
                    "recipe ingredient {id} is not owned by {plugin_id}"
                )));
            }
            Ok(id)
        })
        .transpose()?;
    let physical_profile = item
        .physical_profile
        .as_deref()
        .map(|json| {
            if json.len() > 65_536 {
                return Err(ContributionRefusal::InvalidItem(
                    "physical profile exceeds 64 KiB".to_owned(),
                ));
            }
            serde_json::from_str::<mc_data::item_components::PhysicalItemProfile>(json)
                .map(Box::new)
                .map_err(|error| ContributionRefusal::InvalidItem(error.to_string()))
        })
        .transpose()?;
    Ok(CustomItemDefinition {
        id,
        carrier,
        name: item.name.clone(),
        crafting_ingredient,
        facts: ItemFacts {
            max_stack_size: Some(item.max_stack_size),
            max_damage: item.max_damage,
            weapon: item.weapon,
            attack_damage_modifier: item.attack_damage_modifier,
            attack_speed_modifier: item.attack_speed_modifier,
            equippable_slot: item.equippable_slot.clone(),
            physical_profile,
            ..ItemFacts::default()
        },
    })
}

/// One `spawn-rules` record as the contract's per-biome spawn groups.
fn convert_spawn_rules(rules: &SpawnRules) -> Result<BiomeSpawns, ContributionRefusal> {
    let mut groups = BTreeMap::new();
    for group in &rules.groups {
        let entries = convert_spawn_group(group);
        if groups.insert(group.group.clone(), entries).is_some() {
            // Refused rather than overwritten: see `DuplicateSpawnGroup`.
            return Err(ContributionRefusal::DuplicateSpawnGroup {
                biome: rules.biome.clone(),
                group: group.group.clone(),
            });
        }
    }
    Ok(BiomeSpawns::new(rules.biome.clone(), groups))
}

/// One `spawn-group` record's entries.
///
/// `spawn-entry` and the contract's entry have the same names and the same
/// widths: `min`, `max` and `weight` are `u32` on both sides, so this conversion
/// has no field to narrow and no value to refuse. Their bounds are `validate`'s
/// (`InvalidSpawnEntry`).
fn convert_spawn_group(group: &SpawnGroup) -> Vec<SpawnEntry> {
    group
        .entries
        .iter()
        .map(|entry| SpawnEntry::new(entry.entity.clone(), entry.min, entry.max, entry.weight))
        .collect()
}

/// One `tree-rules` record as the contract's tree declaration.
fn convert_tree_rules(rule: &TreeRules) -> Result<TreeRule, ContributionRefusal> {
    // `spacing` is `u32` in WIT and `u64` in the contract, so every declared
    // value fits its field exactly: there is no width for this one to overflow.
    Ok(TreeRule::new(
        rule.biomes.clone(),
        u64::from(rule.spacing),
        rule.density_threshold,
    ))
}

/// One `clay-rules` record as the contract's clay declaration.
fn convert_clay_rules(rule: &ClayRules) -> Result<ClayRule, ContributionRefusal> {
    Ok(ClayRule::new(
        // `rarity` is `u32` in WIT and `u64` in the contract: exact, never
        // truncated, and no value can overflow it.
        u64::from(rule.rarity),
        narrow_to_u8("clay-rules.radius-min", rule.radius_min)?,
        narrow_to_u8("clay-rules.radius-max", rule.radius_max)?,
        narrow_to_u8("clay-rules.max-water-depth", rule.max_water_depth)?,
    ))
}

/// One `placement-rules` record as the contract's spawn placement.
fn convert_placement_rules(rule: &PlacementRules) -> Result<SpawnPlacement, ContributionRefusal> {
    Ok(SpawnPlacement::new(
        narrow_to_u8("placement-rules.land-spacing", rule.land_spacing)?,
        narrow_to_u8("placement-rules.water-attempts", rule.water_attempts)?,
        narrow_to_u8("placement-rules.water-depth", rule.water_depth)?,
    ))
}

/// Narrow one `u32` contract value to the `u8` its startup field holds.
///
/// A value above `u8::MAX` is refused as [`FieldOverflow`]: the bounds these
/// fields carry are single bytes in the startup contract, and `300` truncated to
/// `44` would be a different rule than the package declared.
fn narrow_to_u8(field: &'static str, value: u32) -> Result<u8, FieldOverflow> {
    u8::try_from(value).map_err(|_| FieldOverflow {
        field,
        value: u64::from(value),
        max: u64::from(u8::MAX),
    })
}

/// What one package's `configure` contributed to startup.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ContributionOutcome {
    /// The package answered no contribution: a legitimate package, not a refusal.
    NoContribution,
    /// The validated categories, including server-owned item identities.
    Configured(ValidatedStartupContribution),
    /// The contribution was refused and no caller may materialize it.
    Refused(ContributionRefusal),
}

/// One package of a deployment and what it contributed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PackageContribution {
    id: String,
    outcome: ContributionOutcome,
}

impl PackageContribution {
    /// The plugin id the manifest declared.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What its `configure` contributed.
    #[must_use]
    pub fn outcome(&self) -> &ContributionOutcome {
        &self.outcome
    }

    /// The validated worldgen rules, if this package declared them.
    #[must_use]
    pub fn rules(&self) -> Option<&GameplayRules> {
        match &self.outcome {
            ContributionOutcome::Configured(content) => content.rules(),
            ContributionOutcome::NoContribution | ContributionOutcome::Refused(_) => None,
        }
    }

    #[must_use]
    pub fn items(&self) -> &[CustomItemDefinition] {
        match &self.outcome {
            ContributionOutcome::Configured(content) => content.items(),
            ContributionOutcome::NoContribution | ContributionOutcome::Refused(_) => &[],
        }
    }

    #[must_use]
    pub fn work_recipes(&self) -> &[mc_data::recipes::work::WorkRecipe] {
        match &self.outcome {
            ContributionOutcome::Configured(content) => content.work_recipes(),
            ContributionOutcome::NoContribution | ContributionOutcome::Refused(_) => &[],
        }
    }

    #[must_use]
    pub fn crafting_recipes(&self) -> &[mc_data::recipes::Recipe] {
        match &self.outcome {
            ContributionOutcome::Configured(content) => content.crafting_recipes(),
            ContributionOutcome::NoContribution | ContributionOutcome::Refused(_) => &[],
        }
    }

    #[must_use]
    pub fn block_tools(&self) -> &[mc_data::block_tools::BlockToolRule] {
        match &self.outcome {
            ContributionOutcome::Configured(content) => content.block_tools(),
            ContributionOutcome::NoContribution | ContributionOutcome::Refused(_) => &[],
        }
    }

    /// The refusal, when the package declared a contribution the contract will not
    /// take.
    #[must_use]
    pub fn refusal(&self) -> Option<&ContributionRefusal> {
        match &self.outcome {
            ContributionOutcome::Refused(refusal) => Some(refusal),
            ContributionOutcome::NoContribution | ContributionOutcome::Configured(_) => None,
        }
    }
}

/// What every package of a started deployment contributed to startup.
///
/// The composition root's input, not its policy: it answers per package, and the
/// caller decides that a refused contribution fails startup while a package that
/// declared none is ordinary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeploymentContribution {
    packages: Vec<PackageContribution>,
}

impl DeploymentContribution {
    /// Every package, in the order the deployment started them.
    #[must_use]
    pub fn packages(&self) -> &[PackageContribution] {
        &self.packages
    }

    /// The first refused contribution and the package that declared it, if any.
    ///
    /// A caller that must fail startup closed reads this one value; the rest of
    /// the deployment's answers are on [`DeploymentContribution::packages`].
    #[must_use]
    pub fn refusal(&self) -> Option<(&str, &ContributionRefusal)> {
        self.packages
            .iter()
            .find_map(|package| package.refusal().map(|refusal| (package.id(), refusal)))
    }

    /// The validated rules of every package that contributed them.
    ///
    /// More than one is a world-contract conflict the caller has to settle: one
    /// world is opened against one rule set, so two owners cannot both win.
    pub fn rules(&self) -> impl Iterator<Item = (&str, &GameplayRules)> {
        self.packages
            .iter()
            .filter_map(|package| package.rules().map(|rules| (package.id(), rules)))
    }

    pub fn items(&self) -> impl Iterator<Item = (&str, &CustomItemDefinition)> {
        self.packages
            .iter()
            .flat_map(|package| package.items().iter().map(move |item| (package.id(), item)))
    }

    pub fn work_recipes(
        &self,
    ) -> impl Iterator<Item = (&str, &mc_data::recipes::work::WorkRecipe)> {
        self.packages.iter().flat_map(|package| {
            package
                .work_recipes()
                .iter()
                .map(move |recipe| (package.id(), recipe))
        })
    }

    pub fn crafting_recipes(&self) -> impl Iterator<Item = &mc_data::recipes::Recipe> {
        self.packages
            .iter()
            .flat_map(|package| package.crafting_recipes())
    }

    pub fn block_tools(&self) -> impl Iterator<Item = &mc_data::block_tools::BlockToolRule> {
        self.packages
            .iter()
            .flat_map(|package| package.block_tools())
    }

    /// Record what one package's `configure` answered.
    ///
    /// Called by the host on the same answer it already has, so the recorded
    /// contribution is the run path's own, not a second run of the phase.
    pub(crate) fn record(&mut self, id: &str, contribution: Option<&StartupContribution>) {
        let outcome = match contribution {
            None => ContributionOutcome::NoContribution,
            Some(contribution) => match convert_startup_contribution(id, contribution) {
                Ok(content) => ContributionOutcome::Configured(content),
                Err(refusal) => ContributionOutcome::Refused(refusal),
            },
        };
        self.packages.push(PackageContribution {
            id: id.to_owned(),
            outcome,
        });
    }
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
