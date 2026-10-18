use std::collections::BTreeMap;
use std::path::Path;

use super::{EntityLootRecipeLookup, EntityLootSmeltingRecipe, EntityLootTagLookup};
use crate::Identifier;
use crate::recipes::{IngredientAlternative, RecipeKind};

pub struct VanillaEntityLootLookups {
    discs: Vec<Identifier>,
    skeletons: Vec<Identifier>,
    smelts_loot: Vec<Identifier>,
    smelting: BTreeMap<Identifier, EntityLootSmeltingRecipe>,
}

/// Build the entity-loot tag and smelting lookups from the startup-selected
/// vanilla content root and the native item/recipe catalogs.
///
/// # Errors
///
/// Returns the exact missing or invalid tag file.
pub fn load_vanilla_lookups(root: &Path) -> Result<VanillaEntityLootLookups, String> {
    let items = crate::items::solaris_required_items();
    let tags = crate::tags::solaris_required_item_tags(&items);
    let facts = crate::item_components::solaris_required_item_facts();
    let mut smelting = BTreeMap::new();
    for recipe in crate::recipes::solaris_required_recipes() {
        let RecipeKind::Smelting(cooking) = recipe.kind else {
            continue;
        };
        let output = EntityLootSmeltingRecipe {
            max_stack_size: facts
                .get(&recipe.result.item)
                .and_then(|facts| facts.max_stack_size)
                .unwrap_or(64),
            output: recipe.result.item,
            output_count: recipe.result.count,
        };
        for ingredient in cooking.ingredient.alternatives {
            match ingredient {
                IngredientAlternative::Item(item) => {
                    smelting.insert(item, output.clone());
                }
                IngredientAlternative::Tag(tag) => {
                    let registry =
                        Identifier::parse("minecraft:item").expect("registry identifier");
                    for raw_id in tags
                        .registries
                        .get(&registry)
                        .and_then(|registry| registry.get(&tag))
                        .into_iter()
                        .flatten()
                    {
                        if let Ok(raw_id) = u32::try_from(*raw_id)
                            && let Some(item) = items.name_of(raw_id)
                        {
                            smelting.insert(item.clone(), output.clone());
                        }
                    }
                }
            }
        }
    }
    Ok(VanillaEntityLootLookups {
        discs: members(
            root,
            "data/minecraft/tags/item/creeper_drop_music_discs.json",
        )?,
        skeletons: members(root, "data/minecraft/tags/entity_type/skeletons.json")?,
        smelts_loot: members(root, "data/minecraft/tags/enchantment/smelts_loot.json")?,
        smelting,
    })
}

fn members(root: &Path, relative: &str) -> Result<Vec<Identifier>, String> {
    #[derive(serde::Deserialize)]
    struct Tag {
        values: Vec<String>,
    }
    let source = crate::runtime_resources::read_json(root, relative)?;
    let path = root.join(relative);
    serde_json::from_str::<Tag>(&source)
        .map_err(|error| format!("parsing loot tag {}: {error}", path.display()))?
        .values
        .into_iter()
        .map(|name| {
            Identifier::parse(name)
                .map_err(|error| format!("loot tag member in {}: {error}", path.display()))
        })
        .collect()
}

impl EntityLootTagLookup for VanillaEntityLootLookups {
    fn item_tag(&self, tag: &Identifier) -> Option<&[Identifier]> {
        (tag.as_str() == "minecraft:creeper_drop_music_discs").then_some(self.discs.as_slice())
    }
    fn entity_type_in_tag(&self, tag: &Identifier, entity_type: &Identifier) -> bool {
        tag.as_str() == "minecraft:skeletons" && self.skeletons.contains(entity_type)
    }
    fn enchantment_in_tag(&self, tag: &Identifier, enchantment: &Identifier) -> bool {
        tag.as_str() == "minecraft:smelts_loot" && self.smelts_loot.contains(enchantment)
    }
}

impl EntityLootRecipeLookup for VanillaEntityLootLookups {
    fn smelting_recipe(&self, input: &Identifier) -> Option<&EntityLootSmeltingRecipe> {
        self.smelting.get(input)
    }
}
