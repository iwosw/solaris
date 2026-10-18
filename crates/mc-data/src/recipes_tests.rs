use super::*;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn parse_test_recipe(value: serde_json::Value) -> Result<Option<Recipe>, RecipeDataError> {
    parse_recipe_value(
        Path::new("test.json"),
        Identifier::parse("minecraft:test").unwrap(),
        value,
    )
}

#[test]
fn shaped_recipe_enforces_vanilla_grid_and_character_width() {
    let unicode = parse_test_recipe(serde_json::json!({
        "type": "minecraft:crafting_shaped",
        "pattern": ["界界"],
        "key": {"界": "minecraft:stone"},
        "result": {"id": "minecraft:stone"}
    }))
    .unwrap();
    assert!(unicode.is_some());

    for pattern in [
        serde_json::json!(["####"]),
        serde_json::json!(["#", "#", "#", "#"]),
        serde_json::json!(["   "]),
    ] {
        let error = parse_test_recipe(serde_json::json!({
            "type": "minecraft:crafting_shaped",
            "pattern": pattern,
            "key": {"#": "minecraft:stone"},
            "result": {"id": "minecraft:stone"}
        }))
        .unwrap_err();
        assert!(matches!(error, RecipeDataError::InvalidPattern { .. }));
    }
}

#[test]
fn shaped_recipe_rejects_unused_declared_key() {
    let error = parse_test_recipe(serde_json::json!({
        "type": "minecraft:crafting_shaped",
        "pattern": ["#"],
        "key": {
            "#": "minecraft:stone",
            "X": "minecraft:dirt"
        },
        "result": {"id": "minecraft:stone"}
    }))
    .unwrap_err();
    assert!(matches!(error, RecipeDataError::UnusedKey { key: 'X', .. }));
}

#[test]
fn shapeless_recipe_requires_one_to_nine_ingredients() {
    let empty = parse_test_recipe(serde_json::json!({
        "type": "minecraft:crafting_shapeless",
        "ingredients": [],
        "result": {"id": "minecraft:stone"}
    }))
    .unwrap_err();
    assert!(matches!(
        empty,
        RecipeDataError::InvalidIngredientCount { count: 0, .. }
    ));

    let too_many = parse_test_recipe(serde_json::json!({
        "type": "minecraft:crafting_shapeless",
        "ingredients": vec!["minecraft:stone"; 10],
        "result": {"id": "minecraft:stone"}
    }))
    .unwrap_err();
    assert!(matches!(
        too_many,
        RecipeDataError::InvalidIngredientCount { count: 10, .. }
    ));
}

#[test]
fn recipes_reject_zero_result_count_and_zero_cooking_time() {
    let zero_result = parse_test_recipe(serde_json::json!({
        "type": "minecraft:crafting_shapeless",
        "ingredients": ["minecraft:stone"],
        "result": {"id": "minecraft:stone", "count": 0}
    }))
    .unwrap_err();
    assert!(matches!(
        zero_result,
        RecipeDataError::InvalidResultCount { count: 0, .. }
    ));

    let zero_time = parse_test_recipe(serde_json::json!({
        "type": "minecraft:smelting",
        "ingredient": "minecraft:iron_ore",
        "result": {"id": "minecraft:iron_ingot"},
        "cooking_time": 0
    }))
    .unwrap_err();
    assert!(matches!(
        zero_time,
        RecipeDataError::InvalidCookingTime { value: 0, .. }
    ));
}

