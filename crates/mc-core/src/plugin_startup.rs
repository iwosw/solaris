//! Materialize the running component host's startup answer before world admission.
//!
//! `apply` runs while the registry is exclusively owned, before opening native
//! storage. `apply_worldgen` runs after loading the vanilla router but before
//! locating spawn or generating chunks. Refusals are startup errors, not warnings;
//! callers must discard the registry/generator on error. Package identity never
//! selects gameplay policy here. The network consumes `plugin_client_bundles`
//! through its existing Loader artifact-index/manifest validator before binding.

use std::collections::BTreeMap;

use mc_data::Identifier;
use mc_data::biomes::BiomeSpawnEntry;
use mc_data::item_components::CustomItemDefinition;
use mc_data::recipes::{
    Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
};
use mc_plugin_host::{ContributionOutcome, DeploymentContribution};
use mc_script::{
    ClientBundle, ClientContentKind, ClientPermission, GameplayRules, PluginWorldgenOreProfile,
};
use mc_worldgen::{ClayRule, TerrainGenerator, TreeRule};

use crate::blueprint::MobCategory;
use crate::config::CoreError;
use crate::registry::CoreRegistry;

/// Install the host's actual configure answers and discovery-verified artifacts.
///
/// This must precede sharing the registry and all native world admission. A
/// deployment with no configure contribution is valid; a refused or unsupported
/// contribution is not. Loader block identities have no v2 registry authority
/// and are refused; physical item mechanics use the native execution validator.
/// Items must have an owner-authorized client bundle; the network additionally
/// validates exact item/carrier/name artifact identities before admitting clients.
pub(crate) fn apply(
    registry: &mut CoreRegistry,
    contribution: &DeploymentContribution,
    ore_profile: Option<PluginWorldgenOreProfile>,
    client_bundles: &[ClientBundle],
) -> Result<(), CoreError> {
    for package in contribution.packages() {
        validate_outcome(package.id(), package.outcome())?;
    }
    let rules = resolve_rules(contribution.rules())?;
    validate_client_bundles(client_bundles)?;
    validate_ore_profile(ore_profile)?;
    if let Some((owner, rules)) = rules {
        validate_rules(registry, owner, rules)?;
    }
    for (owner, item) in contribution.items() {
        validate_item(owner, item, client_bundles)?;
    }
    // Deployment recipes carry the namespace/validation the host conversion
    // already enforced; identity collisions reject at admission below.

    // S1 native settlement startup data: the package is removed, so the six
    // native items and the expanded native work recipes are the sole
    // authority for their identities. Any deployment contribution under the
    // same identities is rejected below by the existing registry validation
    // (duplicate native work recipe / custom item identity), never merged.
    let native_items =
        crate::settlements::startup::authored_items().map_err(CoreError::Registry)?;
    let native_item_ids: std::collections::BTreeSet<_> =
        native_items.iter().map(|item| item.id.clone()).collect();
    registry.item_facts = std::mem::take(&mut registry.item_facts)
        .with_custom_items(
            native_items
                .into_iter()
                .chain(contribution.items().map(|(_, item)| item.clone())),
            &registry.items,
        )
        .map_err(|error| CoreError::Registry(error.to_string()))?;
    let native_recipes: Vec<mc_data::recipes::work::WorkRecipe> =
        mc_plugin_host::convert_native_work_recipes(
            crate::settlements::startup::OWNER,
            &crate::settlements::startup::authored_recipes(),
            &registry.item_facts,
        )
        .map_err(|error| CoreError::Registry(error.to_string()))?;
    let native_recipe_ids: std::collections::BTreeSet<_> = native_recipes
        .iter()
        .map(|recipe| recipe.id.clone())
        .collect();
    // Admit the complete recipe table once; duplicate native identities fail
    // through the same validator as any other startup collision.
    registry.item_facts = std::mem::take(&mut registry.item_facts)
        .with_work_recipes(
            native_recipes.into_iter().chain(
                contribution
                    .work_recipes()
                    .map(|(_, recipe)| recipe.clone()),
            ),
            &registry.items,
        )
        .map_err(CoreError::Registry)?;
    for (_, item) in contribution.items() {
        if let Some(ingredient) = &item.crafting_ingredient {
            if registry.recipes.iter().any(|recipe| recipe.id == item.id) {
                return Err(CoreError::Registry(format!(
                    "custom item {} conflicts with an existing crafting recipe",
                    item.id
                )));
            }
            registry.recipes.push(Recipe {
                id: item.id.clone(),
                show_notification: true,
                kind: RecipeKind::Shapeless(ShapelessRecipe {
                    ingredients: vec![Ingredient {
                        alternatives: vec![IngredientAlternative::Item(ingredient.clone())],
                    }],
                }),
                result: RecipeResult {
                    item: item.id.clone(),
                    count: 1,
                    stew_effects: Vec::new(),
                },
            });
        }
    }
    for recipe in contribution.crafting_recipes() {
        if registry
            .recipes
            .iter()
            .any(|existing| existing.id == recipe.id)
        {
            return Err(CoreError::Registry(format!(
                "plugin crafting recipe {} conflicts with an existing crafting recipe",
                recipe.id
            )));
        }
        registry.recipes.push(recipe.clone());
        registry.plugin_crafting_recipes.push(recipe.clone());
    }
    registry.block_tools = crate::block_tools::BlockToolTable::admit(
        &registry.blocks,
        contribution.block_tools().cloned().collect(),
    )
    .map_err(CoreError::Registry)?;
    if let Some((_, rules)) = rules {
        apply_spawning(registry, rules)?;
        registry.plugin_rules = Some(rules.clone());
    }
    registry.plugin_ore_profile = ore_profile;
    registry.plugin_client_bundles = client_bundles.to_vec();
    registry.plugin_item_ids = native_item_ids
        .into_iter()
        .chain(contribution.items().map(|(_, item)| item.id.clone()))
        .collect();
    registry.plugin_work_recipe_ids = native_recipe_ids
        .into_iter()
        .chain(
            contribution
                .work_recipes()
                .map(|(_, recipe)| recipe.id.clone()),
        )
        .collect();
    Ok(())
}

