//! Recipe displays use core recipe indices; only advancement-reward recipes
//! supplied by the core are unlocked in each player's book.

use mc_data::ItemStack;
use mc_data::identifier::Identifier;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_protocol::packets::play::{
    ClientboundRecipeBookAdd, ClientboundUpdateRecipes, RecipeBookDisplay, RecipeBookEntry,
    RecipeBookIngredient, RecipeBookSlotDisplay, RecipePropertySet, StonecutterRecipeEntry,
};

/// The join burst's recipe packets: `UpdateRecipes` (stonecutter list and
/// campfire admission inputs) and a `RecipeBookAdd` replacing the client's book.
pub struct RecipePackets {
    pub update: ClientboundUpdateRecipes,
    pub book: ClientboundRecipeBookAdd,
    pub advancements: std::sync::Arc<crate::advancements::AdvancementPackets>,
}

impl RecipePackets {
    pub fn new(registry: &mc_core::registry::CoreRegistry) -> anyhow::Result<Self> {
        Ok(Self {
            update: initial_recipe_update(
                &registry.recipes,
                &registry.items,
                &registry.item_facts,
                &registry.tags,
            ),
            book: ClientboundRecipeBookAdd {
                entries: Vec::new(),
                replace: true,
            },
            advancements: std::sync::Arc::new(crate::advancements::AdvancementPackets::new(
                registry,
            )?),
        })
    }
}

fn item_max_stack(item_facts: &ItemFactsTable, items: &ItemRegistry, stack: &ItemStack) -> i32 {
    mc_data::item_semantics_26_1_2::max_stack_for_stack(item_facts, items, stack)
}

pub(crate) fn initial_recipe_update(
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    tags: &mc_data::tags::TagsData,
) -> ClientboundUpdateRecipes {
    let stonecutter_recipes = recipes
        .iter()
        .filter_map(|recipe| stonecutter_recipe_entry(recipe, items, item_facts))
        .collect();
    ClientboundUpdateRecipes {
        item_sets: vec![campfire_inputs(recipes, items, tags)],
        stonecutter_recipes,
    }
}

fn campfire_inputs(
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    tags: &mc_data::tags::TagsData,
) -> RecipePropertySet {
    use mc_data::recipes::{IngredientAlternative, RecipeKind};
    let item_tags = tags
        .registries
        .get(&Identifier::parse("minecraft:item").expect("static id"));
    let mut item_ids = Vec::new();
    for recipe in recipes {
        let RecipeKind::CampfireCooking(cooking) = &recipe.kind else {
            continue;
        };
        for alternative in &cooking.ingredient.alternatives {
            match alternative {
                IngredientAlternative::Item(item) => {
                    if let Some(id) = items.id_of(item).and_then(|id| i32::try_from(id).ok()) {
                        item_ids.push(id);
                    }
                }
                IngredientAlternative::Tag(tag) => {
                    if let Some(ids) = item_tags.and_then(|tags| tags.get(tag)) {
                        item_ids.extend_from_slice(ids);
                    }
                }
            }
        }
    }
    item_ids.sort_unstable();
    item_ids.dedup();
    RecipePropertySet {
        key: Identifier::parse("minecraft:campfire_input").expect("static id"),
        item_ids,
    }
}