#[test]
fn loads_synthetic_shaped_and_shapeless_recipes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("building_blocks/test_planks.json"),
        r###"{
          "type": "minecraft:crafting_shaped",
          "pattern": ["##", "##"],
          "key": { "#": ["minecraft:oak_log", "minecraft:birch_log"] },
          "result": { "id": "minecraft:test_planks", "count": 4 }
        }"###,
    );
    write(
        &root.join("misc/test_stick.json"),
        r#"{
          "type": "minecraft:crafting_shapeless",
          "ingredients": ["minecraft:test_planks", ["minecraft:oak_planks", "minecraft:birch_planks"]],
          "result": { "id": "minecraft:stick", "count": 4 }
        }"#,
    );
    write(
        &root.join("ignored/test_smelting.json"),
        r#"{
          "type": "minecraft:smelting",
          "ingredient": "minecraft:iron_ore",
          "result": { "id": "minecraft:iron_ingot" }
        }"#,
    );
    write(
        &root.join("ignored/test_blasting.json"),
        r#"{
          "type": "minecraft:blasting",
          "ingredient": "minecraft:raw_iron",
          "result": { "id": "minecraft:iron_ingot" },
          "cooking_time": 100
        }"#,
    );
    write(
        &root.join("ignored/test_smoking.json"),
        r#"{
          "type": "minecraft:smoking",
          "ingredient": "minecraft:beef",
          "result": { "id": "minecraft:cooked_beef" },
          "cooking_time": 100
        }"#,
    );
    write(
        &root.join("ignored/test_campfire.json"),
        r#"{
          "type": "minecraft:campfire_cooking",
          "ingredient": "minecraft:porkchop",
          "result": { "id": "minecraft:cooked_porkchop" },
          "cooking_time": 600
        }"#,
    );

    let recipes = load_recipes(root).unwrap();

    assert_eq!(recipes.len(), 6);
    assert_eq!(
        recipes[0].id.as_str(),
        "minecraft:building_blocks/test_planks"
    );
    assert_eq!(recipes[0].result.item.as_str(), "minecraft:test_planks");
    assert_eq!(recipes[0].result.count, 4);
    let RecipeKind::Shaped(shaped) = &recipes[0].kind else {
        panic!("expected shaped recipe");
    };
    assert_eq!(shaped.pattern, ["##", "##"]);
    assert_eq!(
        shaped.key[&'#']
            .alternatives
            .iter()
            .map(|alternative| match alternative {
                IngredientAlternative::Item(id) => id.as_str(),
                IngredientAlternative::Tag(id) => id.as_str(),
            })
            .collect::<Vec<_>>(),
        ["minecraft:oak_log", "minecraft:birch_log"]
    );

    let stick = recipes
        .iter()
        .find(|recipe| recipe.id.as_str() == "minecraft:misc/test_stick")
        .unwrap();
    let RecipeKind::Shapeless(shapeless) = &stick.kind else {
        panic!("expected shapeless recipe");
    };
    assert_eq!(shapeless.ingredients.len(), 2);
    assert_eq!(
        shapeless.ingredients[0].alternatives[0],
        IngredientAlternative::Item(Identifier::parse("minecraft:test_planks").unwrap())
    );
    assert_eq!(
        shapeless.ingredients[1].alternatives[0],
        IngredientAlternative::Item(Identifier::parse("minecraft:oak_planks").unwrap())
    );
    assert_eq!(stick.result.item.as_str(), "minecraft:stick");

    let smelting_recipe = recipes
        .iter()
        .find(|recipe| recipe.id.as_str() == "minecraft:ignored/test_smelting")
        .unwrap();
    let RecipeKind::Smelting(smelting) = &smelting_recipe.kind else {
        panic!("expected smelting recipe");
    };
    assert_eq!(smelting.cooking_time, 200);
    assert_eq!(
        smelting.ingredient.alternatives[0],
        IngredientAlternative::Item(Identifier::parse("minecraft:iron_ore").unwrap())
    );
    assert_eq!(smelting_recipe.result.item.as_str(), "minecraft:iron_ingot");

    assert!(recipes.iter().any(|recipe| matches!(
        (&recipe.kind, recipe.id.as_str()),
        (RecipeKind::Blasting(smelting), "minecraft:ignored/test_blasting")
            if smelting.cooking_time == 100
    )));
    assert!(recipes.iter().any(|recipe| matches!(
        (&recipe.kind, recipe.id.as_str()),
        (RecipeKind::Smoking(smelting), "minecraft:ignored/test_smoking")
            if smelting.cooking_time == 100
    )));
    assert!(recipes.iter().any(|recipe| matches!(
        (&recipe.kind, recipe.id.as_str()),
        (RecipeKind::CampfireCooking(smelting), "minecraft:ignored/test_campfire")
            if smelting.cooking_time == 600
    )));
}

