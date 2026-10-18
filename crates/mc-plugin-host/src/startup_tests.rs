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
