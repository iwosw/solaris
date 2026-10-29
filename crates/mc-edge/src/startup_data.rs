//! Core-independent vanilla content validation shared by import, verify and startup.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Exercise the same core-independent loaders before import publication,
/// operator verification, or world/network startup can accept this cache.
pub fn validate_startup_data(root: &Path) -> Result<()> {
    let data = load_effective_protocol_data(root)?;
    let blocks = mc_data::blocks::solaris_required_blocks_report();
    load_effective_block_light(root, &blocks)?;
    load_effective_block_mining(root, &blocks)?;
    load_effective_block_explosion(root)?;
    let items = mc_data::items::solaris_required_items();
    let item_facts = load_effective_item_facts(root)?;
    load_effective_tags(root, &data, &items)?;
    let recipes = load_effective_recipes(root)?;
    validate_recipe_result_stacks(&recipes, &item_facts)?;
    load_effective_loot(root)?;
    mc_data::advancements::AdvancementTree::load(root)
        .context("loading vanilla advancement data")?;
    Ok(())
}

pub fn load_effective_protocol_data(vanilla_data_dir: &Path) -> Result<mc_data::VanillaData> {
    validate_vanilla_sidecar_version(vanilla_data_dir)?;
    let data = mc_data::load(vanilla_data_dir).with_context(|| {
        format!(
            "loading vanilla registry data from {}",
            vanilla_data_dir.display()
        )
    })?;
    anyhow::ensure!(
        data.has_full_registry_payloads(),
        "vanilla content cache at {} is missing exact RegistryData network payloads; \
         import a complete cache with `{}`",
        vanilla_data_dir.display(),
        crate::content_cache::IMPORT_COMMAND,
    );
    Ok(data)
}

pub fn load_effective_tags(
    vanilla_data_dir: &Path,
    data: &mc_data::VanillaData,
    items: &mc_data::items::ItemRegistry,
) -> Result<mc_data::tags::TagsData> {
    let tags = mc_data::tags::load(vanilla_data_dir, data)
        .with_context(|| format!("loading vanilla tags from {}", vanilla_data_dir.display()))?
        .with_vanilla_fuel_values(items);
    if tags.total_tags() == 0 {
        bail!(
            "vanilla tags from {} were empty; import a complete content cache with `solaris-v2 content import`",
            vanilla_data_dir.display()
        );
    }
    for registry in ["minecraft:block", "minecraft:item", "minecraft:entity_type"] {
        let registry_id = mc_data::Identifier::parse(registry).expect("static registry id");
        let missing = match tags.registries.get(&registry_id) {
            Some(entries) => !entries.values().any(|ids| !ids.is_empty()),
            None => true,
        };
        if missing {
            bail!(
                "vanilla tags from {} missing required resolved entries for tag registry {registry}",
                vanilla_data_dir.display()
            );
        }
    }
    if !tags.fuel_values().matches_default_vanilla_26_1_2(items) {
        bail!(
            "vanilla tags from {} resolved {} furnace fuels instead of the canonical 26.1.2 default set; reimport the content cache",
            vanilla_data_dir.display(),
            tags.fuel_values().fuel_count(),
        );
    }
    validate_payload_tag_references(vanilla_data_dir, data, &tags)?;
    Ok(tags)
}