#[test]
fn loads_simple_stonecutting_recipe_and_skips_component_results() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("building_blocks/test_slab.json"),
        r#"{
          "type": "minecraft:stonecutting",
          "ingredient": "minecraft:cobblestone",
          "result": { "id": "minecraft:cobblestone_slab", "count": 2 }
        }"#,
    );
    write(
        &root.join("building_blocks/unsupported_components.json"),
        r#"{
          "type": "minecraft:stonecutting",
          "ingredient": "minecraft:stone",
          "result": {
            "id": "minecraft:stone_slab",
            "components": { "minecraft:custom_name": "unsupported" }
          }
        }"#,
    );

    let recipes = load_recipes(root).unwrap();

    assert_eq!(recipes.len(), 1);
    assert_eq!(
        recipes[0].id.as_str(),
        "minecraft:building_blocks/test_slab"
    );
    assert_eq!(
        recipes[0].kind,
        RecipeKind::Stonecutting(StonecuttingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(
                    Identifier::parse("minecraft:cobblestone").unwrap(),
                )],
            },
        })
    );
    assert_eq!(
        recipes[0].result,
        RecipeResult {
            item: Identifier::parse("minecraft:cobblestone_slab").unwrap(),
            count: 2,
            stew_effects: Vec::new(),
        }
    );
}

#[test]
fn loads_canonical_cookingtime_and_experience() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("raw_iron_blasting.json"),
        r#"{
          "type": "minecraft:blasting",
          "ingredient": "minecraft:raw_iron",
          "result": { "id": "minecraft:iron_ingot" },
          "cookingtime": 100,
          "experience": 0.7
        }"#,
    );

    let recipes = load_recipes(root).unwrap();
    let RecipeKind::Blasting(recipe) = &recipes[0].kind else {
        panic!("expected blasting recipe");
    };
    assert_eq!(recipe.cooking_time, 100);
    assert_eq!(recipe.experience_milli, 700);
}

#[test]
fn parses_tag_ingredients() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("oak_planks.json"),
        r##"{
          "type": "minecraft:crafting_shapeless",
          "ingredients": ["#minecraft:oak_logs"],
          "result": { "id": "minecraft:oak_planks", "count": 4 }
        }"##,
    );

    let recipes = load_recipes(root).unwrap();
    let RecipeKind::Shapeless(shapeless) = &recipes[0].kind else {
        panic!("expected shapeless recipe");
    };
    assert_eq!(
        shapeless.ingredients[0].alternatives[0],
        IngredientAlternative::Tag(Identifier::parse("minecraft:oak_logs").unwrap())
    );
    assert_eq!(recipes[0].result.item.as_str(), "minecraft:oak_planks");
}

#[test]
fn missing_recipe_directory_loads_empty() {
    let tmp = tempfile::tempdir().unwrap();

    let recipes = load_recipes(tmp.path().join("missing")).unwrap();

    assert!(recipes.is_empty());
}

#[test]
fn embedded_required_recipes_cover_basic_wood_and_stone_tools() {
    let recipes = solaris_required_recipes();
    for id in [
        "minecraft:wooden_pickaxe",
        "minecraft:wooden_axe",
        "minecraft:wooden_shovel",
        "minecraft:wooden_sword",
        "minecraft:wooden_hoe",
        "minecraft:stone_pickaxe",
        "minecraft:stone_axe",
        "minecraft:stone_shovel",
        "minecraft:stone_sword",
        "minecraft:stone_hoe",
    ] {
        assert!(
            recipes.iter().any(|recipe| recipe.id.as_str() == id
                && recipe.result.item.as_str() == id
                && recipe.result.count == 1),
            "missing fallback recipe {id}"
        );
    }
}

#[test]
fn embedded_required_recipes_turn_one_bone_into_three_bone_meal() {
    let recipes = solaris_required_recipes();
    let recipe = recipes
        .iter()
        .find(|recipe| recipe.id.as_str() == "minecraft:bone_meal")
        .expect("bone meal fallback recipe");

    assert_eq!(recipe.result.item.as_str(), "minecraft:bone_meal");
    assert_eq!(recipe.result.count, 3);
    let RecipeKind::Shapeless(shapeless) = &recipe.kind else {
        panic!("bone meal fallback recipe must be shapeless");
    };
    assert_eq!(shapeless.ingredients.len(), 1);
    assert_eq!(
        shapeless.ingredients[0].alternatives,
        vec![IngredientAlternative::Item(
            Identifier::parse("minecraft:bone").unwrap()
        )]
    );
}

