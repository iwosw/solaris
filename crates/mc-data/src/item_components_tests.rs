use super::*;
use std::fs;

#[test]
fn loads_component_report_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("apple.json"),
        r#"{
              "components": {
                "minecraft:max_stack_size": 64,
                "minecraft:food": { "nutrition": 4, "saturation": 2.4 },
                "minecraft:consumable": {}
              }
            }"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("iron_helmet.json"),
        r#"{
              "components": {
                "minecraft:max_damage": 165,
                "minecraft:max_stack_size": 1,
                "minecraft:equippable": { "slot": "head" }
              }
            }"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("diamond_sword.json"),
        r#"{
              "components": {
                "minecraft:max_damage": 1561,
                "minecraft:max_stack_size": 1,
                "minecraft:attribute_modifiers": [
                  {
                    "type": "minecraft:attack_damage",
                    "amount": 6.0,
                    "id": "minecraft:base_attack_damage",
                    "operation": "add_value",
                    "slot": "mainhand"
                  },
                  {
                    "type": "minecraft:attack_speed",
                    "amount": -2.4,
                    "id": "minecraft:base_attack_speed",
                    "operation": "add_value",
                    "slot": "mainhand"
                  }
                ],
                "minecraft:weapon": {}
              }
            }"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("iron_chestplate.json"),
        r#"{
              "components": {
                "minecraft:max_damage": 240,
                "minecraft:max_stack_size": 1,
                "minecraft:equippable": { "slot": "chest" },
                "minecraft:attribute_modifiers": [
                  {
                    "type": "minecraft:armor",
                    "amount": 6.0,
                    "operation": "add_value",
                    "slot": "chest"
                  },
                  {
                    "type": "minecraft:armor_toughness",
                    "amount": 0.0,
                    "operation": "add_value",
                    "slot": "chest"
                  }
                ]
              }
            }"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("wooden_pickaxe.json"),
        r##"{
              "components": {
                "minecraft:max_damage": 59,
                "minecraft:tool": {
                  "rules": [
                    {
                      "blocks": "#minecraft:incorrect_for_wooden_tool",
                      "correct_for_drops": false
                    },
                    {
                      "blocks": ["minecraft:stone", "minecraft:cobblestone"],
                      "speed": 2.0,
                      "correct_for_drops": true
                    }
                  ]
                }
              }
            }"##,
    )
    .unwrap();
    fs::write(
        tmp.path().join("wooden_spear.json"),
        r#"{
              "components": {
                "minecraft:attack_range": {
                  "min_reach": 2.0,
                  "max_reach": 4.5,
                  "min_creative_reach": 2.0,
                  "max_creative_reach": 6.5,
                  "hitbox_margin": 0.125,
                  "mob_factor": 0.5
                }
              }
            }"#,
    )
    .unwrap();

    let facts = load_item_facts(tmp.path()).unwrap();

    let apple = facts
        .get(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    assert_eq!(apple.max_stack_size, Some(64));
    assert_eq!(
        apple.food,
        Some(FoodEntry {
            food: 4,
            saturation: 2.4,
            can_always_eat: false,
        })
    );
    assert_eq!(apple.use_duration_ticks, Some(32));
    assert_eq!(apple.use_action, Some(UseAction::Eat));

    let helmet = facts
        .get(&Identifier::parse("minecraft:iron_helmet").unwrap())
        .unwrap();
    assert_eq!(helmet.max_damage, Some(165));
    assert_eq!(helmet.max_stack_size, Some(1));
    assert_eq!(helmet.equippable_slot.as_deref(), Some("head"));

    let sword = facts
        .get(&Identifier::parse("minecraft:diamond_sword").unwrap())
        .unwrap();
    assert_eq!(sword.max_damage, Some(1561));
    assert!(sword.weapon);
    assert_eq!(sword.weapon_damage_per_attack, Some(1));
    assert_eq!(sword.attack_damage_modifier, Some(6.0));
    assert_eq!(sword.attack_speed_modifier, Some(-2.4));

    let chestplate = facts
        .get(&Identifier::parse("minecraft:iron_chestplate").unwrap())
        .unwrap();
    assert_eq!(chestplate.max_damage, Some(240));
    assert_eq!(chestplate.equippable_slot.as_deref(), Some("chest"));
    assert_eq!(
        chestplate.armor,
        Some(ItemArmorFacts {
            slot: "chest".to_string(),
            armor: 6.0,
            toughness: 0.0,
        })
    );

    let pickaxe = facts
        .get(&Identifier::parse("minecraft:wooden_pickaxe").unwrap())
        .and_then(|facts| facts.tool.as_ref())
        .expect("wooden pickaxe tool facts");
    assert_eq!(pickaxe.default_mining_speed, None);
    assert_eq!(pickaxe.rules.len(), 2);
    assert_eq!(
        pickaxe.rules[0],
        ToolRuleFacts {
            blocks: vec!["#minecraft:incorrect_for_wooden_tool".to_string()],
            speed: None,
            correct_for_drops: Some(false),
        }
    );
    assert_eq!(
        pickaxe.rules[1],
        ToolRuleFacts {
            blocks: vec![
                "minecraft:stone".to_string(),
                "minecraft:cobblestone".to_string(),
            ],
            speed: Some(2.0),
            correct_for_drops: Some(true),
        }
    );

    let spear = facts
        .get(&Identifier::parse("minecraft:wooden_spear").unwrap())
        .and_then(|facts| facts.attack_range)
        .expect("wooden spear attack range");
    assert_eq!(
        spear,
        AttackRangeFacts {
            min_reach: 2.0,
            max_reach: 4.5,
            min_creative_reach: 2.0,
            max_creative_reach: 6.5,
            hitbox_margin: 0.125,
            mob_factor: 0.5,
        }
    );
}

