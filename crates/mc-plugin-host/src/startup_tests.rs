use super::{
    ContributionRefusal, ItemDefinition, StartupContribution, convert_startup_contribution,
};

fn contribution(plugin_id: &str) -> StartupContribution {
    StartupContribution {
        spawning: None,
        trees: None,
        clay: None,
        placement: None,
        items: Some(vec![ItemDefinition {
            id: format!("{plugin_id}:token"),
            carrier: "minecraft:paper".to_owned(),
            name: "Token".to_owned(),
            max_stack_size: 16,
            max_damage: None,
            weapon: false,
            attack_damage_modifier: None,
            attack_speed_modifier: None,
            equippable_slot: None,
            crafting_ingredient: None,
            physical_profile: None,
        }]),
        work_recipes: Some(vec![
            serde_json::json!({
                "id": format!("{plugin_id}:token"),
                "revision": "1",
                "station": "minecraft:crafting_table",
                "duration_ticks": 20,
                "inputs": [{"item": null, "tag": "minecraft:logs", "count": 1, "components": null}],
                "outputs": [{"item": format!("{plugin_id}:token"), "count": 1, "components": {
                    "damage": null, "enchantments": [], "custom_name": null, "item_model": null,
                    "stew_effects": [], "max_damage": null, "custom_data": null
                }}],
                "remainders": [],
                "tools": []
            })
            .to_string(),
        ]),
        crafting_recipes: None,
        block_tools: None,
    }
}

#[test]
fn repeated_contributions_keep_required_item_and_tag_validation() {
    for plugin_id in ["first", "second", "first"] {
        let validated = convert_startup_contribution(plugin_id, &contribution(plugin_id))
            .expect("registered carrier, item tag and package-local output");
        assert_eq!(validated.items()[0].carrier.as_str(), "minecraft:paper");
        assert_eq!(
            validated.work_recipes()[0].inputs[0]
                .tag
                .as_ref()
                .unwrap()
                .as_str(),
            "minecraft:logs"
        );
        assert_eq!(
            validated.work_recipes()[0].outputs[0].item.as_str(),
            format!("{plugin_id}:token")
        );
    }
}

#[test]
fn cached_required_data_does_not_retain_custom_items_or_refused_tags() {
    let valid = contribution("first");
    convert_startup_contribution("first", &valid).expect("initial contribution");

    let mut missing_item = valid.clone();
    missing_item.items = None;
    assert!(matches!(
        convert_startup_contribution("first", &missing_item),
        Err(ContributionRefusal::InvalidWorkRecipe(_))
    ));

    let mut unknown_tag = valid.clone();
    let recipe = &mut unknown_tag.work_recipes.as_mut().unwrap()[0];
    *recipe = recipe.replace("minecraft:logs", "minecraft:undeclared");
    assert!(matches!(
        convert_startup_contribution("first", &unknown_tag),
        Err(ContributionRefusal::InvalidWorkRecipe(_))
    ));

    convert_startup_contribution("first", &valid)
        .expect("neither refusal changes the immutable required data");
}

fn chisel_contribution() -> StartupContribution {
    let mut contribution = contribution("chisel");
    contribution.items.as_mut().unwrap()[0].id = "chisel:chisel".to_owned();
    contribution.items.as_mut().unwrap()[0].carrier = "minecraft:stick".to_owned();
    contribution.work_recipes = None;
    contribution.crafting_recipes = Some(vec![
        r#"{"type":"minecraft:crafting_shaped","pattern":["I","S"],
            "key":{"I":"minecraft:iron_ingot","S":"minecraft:stick"},
            "result":{"id":"chisel:chisel","count":1}}"#
            .to_owned(),
    ]);
    contribution.block_tools = Some(vec![
        r#"{"item":"chisel:chisel","block":"minecraft:stone_bricks","face":"up",
            "result":"minecraft:stone_brick_slab","set":{"type":"top"},
            "give":"minecraft:stone_brick_slab"}"#
            .to_owned(),
    ]);
    contribution
}

#[test]
fn crafting_recipes_and_block_tools_are_validated_against_declared_items() {
    let validated = convert_startup_contribution("chisel", &chisel_contribution())
        .expect("owned recipe and tool over a declared item");
    assert_eq!(validated.crafting_recipes()[0].id.as_str(), "chisel:chisel");
    assert_eq!(validated.block_tools()[0].wear, 1);

    let mut foreign_result = chisel_contribution();
    foreign_result.crafting_recipes.as_mut().unwrap()[0] =
        foreign_result.crafting_recipes.as_ref().unwrap()[0]
            .replace("chisel:chisel", "chisel:other");
    assert!(matches!(
        convert_startup_contribution("chisel", &foreign_result),
        Err(ContributionRefusal::InvalidCraftingRecipe(_))
    ));

    let mut unknown_ingredient = chisel_contribution();
    unknown_ingredient.crafting_recipes.as_mut().unwrap()[0] =
        unknown_ingredient.crafting_recipes.as_ref().unwrap()[0]
            .replace("minecraft:iron_ingot", "minecraft:not_an_item");
    assert!(matches!(
        convert_startup_contribution("chisel", &unknown_ingredient),
        Err(ContributionRefusal::InvalidCraftingRecipe(_))
    ));

    let mut undeclared_tool = chisel_contribution();
    undeclared_tool.block_tools.as_mut().unwrap()[0] =
        undeclared_tool.block_tools.as_ref().unwrap()[0].replace("chisel:chisel", "chisel:other");
    assert!(matches!(
        convert_startup_contribution("chisel", &undeclared_tool),
        Err(ContributionRefusal::InvalidBlockTool(_))
    ));

    let mut unknown_grant = chisel_contribution();
    unknown_grant.block_tools.as_mut().unwrap()[0] = unknown_grant.block_tools.as_ref().unwrap()[0]
        .replace("minecraft:stone_brick_slab\"}", "minecraft:not_an_item\"}");
    assert!(matches!(
        convert_startup_contribution("chisel", &unknown_grant),
        Err(ContributionRefusal::InvalidBlockTool(_))
    ));
}