#[test]
fn embedded_required_recipes_cover_charcoal_from_logs() {
    let recipes = solaris_required_recipes();
    let recipe = recipes
        .iter()
        .find(|recipe| recipe.id.as_str() == "minecraft:charcoal")
        .expect("charcoal fallback recipe");
    assert_eq!(recipe.result.item.as_str(), "minecraft:charcoal");
    assert_eq!(recipe.result.count, 1);

    let RecipeKind::Smelting(smelting) = &recipe.kind else {
        panic!("charcoal fallback recipe must be a furnace smelting recipe");
    };
    assert_eq!(smelting.cooking_time, 200);
    assert_eq!(smelting.experience_milli, 150);
    assert!(
        smelting.ingredient.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:logs_that_burn")
        }),
        "charcoal fallback must accept the generated natural log family through minecraft:logs_that_burn"
    );
}

#[test]
fn embedded_required_recipes_cover_playable_cooked_passive_food() {
    let recipes = solaris_required_recipes();
    for (raw_item, cooked_item) in [
        ("minecraft:beef", "minecraft:cooked_beef"),
        ("minecraft:porkchop", "minecraft:cooked_porkchop"),
        ("minecraft:chicken", "minecraft:cooked_chicken"),
    ] {
        let recipe = recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == cooked_item)
            .unwrap_or_else(|| panic!("missing cooked passive food fallback recipe {cooked_item}"));
        assert_eq!(recipe.result.item.as_str(), cooked_item);
        assert_eq!(recipe.result.count, 1);

        let RecipeKind::Smelting(smelting) = &recipe.kind else {
            panic!("cooked passive food fallback recipe must be a furnace smelting recipe");
        };
        assert_eq!(smelting.cooking_time, 200);
        assert_eq!(smelting.experience_milli, 350);
        assert!(
            smelting.ingredient.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == raw_item)
            }),
            "cooked passive food fallback recipe {cooked_item} must accept {raw_item}"
        );
    }
}

#[test]
fn embedded_required_recipes_cover_playable_chest_from_planks() {
    let recipes = solaris_required_recipes();
    let recipe = recipes
        .iter()
        .find(|recipe| recipe.id.as_str() == "minecraft:chest")
        .expect("chest fallback recipe");
    assert_eq!(recipe.result.item.as_str(), "minecraft:chest");
    assert_eq!(recipe.result.count, 1);

    let RecipeKind::Shaped(shaped) = &recipe.kind else {
        panic!("chest fallback recipe must be a shaped crafting recipe");
    };
    assert_eq!(shaped.pattern, ["###", "# #", "###"]);
    let planks = shaped.key.get(&'#').expect("chest planks ingredient");
    assert!(
        planks.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:planks")
        }),
        "chest fallback must accept earned generated planks through minecraft:planks"
    );
}

#[test]
fn embedded_required_recipes_cover_playable_white_bed() {
    let recipes = solaris_required_recipes();

    let recipe = recipes
        .iter()
        .find(|recipe| recipe.id.as_str() == "minecraft:zz_playable_white_bed")
        .expect("white bed fallback recipe");
    assert_eq!(recipe.result.count, 1);

    let RecipeKind::Shaped(shaped) = &recipe.kind else {
        panic!("white bed fallback recipe must be a shaped crafting recipe");
    };
    assert_eq!(shaped.pattern, ["###", "PPP"]);
    let wool = shaped.key.get(&'#').expect("bed wool ingredient");
    assert!(
        wool.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:white_wool")
        }),
        "playable white bed recipe must use naturally earned white wool"
    );
    let planks = shaped.key.get(&'P').expect("bed planks ingredient");
    assert!(
        planks.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:planks")
        }),
        "playable white bed recipe must accept generated planks through minecraft:planks"
    );
}

#[test]
fn embedded_required_recipes_cover_playable_wooden_doors() {
    let recipes = solaris_required_recipes();
    let display_id = |id: &str| {
        recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
    };

    for wood in [
        "acacia", "birch", "cherry", "dark_oak", "jungle", "mangrove", "oak", "pale_oak", "spruce",
    ] {
        let recipe_id = format!("minecraft:zz_playable_wooden_{wood}_door");
        let recipe = &recipes[display_id(&recipe_id)];
        assert_eq!(
            recipe.result.item.as_str(),
            format!("minecraft:{wood}_door")
        );
        assert_eq!(recipe.result.count, 3);

        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("{recipe_id} fallback recipe must be a shaped crafting recipe");
        };
        assert_eq!(shaped.pattern, ["##", "##", "##"]);
        let planks = shaped.key.get(&'#').expect("door planks ingredient");
        assert!(
            planks.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == format!("minecraft:{wood}_planks"))
            }),
            "playable door recipe {recipe_id} must use its matching generated planks item"
        );
    }
}