#[test]
fn missing_component_dir_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let facts = load_item_facts(tmp.path().join("missing")).unwrap();
    assert!(facts.is_empty());
}

#[test]
fn embedded_required_item_facts_cover_basic_wood_and_stone_tools() {
    let facts = solaris_required_item_facts();
    for (id, max_damage) in [
        ("minecraft:wooden_pickaxe", 59),
        ("minecraft:wooden_axe", 59),
        ("minecraft:wooden_shovel", 59),
        ("minecraft:wooden_sword", 59),
        ("minecraft:wooden_hoe", 59),
        ("minecraft:stone_pickaxe", 131),
        ("minecraft:stone_axe", 131),
        ("minecraft:stone_shovel", 131),
        ("minecraft:stone_sword", 131),
        ("minecraft:stone_hoe", 131),
    ] {
        let item = Identifier::parse(id).unwrap();
        let fact = facts.get(&item).unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(fact.max_damage, Some(max_damage), "{id}");
        assert!(fact.attack_damage_modifier.is_some(), "{id}");
    }
}

#[test]
fn embedded_required_item_facts_cover_shield_disable_contract() {
    let facts = solaris_required_item_facts();
    for id in [
        "minecraft:wooden_axe",
        "minecraft:stone_axe",
        "minecraft:iron_axe",
        "minecraft:golden_axe",
        "minecraft:diamond_axe",
        "minecraft:netherite_axe",
    ] {
        let fact = facts
            .get(&Identifier::parse(id).unwrap())
            .unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(fact.weapon_disable_blocking_seconds, Some(5.0), "{id}");
    }
    let shield = facts
        .get(&Identifier::parse("minecraft:shield").unwrap())
        .expect("embedded shield facts");
    assert_eq!(shield.max_damage, Some(336));
    assert_eq!(shield.max_stack_size, Some(1));
    assert_eq!(shield.blocks_attacks_disable_cooldown_scale, Some(1.0));
}

#[test]
fn embedded_required_item_facts_cover_shears_durability() {
    let facts = solaris_required_item_facts();
    let shears = facts
        .get(&Identifier::parse("minecraft:shears").unwrap())
        .expect("embedded shears facts");
    assert_eq!(shears.max_damage, Some(238));
    assert_eq!(shears.max_stack_size, Some(1));
}