pub(crate) fn stonecutter_recipe_entry(
    recipe: &mc_data::recipes::Recipe,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> Option<StonecutterRecipeEntry> {
    let mc_data::recipes::RecipeKind::Stonecutting(stonecutting) = &recipe.kind else {
        return None;
    };
    if recipe.result.item.as_str() == "minecraft:air" {
        return None;
    }
    let result = recipe.result.to_stack(items, item_facts)?;
    if result.count > item_max_stack(item_facts, items, &result) {
        return None;
    }
    Some(StonecutterRecipeEntry {
        input: recipe_book_requirement(&stonecutting.ingredient, items)?,
        result: RecipeBookSlotDisplay::ItemStack(Box::new(result)),
    })
}

pub(crate) fn recipe_book_entry(
    display_id: usize,
    recipe: &mc_data::recipes::Recipe,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> Option<RecipeBookEntry> {
    let display_id = i32::try_from(display_id).ok()?;
    let result =
        RecipeBookSlotDisplay::ItemStack(Box::new(recipe.result.to_stack(items, item_facts)?));

    let (display, category_id, crafting_requirements) = match &recipe.kind {
        mc_data::recipes::RecipeKind::Shapeless(shapeless) => {
            let ingredients = shapeless
                .ingredients
                .iter()
                .map(|ingredient| recipe_book_slot(ingredient, items, item_facts))
                .collect::<Option<Vec<_>>>()?;
            let requirements = recipe_book_requirements(&shapeless.ingredients, items, item_facts);
            let crafting_station = named_recipe_book_item(items, "minecraft:crafting_table")?;
            (
                RecipeBookDisplay::Shapeless {
                    ingredients,
                    result,
                    crafting_station,
                },
                3,
                requirements,
            )
        }
        mc_data::recipes::RecipeKind::Shaped(shaped) => {
            let height = i32::try_from(shaped.pattern.len()).ok()?;
            let width = i32::try_from(shaped.pattern.first()?.chars().count()).ok()?;
            if width <= 0 || height <= 0 {
                return None;
            }
            let mut ingredients = Vec::with_capacity((width * height) as usize);
            let mut requirement_sources = Vec::new();
            for row in &shaped.pattern {
                if i32::try_from(row.chars().count()).ok()? != width {
                    return None;
                }
                for key in row.chars() {
                    if key == ' ' {
                        ingredients.push(RecipeBookSlotDisplay::Empty);
                    } else {
                        let ingredient = shaped.key.get(&key)?;
                        ingredients.push(recipe_book_slot(ingredient, items, item_facts)?);
                        requirement_sources.push(ingredient.clone());
                    }
                }
            }
            let requirements = recipe_book_requirements(&requirement_sources, items, item_facts);
            let crafting_station = named_recipe_book_item(items, "minecraft:crafting_table")?;
            (
                RecipeBookDisplay::Shaped {
                    width,
                    height,
                    ingredients,
                    result,
                    crafting_station,
                },
                3,
                requirements,
            )
        }
        mc_data::recipes::RecipeKind::Smelting(cooking) => (
            recipe_book_cooking_display(cooking, result, items, item_facts, "minecraft:furnace")?,
            6,
            None,
        ),
        mc_data::recipes::RecipeKind::Blasting(cooking) => (
            recipe_book_cooking_display(
                cooking,
                result,
                items,
                item_facts,
                "minecraft:blast_furnace",
            )?,
            8,
            None,
        ),
        mc_data::recipes::RecipeKind::Smoking(cooking) => (
            recipe_book_cooking_display(cooking, result, items, item_facts, "minecraft:smoker")?,
            9,
            None,
        ),
        mc_data::recipes::RecipeKind::CampfireCooking(cooking) => (
            recipe_book_cooking_display(cooking, result, items, item_facts, "minecraft:campfire")?,
            12,
            None,
        ),
        mc_data::recipes::RecipeKind::Stonecutting(_) => return None,
    };

    Some(RecipeBookEntry {
        display_id,
        display,
        group: None,
        category_id,
        crafting_requirements,
        flags: u8::from(recipe.show_notification) | 2,
    })
}

fn named_recipe_book_item(items: &ItemRegistry, name: &str) -> Option<RecipeBookSlotDisplay> {
    let name = Identifier::parse(name).ok()?;
    let item_id = i32::try_from(items.id_of(&name)?).ok()?;
    Some(RecipeBookSlotDisplay::Item { item_id })
}

fn recipe_book_slot(
    ingredient: &mc_data::recipes::Ingredient,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> Option<RecipeBookSlotDisplay> {
    let mut alternatives = ingredient
        .alternatives
        .iter()
        .map(|alternative| match alternative {
            mc_data::recipes::IngredientAlternative::Item(item) => {
                if let Some(custom) = item_facts.custom(item) {
                    let item_id = items.id_of(&custom.carrier)?;
                    Some(RecipeBookSlotDisplay::ItemStack(Box::new(
                        ItemStack::new(item_id, 1)
                            .with_item_model(custom.id.clone())
                            .with_custom_name(&custom.name),
                    )))
                } else {
                    items
                        .id_of(item)
                        .and_then(|item_id| i32::try_from(item_id).ok())
                        .map(|item_id| RecipeBookSlotDisplay::Item { item_id })
                }
            }
            mc_data::recipes::IngredientAlternative::Tag(tag) => {
                Some(RecipeBookSlotDisplay::Tag(tag.clone()))
            }
        })
        .collect::<Option<Vec<_>>>()?;
    match alternatives.len() {
        0 => None,
        1 => alternatives.pop(),
        _ => Some(RecipeBookSlotDisplay::Composite(alternatives)),
    }
}

fn recipe_book_requirements(
    ingredients: &[mc_data::recipes::Ingredient],
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> Option<Vec<RecipeBookIngredient>> {
    ingredients
        .iter()
        .map(|ingredient| {
            if ingredient.alternatives.iter().any(|alternative| {
                matches!(alternative, mc_data::recipes::IngredientAlternative::Item(item) if item_facts.custom(item).is_some())
            }) {
                None
            } else {
                recipe_book_requirement(ingredient, items)
            }
        })
        .collect()
}

fn recipe_book_requirement(
    ingredient: &mc_data::recipes::Ingredient,
    items: &ItemRegistry,
) -> Option<RecipeBookIngredient> {
    if let [mc_data::recipes::IngredientAlternative::Tag(tag)] = ingredient.alternatives.as_slice()
    {
        return Some(RecipeBookIngredient::Tag(tag.clone()));
    }
    let item_ids = ingredient
        .alternatives
        .iter()
        .map(|alternative| match alternative {
            mc_data::recipes::IngredientAlternative::Item(item) => {
                items.id_of(item).and_then(|id| i32::try_from(id).ok())
            }
            mc_data::recipes::IngredientAlternative::Tag(_) => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (!item_ids.is_empty()).then_some(RecipeBookIngredient::Items(item_ids))
}

fn recipe_book_cooking_display(
    recipe: &mc_data::recipes::SmeltingRecipe,
    result: RecipeBookSlotDisplay,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    station: &str,
) -> Option<RecipeBookDisplay> {
    Some(RecipeBookDisplay::Furnace {
        ingredient: recipe_book_slot(&recipe.ingredient, items, item_facts)?,
        fuel: RecipeBookSlotDisplay::AnyFuel,
        result,
        crafting_station: named_recipe_book_item(items, station)?,
        duration: i32::try_from(recipe.cooking_time).ok()?,
        // Recipe JSON loaded by mc-data does not yet retain cooking XP.
        experience: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_protocol::packets::Packet;

    #[test]
    fn join_campfire_property_set_admits_chicken_and_round_trips() {
        let items = mc_data::items::solaris_required_items();
        let facts = mc_data::item_components::solaris_required_item_facts();
        let tags = mc_data::tags::solaris_required_item_tags(&items);
        let recipes = mc_data::recipes::solaris_required_recipes();
        let update = initial_recipe_update(&recipes, &items, &facts, &tags);
        let inputs = update
            .item_sets
            .iter()
            .find(|set| set.key.as_str() == "minecraft:campfire_input")
            .unwrap();
        let item = |name| items.id_of(&Identifier::parse(name).unwrap()).unwrap() as i32;
        assert!(inputs.item_ids.contains(&item("minecraft:chicken")));
        assert!(!inputs.item_ids.contains(&item("minecraft:wheat")));
        let mut body = Vec::new();
        update.encode(&mut body).unwrap();
        assert_eq!(
            ClientboundUpdateRecipes::decode(&mut body.as_slice()).unwrap(),
            update
        );
    }

    #[test]
    fn campfire_property_set_expands_tags_and_deduplicates_alternatives() {
        use mc_data::recipes::{IngredientAlternative, RecipeKind};
        let items = mc_data::items::solaris_required_items();
        let mut tags = mc_data::tags::solaris_required_item_tags(&items);
        let recipes = mc_data::recipes::solaris_required_recipes();
        let chicken = Identifier::parse("minecraft:chicken").unwrap();
        let cod = Identifier::parse("minecraft:cod").unwrap();
        let tag = Identifier::parse("solaris:test_campfire_food").unwrap();
        let chicken_id = items.id_of(&chicken).unwrap() as i32;
        let cod_id = items.id_of(&cod).unwrap() as i32;
        tags.registries
            .entry(Identifier::parse("minecraft:item").unwrap())
            .or_default()
            .insert(tag.clone(), vec![chicken_id, cod_id, cod_id]);
        let mut recipe = recipes
            .iter()
            .find(|recipe| matches!(recipe.kind, RecipeKind::CampfireCooking(_)))
            .unwrap()
            .clone();
        let RecipeKind::CampfireCooking(cooking) = &mut recipe.kind else {
            unreachable!()
        };
        cooking.ingredient.alternatives = vec![
            IngredientAlternative::Item(chicken),
            IngredientAlternative::Tag(tag),
        ];
        let inputs = campfire_inputs(&[recipe], &items, &tags);
        let mut expected = vec![chicken_id, cod_id];
        expected.sort_unstable();
        assert_eq!(inputs.item_ids, expected);
    }
}