#[test]
fn embedded_required_recipes_cover_playable_wooden_signs() {
    let recipes = solaris_required_recipes();
    let display_id = |id: &str| {
        recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
    };

    for wood in [
        "acacia", "birch", "cherry", "dark_oak", "jungle", "mangrove", "oak", "pale_oak", "spruce",
    ] {
        let recipe_id = format!("minecraft:zz_playable_wooden_zsign_{wood}");
        let recipe = &recipes[display_id(&recipe_id)];
        assert_eq!(
            recipe.result.item.as_str(),
            format!("minecraft:{wood}_sign")
        );
        assert_eq!(recipe.result.count, 3);

        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("{recipe_id} fallback recipe must be a shaped crafting recipe");
        };
        assert_eq!(shaped.pattern, ["###", "###", " X "]);
        let planks = shaped.key.get(&'#').expect("sign planks ingredient");
        assert!(
            planks.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == format!("minecraft:{wood}_planks"))
            }),
            "playable sign recipe {recipe_id} must use its matching generated planks item"
        );
        let stick = shaped.key.get(&'X').expect("sign stick ingredient");
        assert!(
            stick.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
            }),
            "playable sign recipe {recipe_id} must use an earned stick"
        );
    }
}

#[test]
fn embedded_required_recipes_cover_playable_campfire() {
    let recipes = solaris_required_recipes();
    let display_id = |id: &str| {
        recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
    };

    let campfire = &recipes[display_id("minecraft:zz_playable_zz_campfire")];

    assert_eq!(campfire.result.item.as_str(), "minecraft:campfire");
    assert_eq!(campfire.result.count, 1);
    let RecipeKind::Shaped(shaped) = &campfire.kind else {
        panic!("campfire fallback recipe must be a shaped crafting recipe");
    };
    assert_eq!(shaped.pattern, [" S ", "SCS", "LLL"]);
    let stick = shaped.key.get(&'S').expect("campfire stick ingredient");
    assert!(
        stick.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
        }),
        "playable campfire recipe must use earned sticks"
    );
    let charcoal = shaped.key.get(&'C').expect("campfire charcoal ingredient");
    assert!(
        charcoal.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:charcoal")
        }),
        "playable campfire recipe must use earned charcoal"
    );
    let logs = shaped.key.get(&'L').expect("campfire log ingredient");
    assert!(
        logs.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:logs_that_burn")
        }),
        "playable campfire recipe must accept generated logs through minecraft:logs_that_burn"
    );

    for (raw_item, cooked_item, recipe_id) in [
        (
            "minecraft:beef",
            "minecraft:cooked_beef",
            "minecraft:cooked_beef_from_campfire_cooking",
        ),
        (
            "minecraft:chicken",
            "minecraft:cooked_chicken",
            "minecraft:cooked_chicken_from_campfire_cooking",
        ),
        (
            "minecraft:porkchop",
            "minecraft:cooked_porkchop",
            "minecraft:cooked_porkchop_from_campfire_cooking",
        ),
    ] {
        let recipe = &recipes[display_id(recipe_id)];

        assert_eq!(recipe.result.item.as_str(), cooked_item);
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::CampfireCooking(cooking) = &recipe.kind else {
            panic!("{recipe_id} fallback recipe must be campfire cooking");
        };
        assert_eq!(cooking.cooking_time, 600);
        assert!(
            cooking.ingredient.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == raw_item)
            }),
            "playable campfire cooking recipe {recipe_id} must accept {raw_item}"
        );
    }
}