#[test]
fn embedded_required_item_facts_cover_spear_attack_range() {
    let facts = solaris_required_item_facts();
    for id in [
        "minecraft:wooden_spear",
        "minecraft:stone_spear",
        "minecraft:copper_spear",
        "minecraft:iron_spear",
        "minecraft:golden_spear",
        "minecraft:diamond_spear",
        "minecraft:netherite_spear",
    ] {
        let range = facts
            .get(&Identifier::parse(id).unwrap())
            .and_then(|facts| facts.attack_range)
            .unwrap_or_else(|| panic!("missing attack range for {id}"));
        assert_eq!(range.max_reach, 4.5, "{id}");
        assert_eq!(range.max_creative_reach, 6.5, "{id}");
        assert_eq!(range.hitbox_margin, 0.125, "{id}");
    }
}

#[test]
#[ignore = "requires local 26.1.2 item component reports"]
fn loads_real_apple_and_bread_components_when_present() {
    let path = workspace_path("data/vanilla/reports/minecraft/components/item");
    assert!(
        path.is_dir(),
        "{} not present; run tools/extract-vanilla-data.sh",
        path.display()
    );

    let facts = load_item_facts(path).unwrap();

    let apple = facts
        .get(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    assert_eq!(apple.max_stack_size, Some(64));
    assert_eq!(
        apple.food,
        Some(FoodEntry {
            food: 4,
            saturation: 2.4,
            can_always_eat: false,
        })
    );
    assert_eq!(apple.use_duration_ticks, Some(32));
    assert_eq!(apple.use_action, Some(UseAction::Eat));

    let bread = facts
        .get(&Identifier::parse("minecraft:bread").unwrap())
        .unwrap();
    assert_eq!(bread.max_stack_size, Some(64));
    assert_eq!(
        bread.food,
        Some(FoodEntry {
            food: 5,
            saturation: 6.0,
            can_always_eat: false,
        })
    );
}

#[test]
#[ignore = "requires local 26.1.2 item component reports"]
fn loads_real_combat_components_when_present() {
    let path = workspace_path("data/vanilla/reports/minecraft/components/item");
    assert!(
        path.is_dir(),
        "{} not present; run tools/extract-vanilla-data.sh",
        path.display()
    );

    let facts = load_item_facts(path).unwrap();

    let sword = facts
        .get(&Identifier::parse("minecraft:diamond_sword").unwrap())
        .unwrap();
    assert_eq!(sword.max_damage, Some(1561));
    assert!(sword.weapon);
    assert_eq!(sword.weapon_damage_per_attack, Some(1));
    assert_eq!(sword.attack_damage_modifier, Some(6.0));
    assert!(sword.attack_speed_modifier.is_some());

    let axe = facts
        .get(&Identifier::parse("minecraft:iron_axe").unwrap())
        .unwrap();
    assert!(axe.weapon);
    assert_eq!(axe.weapon_damage_per_attack, Some(2));
    assert_eq!(axe.weapon_disable_blocking_seconds, Some(5.0));
    assert_eq!(axe.attack_damage_modifier, Some(8.0));

    let shield = facts
        .get(&Identifier::parse("minecraft:shield").unwrap())
        .unwrap();
    assert_eq!(shield.blocks_attacks_disable_cooldown_scale, Some(1.0));

    let bow = facts
        .get(&Identifier::parse("minecraft:bow").unwrap())
        .unwrap();
    assert_eq!(bow.max_damage, Some(384));
    assert_eq!(bow.max_stack_size, Some(1));

    let chestplate = facts
        .get(&Identifier::parse("minecraft:iron_chestplate").unwrap())
        .unwrap();
    assert_eq!(chestplate.max_damage, Some(240));
    assert_eq!(chestplate.equippable_slot.as_deref(), Some("chest"));
    assert_eq!(
        chestplate.armor,
        Some(ItemArmorFacts {
            slot: "chest".to_string(),
            armor: 6.0,
            toughness: 0.0,
        })
    );
}

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(rel)
}