/// Exact native startup semantics for the checkpoint-owned admission contract.
///
/// The checkpoint owner persists these bytes and rejects mismatches on reopen.
/// Discovery order, artifact paths and package versions are not world policy.
/// UI-only client artifacts likewise do not change native generation/gameplay;
/// their exact catalog is validated separately by the Loader before binding.
pub(crate) fn world_identity(registry: &CoreRegistry) -> Result<Vec<u8>, CoreError> {
    #[derive(serde::Serialize)]
    struct ItemIdentity<'a> {
        id: &'a Identifier,
        carrier: &'a Identifier,
        name: &'a str,
        crafting_ingredient: Option<&'a Identifier>,
        max_stack_size: Option<u32>,
        max_damage: Option<u32>,
        weapon: bool,
        attack_damage_bits: Option<u32>,
        attack_speed_bits: Option<u32>,
        equippable_slot: Option<&'a str>,
        physical_profile: Option<&'a mc_data::item_components::PhysicalItemProfile>,
    }
    #[derive(serde::Serialize)]
    struct WorldIdentity<'a> {
        schema: u32,
        rules: Option<&'a GameplayRules>,
        items: Vec<ItemIdentity<'a>>,
        work_recipes: Vec<&'a mc_data::recipes::work::WorkRecipe>,
        /// Omitted when empty so worlds without these declarations keep their bytes.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        crafting_recipes: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        block_tools: Vec<&'a mc_data::block_tools::BlockToolRule>,
        ore_profile: &'static str,
    }

    let mut items = registry
        .plugin_items()
        .map(|item| ItemIdentity {
            id: &item.id,
            carrier: &item.carrier,
            name: &item.name,
            crafting_ingredient: item.crafting_ingredient.as_ref(),
            max_stack_size: item.facts.max_stack_size,
            max_damage: item.facts.max_damage,
            weapon: item.facts.weapon,
            attack_damage_bits: item.facts.attack_damage_modifier.map(f32::to_bits),
            attack_speed_bits: item.facts.attack_speed_modifier.map(f32::to_bits),
            equippable_slot: item.facts.equippable_slot.as_deref(),
            physical_profile: item.facts.physical_profile.as_deref(),
        })
        .collect::<Vec<_>>();
    items.sort_unstable_by(|left, right| left.id.cmp(right.id));
    let mut work_recipes = registry
        .plugin_work_recipe_ids
        .iter()
        .map(|id| {
            registry.item_facts.work_recipe(id).ok_or_else(|| {
                CoreError::Registry(format!("admitted native work recipe {id} is missing"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    work_recipes.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    serde_json::to_vec(&WorldIdentity {
        schema: 1,
        rules: registry.plugin_rules.as_ref(),
        items,
        work_recipes,
        crafting_recipes: {
            let mut recipes = registry
                .plugin_crafting_recipes
                .iter()
                .map(|recipe| format!("{recipe:?}"))
                .collect::<Vec<_>>();
            recipes.sort_unstable();
            recipes
        },
        block_tools: registry.block_tools.rules().iter().collect(),
        ore_profile: registry
            .plugin_ore_profile
            .map_or("vanilla", PluginWorldgenOreProfile::contract_name),
    })
    .map_err(|error| CoreError::Registry(format!("serializing native plugin startup: {error}")))
}

/// Apply worldgen declarations to the generator that will actually own chunks.
/// Call after loading vanilla data and before even generating a spawn candidate.
pub(crate) fn apply_worldgen(
    registry: &CoreRegistry,
    mut terrain: TerrainGenerator,
) -> Result<TerrainGenerator, CoreError> {
    match registry.plugin_ore_profile {
        None => {}
        Some(PluginWorldgenOreProfile::RealisticDeposits) => {
            terrain = terrain
                .try_with_realistic_deposits(&registry.blocks)
                .map_err(|error| CoreError::Worldgen(error.to_string()))?;
        }
        Some(_) => return Err(CoreError::Worldgen("unsupported plugin ore profile".into())),
    }
    if let Some(rules) = &registry.plugin_rules {
        for definition in &rules.trees {
            let rule = TreeRule::new(definition.spacing, definition.density_threshold)
                .ok_or_else(|| CoreError::Worldgen("invalid native tree rule".into()))?;
            for biome in &definition.biomes {
                terrain
                    .define_tree_rule(identifier(biome, "tree biome")?, rule)
                    .map_err(|error| CoreError::Worldgen(format!("tree biome {biome}: {error}")))?;
            }
        }
        if let Some(definition) = &rules.clay {
            let rule = ClayRule::new(
                definition.rarity,
                definition.radius_min,
                definition.radius_max,
                definition.max_water_depth,
            )
            .ok_or_else(|| CoreError::Worldgen("invalid native clay rule".into()))?;
            terrain
                .define_clay_rule(rule)
                .map_err(|error| CoreError::Worldgen(error.to_owned()))?;
        }
    }
    Ok(terrain)
}

fn validate_outcome(owner: &str, outcome: &ContributionOutcome) -> Result<(), CoreError> {
    match outcome {
        ContributionOutcome::NoContribution | ContributionOutcome::Configured(_) => Ok(()),
        ContributionOutcome::Refused(refusal) => Err(CoreError::Registry(format!(
            "plugin {owner} configure contribution refused: {refusal}"
        ))),
        _ => Err(CoreError::Registry(format!(
            "plugin {owner} returned an unsupported configure outcome"
        ))),
    }
}

fn resolve_rules<'a>(
    mut declarations: impl Iterator<Item = (&'a str, &'a GameplayRules)>,
) -> Result<Option<(&'a str, &'a GameplayRules)>, CoreError> {
    let first = declarations.next();
    if let Some((second, _)) = declarations.next() {
        return Err(CoreError::Registry(format!(
            "plugins {} and {second} both declare startup gameplay rules",
            first.expect("a second declaration has a first").0
        )));
    }
    Ok(first)
}

fn validate_ore_profile(profile: Option<PluginWorldgenOreProfile>) -> Result<(), CoreError> {
    match profile {
        None | Some(PluginWorldgenOreProfile::RealisticDeposits) => Ok(()),
        Some(_) => Err(CoreError::Worldgen("unsupported plugin ore profile".into())),
    }
}

fn validate_client_bundles(bundles: &[ClientBundle]) -> Result<(), CoreError> {
    for bundle in bundles {
        for content in bundle.content() {
            match content {
                ClientContentKind::Blocks => {
                    return Err(CoreError::Registry(format!(
                        "plugin {} bundle {} declares Loader blocks; native v2 block registration is unsupported",
                        bundle.owner_plugin_id(),
                        bundle.id()
                    )));
                }
                ClientContentKind::Items
                | ClientContentKind::Views
                | ClientContentKind::ViewActions
                | ClientContentKind::Assets
                | ClientContentKind::WorldPreviews
                | ClientContentKind::WorldSelection
                | ClientContentKind::Sounds => {}
                _ => {
                    return Err(CoreError::Registry(
                        "unsupported client bundle content".into(),
                    ));
                }
            }
            if !bundle
                .permissions()
                .contains(&content.required_permission())
            {
                return Err(CoreError::Registry(format!(
                    "plugin {} bundle {} lacks its declared content permission",
                    bundle.owner_plugin_id(),
                    bundle.id()
                )));
            }
        }
    }
    Ok(())
}

fn validate_item(
    owner: &str,
    item: &CustomItemDefinition,
    bundles: &[ClientBundle],
) -> Result<(), CoreError> {
    if item.id.namespace() != owner {
        return Err(CoreError::Registry(format!(
            "plugin {owner} custom item {} belongs to another namespace",
            item.id
        )));
    }
    if let Some(profile) = item.facts.physical_profile.as_deref() {
        crate::physical_attacks::validate_profile(profile).map_err(|error| {
            CoreError::Registry(format!(
                "plugin {owner} custom item {} physical profile: {error}",
                item.id
            ))
        })?;
    }
    if !bundles.iter().any(|bundle| {
        bundle.owner_plugin_id() == owner
            && bundle.content().contains(&ClientContentKind::Items)
            && bundle
                .permissions()
                .contains(&ClientPermission::RegisterItems)
    }) {
        return Err(CoreError::Registry(format!(
            "plugin {owner} custom item {} requires an owner-authorized Items/register_items client bundle",
            item.id
        )));
    }
    Ok(())
}

fn validate_rules(
    registry: &CoreRegistry,
    owner: &str,
    rules: &GameplayRules,
) -> Result<(), CoreError> {
    rules.validate().map_err(|error| {
        CoreError::Registry(format!("plugin {owner} startup gameplay rules: {error}"))
    })?;
    // The v2 spawn lane does not read this policy. Accepting it into the table
    // would silently acknowledge an override that cannot affect native spawning.
    if rules.placement.is_some() {
        return Err(CoreError::Registry(format!(
            "plugin {owner} declares unsupported native v2 spawn placement overrides"
        )));
    }
    for definition in &rules.spawning {
        let biome = identifier(&definition.biome, "spawn biome")?;
        if !registry
            .biome_spawns
            .biomes()
            .any(|registered| registered == &biome)
        {
            return Err(CoreError::Registry(format!(
                "plugin {owner} startup rules reference unavailable biome {biome}"
            )));
        }
        for (group, entries) in &definition.groups {
            for entry in entries {
                let entity = identifier(&entry.entity, "spawn entity")?;
                let blueprint = registry
                    .entity_types
                    .id_of(&entity)
                    .and_then(|id| registry.blueprint(id))
                    .ok_or_else(|| {
                        CoreError::Registry(format!(
                        "plugin {owner} startup rules reference unavailable native entity {entity}"
                    ))
                    })?;
                let supported = matches!(
                    (group.as_str(), blueprint.category),
                    ("creature", MobCategory::Creature)
                        | ("monster", MobCategory::Monster)
                        | ("water_ambient", MobCategory::WaterAmbient)
                        | ("water_creature", MobCategory::WaterCreature)
                );
                if !supported {
                    return Err(CoreError::Registry(format!(
                        "plugin {owner} startup entity {entity} cannot spawn in group {group}"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn apply_spawning(registry: &mut CoreRegistry, rules: &GameplayRules) -> Result<(), CoreError> {
    for definition in &rules.spawning {
        // A biome declaration replaces its groups, including omitted groups.
        // define_biome_spawns extends a table, so explicit empty groups clear the
        // seven existing natural lanes instead of retaining vanilla entries.
        let mut groups: BTreeMap<String, Vec<BiomeSpawnEntry>> = [
            "monster",
            "creature",
            "ambient",
            "axolotls",
            "underground_water_creature",
            "water_creature",
            "water_ambient",
        ]
        .into_iter()
        .map(|group| (group.to_owned(), Vec::new()))
        .collect();
        for (group, entries) in &definition.groups {
            let native = entries
                .iter()
                .map(|entry| {
                    Ok(BiomeSpawnEntry {
                        entity_type: identifier(&entry.entity, "spawn entity")?,
                        min_count: entry.min,
                        max_count: entry.max,
                        weight: entry.weight,
                    })
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            groups.insert(group.clone(), native);
        }
        registry
            .biome_spawns
            .define_biome_spawns(identifier(&definition.biome, "spawn biome")?, groups);
    }
    Ok(())
}

fn identifier(value: &str, field: &str) -> Result<Identifier, CoreError> {
    Identifier::parse(value)
        .map_err(|error| CoreError::Registry(format!("invalid {field} {value:?}: {error}")))
}

#[cfg(test)]
#[path = "plugin_startup_tests.rs"]
mod tests;