#[test]
fn embedded_required_recipes_cover_playable_iron_sword() {
    let recipes = solaris_required_recipes();
    let display_id = |id: &str| {
        recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
    };

    let recipe_id = "minecraft:zz_playable_zz_iron_sword";
    let recipe = &recipes[display_id(recipe_id)];

    assert_eq!(recipe.result.item.as_str(), "minecraft:iron_sword");
    assert_eq!(recipe.result.count, 1);
    let RecipeKind::Shaped(shaped) = &recipe.kind else {
        panic!("playable iron sword fallback recipe must be shaped");
    };
    assert_eq!(shaped.pattern, ["#", "#", "X"]);
    let iron = shaped.key.get(&'#').expect("iron sword ingot ingredient");
    assert!(
        iron.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
        }),
        "playable iron sword recipe must use earned iron ingots"
    );
    let stick = shaped.key.get(&'X').expect("iron sword stick ingredient");
    assert!(
        stick.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
        }),
        "playable iron sword recipe must use earned sticks"
    );

    let shield_recipe_id = "minecraft:zz_playable_zz_shield";
    let shield = &recipes[display_id(shield_recipe_id)];

    assert_eq!(shield.result.item.as_str(), "minecraft:shield");
    assert_eq!(shield.result.count, 1);
    let RecipeKind::Shaped(shaped) = &shield.kind else {
        panic!("playable shield fallback recipe must be shaped");
    };
    assert_eq!(shaped.pattern, ["PIP", "PPP", " P "]);
    let planks = shaped.key.get(&'P').expect("shield planks ingredient");
    assert!(
        planks.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:planks")
        }),
        "playable shield recipe must use earned planks"
    );
    let iron = shaped.key.get(&'I').expect("shield iron ingredient");
    assert!(
        iron.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
        }),
        "playable shield recipe must use an earned iron ingot"
    );

    let chestplate_recipe_id = "minecraft:zz_playable_zzz_iron_chestplate";
    let chestplate = &recipes[display_id(chestplate_recipe_id)];

    assert_eq!(chestplate.result.item.as_str(), "minecraft:iron_chestplate");
    assert_eq!(chestplate.result.count, 1);
    let RecipeKind::Shaped(shaped) = &chestplate.kind else {
        panic!("playable iron chestplate fallback recipe must be shaped");
    };
    assert_eq!(shaped.pattern, ["# #", "###", "###"]);
    let iron = shaped
        .key
        .get(&'#')
        .expect("iron chestplate ingot ingredient");
    assert!(
        iron.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
        }),
        "playable iron chestplate recipe must use earned iron ingots"
    );
}

#[test]
fn embedded_required_recipes_cover_playable_bread() {
    let recipes = solaris_required_recipes();
    let display_id = |id: &str| {
        recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
    };

    let recipe_id = "minecraft:bread";
    let recipe = &recipes[display_id(recipe_id)];

    assert_eq!(recipe.result.item.as_str(), "minecraft:bread");
    assert_eq!(recipe.result.count, 1);
    let RecipeKind::Shaped(shaped) = &recipe.kind else {
        panic!("playable bread fallback recipe must be shaped");
    };
    assert_eq!(shaped.pattern, ["###"]);
    let wheat = shaped.key.get(&'#').expect("bread wheat ingredient");
    assert!(
        wheat.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:wheat")
        }),
        "playable bread recipe must use harvested wheat"
    );
}

#[test]
fn embedded_required_recipes_cover_playable_iron_pickaxe() {
    let recipes = solaris_required_recipes();
    let display_id = |id: &str| {
        recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
    };

    let recipe_id = "minecraft:zz_playable_zzzzz_iron_pickaxe";
    let recipe = &recipes[display_id(recipe_id)];

    assert_eq!(recipe.result.item.as_str(), "minecraft:iron_pickaxe");
    assert_eq!(recipe.result.count, 1);
    let RecipeKind::Shaped(shaped) = &recipe.kind else {
        panic!("playable iron pickaxe fallback recipe must be shaped");
    };
    assert_eq!(shaped.pattern, ["###", " X ", " X "]);
    let iron = shaped.key.get(&'#').expect("iron pickaxe ingot ingredient");
    assert!(iron.alternatives.iter().any(|alternative| {
        matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
    }));
    let stick = shaped.key.get(&'X').expect("iron pickaxe stick ingredient");
    assert!(stick.alternatives.iter().any(|alternative| {
        matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
    }));
}