#[test]
#[ignore = "requires local 26.1.2 item component reports"]
fn loads_real_ordered_tool_rules_when_present() {
    let path = workspace_path("data/vanilla/reports/minecraft/components/item");
    assert!(
        path.is_dir(),
        "{} not present; run tools/extract-vanilla-data.sh",
        path.display()
    );

    let facts = load_item_facts(path).unwrap();
    let pickaxe = facts
        .get(&Identifier::parse("minecraft:wooden_pickaxe").unwrap())
        .and_then(|facts| facts.tool.as_ref())
        .expect("wooden pickaxe tool facts");
    assert_eq!(pickaxe.default_mining_speed, None);
    assert_eq!(pickaxe.damage_per_block, None);
    assert_eq!(pickaxe.rules.len(), 2);
    assert_eq!(
        pickaxe.rules[0].blocks,
        ["#minecraft:incorrect_for_wooden_tool"]
    );
    assert_eq!(pickaxe.rules[0].correct_for_drops, Some(false));
    assert_eq!(pickaxe.rules[1].blocks, ["#minecraft:mineable/pickaxe"]);
    assert_eq!(pickaxe.rules[1].speed, Some(2.0));
    assert_eq!(pickaxe.rules[1].correct_for_drops, Some(true));
}

#[test]
fn embedded_equipment_and_projectile_facts_keep_vanilla_durability_and_modifiers() {
    let facts = solaris_required_item_facts();
    for (name, durability) in [
        ("bow", 384),
        ("crossbow", 465),
        ("fishing_rod", 64),
        ("trident", 250),
        ("flint_and_steel", 64),
        ("iron_sword", 250),
    ] {
        let entry = facts
            .get(&Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap();
        assert_eq!(entry.max_stack_size, Some(1), "{name}");
        assert_eq!(entry.max_damage, Some(durability), "{name}");
    }
    let chestplate = facts
        .get(&Identifier::parse("minecraft:diamond_chestplate").unwrap())
        .unwrap();
    assert_eq!(chestplate.max_damage, Some(528));
    assert_eq!(chestplate.equippable_slot.as_deref(), Some("chest"));
    assert_eq!(
        chestplate.armor,
        Some(ItemArmorFacts {
            slot: "chest".into(),
            armor: 8.0,
            toughness: 2.0,
        })
    );
    assert_eq!(
        chestplate.attribute_modifiers[0].id,
        "minecraft:armor.chestplate"
    );
    assert_eq!(chestplate.attribute_modifiers[0].operation, "add_value");
    let sword = facts
        .get(&Identifier::parse("minecraft:iron_sword").unwrap())
        .unwrap();
    assert_eq!(sword.attack_damage_modifier, Some(5.0));
    assert_eq!(
        sword.attribute_modifiers[0].slot.as_deref(),
        Some("mainhand")
    );
    let bottle = facts
        .get(&Identifier::parse("minecraft:ominous_bottle").unwrap())
        .unwrap();
    assert_eq!(bottle.use_action, Some(UseAction::Drink));
    assert_eq!(bottle.use_duration_ticks, Some(32));
}

#[test]
fn equippable_retains_swap_and_allowed_entity_restrictions() {
    for (component, swappable, allowed) in [
        (r#"{"slot":"head","swappable":false}"#, Some(false), None),
        (
            r#"{"slot":"head","allowed_entities":["minecraft:player"]}"#,
            None,
            Some(vec!["minecraft:player".to_owned()]),
        ),
        (
            r##"{"slot":"saddle","allowed_entities":"#minecraft:can_equip_saddle"}"##,
            None,
            Some(vec!["#minecraft:can_equip_saddle".to_owned()]),
        ),
    ] {
        let raw: RawItemComponents = serde_json::from_str(&format!(
            r#"{{"components":{{"minecraft:equippable":{component}}}}}"#
        ))
        .unwrap();
        let facts = raw.into_facts();
        assert_eq!(facts.equippable_swappable, swappable);
        assert_eq!(facts.equippable_allowed_entities, allowed);
    }
}