/// Every `#namespace:path` tag a captured registry payload names must exist in
/// the published tag set. A client binds those references while loading its
/// registries (even from its own known pack) and disconnects in Configuration
/// on a missing tag, so a cache that lacks one is rejected here.
fn validate_payload_tag_references(
    vanilla_data_dir: &Path,
    data: &mc_data::VanillaData,
    tags: &mc_data::tags::TagsData,
) -> Result<()> {
    fn visit(tag: &mc_nbt::Tag, out: &mut Vec<mc_data::Identifier>) {
        match tag {
            mc_nbt::Tag::String(text) => {
                if let Some(reference) = text.strip_prefix('#')
                    && reference.contains(':')
                    && let Ok(id) = mc_data::Identifier::parse(reference)
                {
                    out.push(id);
                }
            }
            mc_nbt::Tag::List(list) => list.elements.iter().for_each(|tag| visit(tag, out)),
            mc_nbt::Tag::Compound(fields) => fields.iter().for_each(|(_, tag)| visit(tag, out)),
            _ => {}
        }
    }
    let mut missing = std::collections::BTreeSet::new();
    for registry in data.registries() {
        for entry in &registry.entries {
            let Some(payload) = data.registry_entry_payload(&registry.id, entry) else {
                continue;
            };
            let root = mc_nbt::read_network_value(&mut payload.as_ref())
                .with_context(|| format!("decoding the {} payload of {entry}", registry.id))?;
            let mut references = Vec::new();
            visit(&root, &mut references);
            for reference in references {
                if !tags
                    .registries
                    .values()
                    .any(|tags| tags.contains_key(&reference))
                {
                    missing.insert(format!("#{reference} (in {} {entry})", registry.id));
                }
            }
        }
    }
    if !missing.is_empty() {
        bail!(
            "vanilla tags from {} lack {} tag(s) the captured registry payloads reference; a client \
             disconnects in Configuration: {}",
            vanilla_data_dir.display(),
            missing.len(),
            missing.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    Ok(())
}

pub fn load_effective_loot(vanilla_data_dir: &Path) -> Result<mc_data::loot::LootTables> {
    let root = vanilla_data_dir
        .join("data")
        .join("minecraft")
        .join("loot_table");
    let mut tables = mc_data::loot::load_vanilla_subset(&root)
        .with_context(|| format!("loading vanilla loot tables from {}", root.display()))?;
    if tables.total_drops() > 0 {
        let builtin = mc_data::loot::load_builtin(vanilla_data_dir)
            .with_context(|| format!("loading builtin loot from {}", vanilla_data_dir.display()))?;
        tables.fill_missing_from(&builtin);
        return Ok(tables);
    }
    bail!(
        "vanilla loot tables from {} had no supported simple drops; import a complete content cache with `solaris-v2 content import`",
        root.display()
    );
}

pub(crate) fn validate_recipe_result_stacks(
    recipes: &[mc_data::recipes::Recipe],
    item_facts: &mc_data::item_components::ItemFactsTable,
) -> Result<()> {
    for recipe in recipes {
        let Some(max_stack_size) = item_facts
            .get(&recipe.result.item)
            .and_then(|facts| facts.max_stack_size)
        else {
            continue;
        };
        if recipe.result.count > max_stack_size {
            bail!(
                "recipe {} produces {} x {}, exceeding the item's max stack size {}",
                recipe.id,
                recipe.result.count,
                recipe.result.item,
                max_stack_size
            );
        }
    }
    Ok(())
}

pub fn load_effective_recipes(vanilla_data_dir: &Path) -> Result<Vec<mc_data::recipes::Recipe>> {
    let root = vanilla_data_dir
        .join("data")
        .join("minecraft")
        .join("recipe");
    let recipes_from_cache = mc_data::recipes::load_recipes(&root)
        .with_context(|| format!("loading vanilla recipes from {}", root.display()))?;
    if recipes_from_cache.is_empty() {
        bail!(
            "vanilla recipes from {} had no supported recipes; import a complete content cache with `solaris-v2 content import`",
            root.display()
        );
    }
    let mut cache_by_id: BTreeMap<_, _> = recipes_from_cache
        .into_iter()
        .map(|recipe| (recipe.id.clone(), recipe))
        .collect();
    let embedded = mc_data::recipes::solaris_required_recipes();
    let mut recipes = Vec::with_capacity(embedded.len() + cache_by_id.len());
    for fallback in embedded {
        let recipe = cache_by_id.remove(&fallback.id).unwrap_or(fallback);
        recipes.push(recipe);
    }
    recipes.extend(cache_by_id.into_values());
    Ok(recipes)
}

pub fn load_effective_block_explosion(
    vanilla_data_dir: &Path,
) -> Result<mc_data::block_explosion::BlockExplosionTable> {
    let path = vanilla_data_dir
        .join("reports")
        .join("block_explosion.json");
    mc_data::block_explosion::load_block_explosion_report(&path).with_context(|| {
        format!(
            "loading vanilla block-explosion table from {}",
            path.display()
        )
    })
}

pub fn load_effective_block_mining(
    vanilla_data_dir: &Path,
    blocks_report: &[mc_data::blocks::BlockReport],
) -> Result<mc_data::block_mining::BlockMiningTable> {
    let path = vanilla_data_dir.join("reports").join("block_mining.json");
    let table = mc_data::block_mining::load(&path)
        .with_context(|| format!("loading vanilla block-mining table from {}", path.display()))?;
    if let Some(max_state_id) = blocks_report
        .iter()
        .flat_map(|block| block.states.iter().map(|state| state.id as usize))
        .max()
        && table.len() <= max_state_id
    {
        bail!(
            "vanilla block-mining table from {} has {} states but blocks report requires state id {max_state_id}",
            path.display(),
            table.len()
        );
    }
    if table.version != mc_protocol::TARGET_RELEASE {
        bail!(
            "vanilla block-mining table from {} targets {} but Solaris targets {}",
            path.display(),
            table.version,
            mc_protocol::TARGET_RELEASE
        );
    }
    Ok(table)
}

pub fn load_effective_item_facts(
    vanilla_data_dir: &Path,
) -> Result<mc_data::item_components::ItemFactsTable> {
    let path = vanilla_data_dir
        .join("reports")
        .join("minecraft")
        .join("components")
        .join("item");
    let table = mc_data::item_components::load_item_facts(&path).with_context(|| {
        format!(
            "loading vanilla item component facts from {}",
            path.display()
        )
    })?;
    if table.is_empty() {
        bail!(
            "vanilla item component facts from {} were empty; reimport the content cache",
            path.display()
        );
    }
    Ok(table)
}

pub fn load_effective_block_light(
    vanilla_data_dir: &Path,
    blocks_report: &[mc_data::blocks::BlockReport],
) -> Result<mc_data::block_light::BlockLightTable> {
    let path = vanilla_data_dir.join("reports").join("block_light.json");
    let table = mc_data::block_light::load(&path)
        .with_context(|| format!("loading vanilla block-light table from {}", path.display()))?;
    if let Some(max_state_id) = blocks_report
        .iter()
        .flat_map(|block| block.states.iter().map(|state| state.id as usize))
        .max()
        && table.len() <= max_state_id
    {
        bail!(
            "vanilla block-light table from {} has {} states but blocks report requires state id {max_state_id}",
            path.display(),
            table.len()
        );
    }
    if table.version != mc_protocol::TARGET_RELEASE {
        bail!(
            "vanilla block-light table from {} targets {} but Solaris targets {}",
            path.display(),
            table.version,
            mc_protocol::TARGET_RELEASE
        );
    }
    Ok(table)
}

#[derive(serde::Deserialize)]
struct VanillaVersionMetadata {
    id: String,
    world_version: u32,
    protocol_version: i32,
}

pub fn validate_vanilla_sidecar_version(vanilla_data_dir: &Path) -> Result<()> {
    let metadata = std::fs::metadata(vanilla_data_dir).with_context(|| {
        format!(
            "reading vanilla sidecar directory metadata for {}",
            vanilla_data_dir.display()
        )
    })?;
    if !metadata.is_dir() {
        bail!(
            "data.vanilla_data_dir is not a directory: {}",
            vanilla_data_dir.display()
        );
    }

    let path = vanilla_data_dir.join("version.json");
    let raw = std::fs::read(&path)
        .with_context(|| format!("reading vanilla sidecar version from {}", path.display()))?;
    let version = serde_json::from_slice::<VanillaVersionMetadata>(&raw)
        .with_context(|| format!("parsing vanilla sidecar version from {}", path.display()))?;

    if version.id != mc_protocol::TARGET_RELEASE {
        bail!(
            "vanilla sidecar release id {:?} does not match Solaris target {:?}",
            version.id,
            mc_protocol::TARGET_RELEASE
        );
    }
    if version.world_version != mc_protocol::WORLD_VERSION {
        bail!(
            "vanilla sidecar world_version {} does not match Solaris world version {}",
            version.world_version,
            mc_protocol::WORLD_VERSION
        );
    }
    if version.protocol_version != mc_protocol::PROTOCOL_VERSION {
        bail!(
            "vanilla sidecar protocol_version {} does not match Solaris protocol version {}",
            version.protocol_version,
            mc_protocol::PROTOCOL_VERSION
        );
    }

    let path = vanilla_data_dir.join(crate::content_import::GAMEPLAY_VERSION_FILE);
    let raw = std::fs::read(&path).with_context(|| {
        format!(
            "reading the gameplay data-pack version from {}",
            path.display()
        )
    })?;
    let gameplay = serde_json::from_slice::<VanillaVersionMetadata>(&raw).with_context(|| {
        format!(
            "parsing the gameplay data-pack version from {}",
            path.display()
        )
    })?;
    if gameplay.id != crate::content_import::GAMEPLAY_DATAPACK_RELEASE {
        bail!(
            "gameplay data pack release {:?} does not match Solaris's {:?}",
            gameplay.id,
            crate::content_import::GAMEPLAY_DATAPACK_RELEASE
        );
    }

    Ok(())
}

#[cfg(test)]
#[path = "startup_data_tests.rs"]
mod tests;