#[test]
fn embedded_required_recipes_cover_core_diamond_tools() {
    let recipes = solaris_required_recipes();
    let recipe = |id: &str, output: &str, pattern: &[&str]| {
        let display_id = recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing fallback recipe {id}"));
        let recipe = &recipes[display_id];
        assert_eq!(recipe.result.item.as_str(), output);
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("{id} fallback recipe must be shaped");
        };
        assert_eq!(shaped.pattern, pattern);
        assert!(shaped
            .key
            .get(&'#')
            .expect("diamond ingredient")
            .alternatives
            .iter()
            .any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:diamond")
            }));
        assert!(shaped
            .key
            .get(&'X')
            .expect("stick ingredient")
            .alternatives
            .iter()
            .any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
            }));
    };

    recipe(
        "minecraft:zz_playable_zzzzzz_diamond_pickaxe",
        "minecraft:diamond_pickaxe",
        &["###", " X ", " X "],
    );
    recipe(
        "minecraft:zz_playable_zzzzzzz_diamond_sword",
        "minecraft:diamond_sword",
        &["#", "#", "X"],
    );
}

#[test]
fn embedded_required_recipes_cover_playable_bucket() {
    let recipes = solaris_required_recipes();
    let recipe_id = "minecraft:zz_playable_zzzzzzzz_bucket";
    let display_id = recipes
        .iter()
        .position(|recipe| recipe.id.as_str() == recipe_id)
        .unwrap_or_else(|| panic!("missing fallback recipe {recipe_id}"));

    let recipe = &recipes[display_id];
    assert_eq!(recipe.result.item.as_str(), "minecraft:bucket");
    assert_eq!(recipe.result.count, 1);
    let RecipeKind::Shaped(shaped) = &recipe.kind else {
        panic!("playable bucket fallback recipe must be shaped");
    };
    assert_eq!(shaped.pattern, ["# #", " # "]);
    assert!(shaped
        .key
        .get(&'#')
        .expect("bucket iron ingredient")
        .alternatives
        .iter()
        .any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
        }));
}

#[test]
fn embedded_required_recipes_complete_playable_iron_tier_after_bucket() {
    let recipes = solaris_required_recipes();

    for (id, output, pattern, uses_stick) in [
        (
            "minecraft:zz_playable_zzzzzzzzz_iron_axe",
            "minecraft:iron_axe",
            &["XX", "X#", " #"][..],
            true,
        ),
        (
            "minecraft:zz_playable_zzzzzzzzzz_iron_shovel",
            "minecraft:iron_shovel",
            &["X", "#", "#"][..],
            true,
        ),
        (
            "minecraft:zz_playable_zzzzzzzzzzz_iron_hoe",
            "minecraft:iron_hoe",
            &["XX", " #", " #"][..],
            true,
        ),
        (
            "minecraft:zz_playable_zzzzzzzzzzzz_iron_helmet",
            "minecraft:iron_helmet",
            &["XXX", "X X"][..],
            false,
        ),
        (
            "minecraft:zz_playable_zzzzzzzzzzzzz_iron_leggings",
            "minecraft:iron_leggings",
            &["XXX", "X X", "X X"][..],
            false,
        ),
        (
            "minecraft:zz_playable_zzzzzzzzzzzzzz_iron_boots",
            "minecraft:iron_boots",
            &["X X", "X X"][..],
            false,
        ),
    ] {
        let actual_display_id = recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing fallback recipe {id}"));
        let recipe = &recipes[actual_display_id];
        assert_eq!(recipe.result.item.as_str(), output);
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("{id} fallback recipe must be shaped");
        };
        assert_eq!(shaped.pattern, pattern);
        assert!(shaped
            .key
            .get(&'X')
            .expect("iron ingredient")
            .alternatives
            .iter()
            .any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
            }));
        assert_eq!(shaped.key.contains_key(&'#'), uses_stick);
        if uses_stick {
            assert!(shaped
                .key
                .get(&'#')
                .expect("stick ingredient")
                .alternatives
                .iter()
                .any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
                }));
        }
    }
}

#[test]
fn embedded_required_recipes_complete_playable_diamond_tier_after_iron() {
    let recipes = solaris_required_recipes();

    for (id, output, pattern, uses_stick) in [
        (
            "minecraft:zzz_playable_diamond_axe",
            "minecraft:diamond_axe",
            &["XX", "X#", " #"][..],
            true,
        ),
        (
            "minecraft:zzz_playable_diamond_boots",
            "minecraft:diamond_boots",
            &["X X", "X X"][..],
            false,
        ),
        (
            "minecraft:zzz_playable_diamond_chestplate",
            "minecraft:diamond_chestplate",
            &["X X", "XXX", "XXX"][..],
            false,
        ),
        (
            "minecraft:zzz_playable_diamond_helmet",
            "minecraft:diamond_helmet",
            &["XXX", "X X"][..],
            false,
        ),
        (
            "minecraft:zzz_playable_diamond_hoe",
            "minecraft:diamond_hoe",
            &["XX", " #", " #"][..],
            true,
        ),
        (
            "minecraft:zzz_playable_diamond_leggings",
            "minecraft:diamond_leggings",
            &["XXX", "X X", "X X"][..],
            false,
        ),
        (
            "minecraft:zzz_playable_diamond_shovel",
            "minecraft:diamond_shovel",
            &["X", "#", "#"][..],
            true,
        ),
    ] {
        let actual_display_id = recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing fallback recipe {id}"));
        let recipe = &recipes[actual_display_id];
        assert_eq!(recipe.result.item.as_str(), output);
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("{id} fallback recipe must be shaped");
        };
        assert_eq!(shaped.pattern, pattern);
        assert!(shaped
            .key
            .get(&'X')
            .expect("diamond ingredient")
            .alternatives
            .iter()
            .any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:diamond")
            }));
        assert_eq!(shaped.key.contains_key(&'#'), uses_stick);
        if uses_stick {
            assert!(shaped
                .key
                .get(&'#')
                .expect("stick ingredient")
                .alternatives
                .iter()
                .any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
                }));
        }
    }
}

#[test]
fn embedded_required_recipes_turn_spider_string_into_a_bow() {
    let recipes = solaris_required_recipes();
    let display_id = recipes
        .iter()
        .position(|recipe| recipe.id.as_str() == "minecraft:zzzz_playable_bow")
        .expect("playable bow recipe");
    let recipe = &recipes[display_id];
    assert_eq!(recipe.result.item.as_str(), "minecraft:bow");
    assert_eq!(recipe.result.count, 1);
    let RecipeKind::Shaped(shaped) = &recipe.kind else {
        panic!("playable bow recipe must be shaped");
    };
    assert_eq!(shaped.pattern, [" #X", "# X", " #X"]);
    assert!(shaped.key.get(&'#').unwrap().alternatives.iter().any(
        |alternative| matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
    ));
    assert!(shaped.key.get(&'X').unwrap().alternatives.iter().any(
        |alternative| matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:string")
    ));
}

#[test]
fn embedded_required_recipes_add_shears_after_existing_display_ids() {
    let recipes = solaris_required_recipes();
    let display_id = recipes
        .iter()
        .position(|recipe| recipe.id.as_str() == "minecraft:zzzzz_playable_shears")
        .expect("playable shears recipe");
    let recipe = &recipes[display_id];
    assert_eq!(recipe.result.item.as_str(), "minecraft:shears");
    assert_eq!(recipe.result.count, 1);
    let RecipeKind::Shaped(shaped) = &recipe.kind else {
        panic!("playable shears recipe must be shaped");
    };
    assert_eq!(shaped.pattern, [" #", "# "]);
    assert!(shaped
        .key
        .get(&'#')
        .expect("shears iron ingredient")
        .alternatives
        .iter()
        .any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
        }));
}

#[test]
fn rejects_missing_shaped_key() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("bad.json"),
        r###"{
          "type": "minecraft:crafting_shaped",
          "pattern": ["#X"],
          "key": { "#": "minecraft:stone" },
          "result": { "id": "minecraft:stone" }
        }"###,
    );

    let err = load_recipes(root).unwrap_err();

    assert!(matches!(err, RecipeDataError::MissingKey { key: 'X', .. }));
}

#[test]
#[ignore = "requires local 26.1.2 recipe sidecars"]
fn loads_real_recipe_sidecar_when_present() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("data/vanilla/data/minecraft/recipe");
    assert!(
        root.is_dir(),
        "{} missing; run tools/extract-vanilla-data.sh",
        root.display()
    );

    let recipes = load_recipes(root).unwrap();
    assert!(recipes.len() > 100, "vanilla has many crafting recipes");
    assert!(recipes.iter().any(|recipe| {
        recipe.id.as_str() == "minecraft:stick" && recipe.result.item.as_str() == "minecraft:stick"
    }));
}
