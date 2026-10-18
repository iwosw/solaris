//! Native S1 settlement startup data (D006 S1).
//!
//! The sibling package's `configure` contributions from
//! `military_catalog::authored_items` and `production::authored_recipes`
//! enter the existing `plugin_startup` pipeline here: the same six custom
//! items and fifty-six work recipes, with the same identities, revisions
//! and stations. No production or military runtime moves into S1.

use mc_data::Identifier;

/// Owner namespace for every S1 startup identity below.
pub const OWNER: &str = "solaris-settlements";

type ItemDefinition = (
    &'static str,
    &'static str,
    u32,
    bool,
    Option<&'static str>,
    &'static str,
);
type ManufacturingRecipe = (&'static str, u32, u64, &'static [(&'static str, u32)]);

/// The six custom items the sibling `configure` authors
/// (`military_catalog::authored_items`): id, display name, durability,
/// weapon, equip slot, physical profile JSON.
pub const ITEMS: [ItemDefinition; 6] = [
    (
        "solaris-settlements:arming_sword",
        "Arming sword",
        320,
        true,
        None,
        r#"{"attacks": [{"mode":"melee","damage_kind":"cut","base_damage_hp":5.0,"windup_ticks":6,"active_ticks":2,"recovery_ticks":10,"min_range_blocks":0.0,"reach_blocks":1.9,"stamina_cost":12.0,"shape":{"kind":"cone","half_angle_degrees":45.0},"projectile":null,"consumption":null,"brace":null,"charge":null,"two_handed":false,"wear":1}],"armor": null,"shield": null,"stamina_capacity": 100.0,"stamina_recovery_per_tick": 0.5}"#,
    ),
    (
        "solaris-settlements:longsword",
        "Longsword",
        280,
        true,
        None,
        r#"{"attacks": [{"mode":"melee","damage_kind":"cut","base_damage_hp":7.0,"windup_ticks":10,"active_ticks":3,"recovery_ticks":16,"min_range_blocks":0.0,"reach_blocks":2.8,"stamina_cost":22.0,"shape":{"kind":"cone","half_angle_degrees":55.0},"projectile":null,"consumption":null,"brace":null,"charge":null,"two_handed":true,"wear":1}],"armor": null,"shield": null,"stamina_capacity": 100.0,"stamina_recovery_per_tick": 0.5}"#,
    ),
    (
        "solaris-settlements:spear",
        "Spear",
        240,
        true,
        None,
        r#"{"attacks": [{"mode":"melee","damage_kind":"pierce","base_damage_hp":5.0,"windup_ticks":8,"active_ticks":2,"recovery_ticks":14,"min_range_blocks":1.25,"reach_blocks":3.2,"stamina_cost":16.0,"shape":{"kind":"thrust","radius_blocks":0.18},"projectile":null,"consumption":null,"brace":null,"charge":null,"two_handed":true,"wear":1}],"armor": null,"shield": null,"stamina_capacity": 100.0,"stamina_recovery_per_tick": 0.5}"#,
    ),
    (
        "solaris-settlements:war_bow",
        "War bow",
        192,
        true,
        None,
        r#"{"attacks": [{"mode":"projectile","damage_kind":"pierce","base_damage_hp":5.0,"windup_ticks":16,"active_ticks":1,"recovery_ticks":20,"min_range_blocks":4.0,"reach_blocks":24.0,"stamina_cost":18.0,"shape":{"kind":"thrust","radius_blocks":0.12},"projectile":{"carrier":"minecraft:arrow","speed_blocks_per_tick":1.5,"gravity_blocks_per_tick_squared":0.035,"collision_radius_blocks":0.12,"lifetime_ticks":160,"recoverable":true,"impulse":0.18},"consumption":{"item":"minecraft:arrow","count":1},"brace":null,"charge":null,"two_handed":true,"wear":1}],"armor": null,"shield": null,"stamina_capacity": 100.0,"stamina_recovery_per_tick": 0.5}"#,
    ),
    (
        "solaris-settlements:mail_hauberk",
        "Mail hauberk",
        480,
        false,
        Some("chest"),
        r#"{"attacks": [],"armor": {"slot":"chest","coverage":[{"region":"torso","fraction":1.0},{"region":"arms","fraction":0.75},{"region":"legs","fraction":0.2}],"resistance":[{"kind":"cut","fraction":0.45},{"kind":"pierce","fraction":0.30},{"kind":"crush","fraction":0.12},{"kind":"fire","fraction":0.05},{"kind":"blast","fraction":0.10},{"kind":"hook","fraction":0.0}],"common_cap":0.60,"mass_units":8.0,"stamina_cost":2.0,"degradation":[{"remaining_fraction":0.0,"resistance_multiplier":0.2},{"remaining_fraction":0.25,"resistance_multiplier":0.6},{"remaining_fraction":1.0,"resistance_multiplier":1.0}],"wear_per_damage_hp":0.5},"shield": null,"stamina_capacity": 100.0,"stamina_recovery_per_tick": 0.5}"#,
    ),
    (
        "solaris-settlements:pavise",
        "Pavise",
        360,
        false,
        None,
        r#"{"attacks": [],"armor": null,"shield": {"width_blocks":1.0,"height_blocks":1.75,"forward_offset_blocks":0.55,"bottom_offset_blocks":0.0,"activation_ticks":8,"manual":true,"deployable":true,"block_stamina_cost":14.0,"block_wear_per_damage_hp":0.75,"max_durability":360,"hook_disables_ticks":30,"impulse_limit":1.2},"stamina_capacity": 100.0,"stamina_recovery_per_tick": 0.5}"#,
    ),
];

/// The six manufacturing work recipes the sibling `configure` authors
/// (`military_catalog::authored_recipes`): id, durability, duration, inputs.
pub const MANUFACTURING_RECIPES: [ManufacturingRecipe; 6] = [
    (
        "solaris-settlements:arming_sword",
        320,
        320,
        &[
            ("minecraft:iron_ingot", 2),
            ("minecraft:stick", 1),
            ("minecraft:leather", 1),
        ],
    ),
    (
        "solaris-settlements:longsword",
        280,
        480,
        &[
            ("minecraft:iron_ingot", 3),
            ("minecraft:stick", 1),
            ("minecraft:leather", 1),
        ],
    ),
    (
        "solaris-settlements:spear",
        240,
        240,
        &[("minecraft:iron_ingot", 1), ("minecraft:stick", 2)],
    ),
    (
        "solaris-settlements:war_bow",
        192,
        360,
        &[("minecraft:stick", 3), ("minecraft:string", 3)],
    ),
    (
        "solaris-settlements:mail_hauberk",
        480,
        640,
        &[("minecraft:iron_ingot", 8), ("minecraft:leather", 2)],
    ),
    (
        "solaris-settlements:pavise",
        360,
        480,
        &[
            ("minecraft:oak_planks", 6),
            ("minecraft:leather", 2),
            ("minecraft:string", 1),
        ],
    ),
];

/// Build the six `CustomItemDefinition` values the sibling pipeline hands to
/// `ItemFactsTable::with_custom_items`. Same conversion as
/// `mc-plugin-host/src/startup.rs::convert_item_definition`: owned
/// `solaris-settlements` ids, vanilla `minecraft:paper` carrier, stack 1,
/// physical profile parsed from the authored JSON.
pub fn authored_items() -> Result<Vec<mc_data::item_components::CustomItemDefinition>, String> {
    let mut items = Vec::with_capacity(ITEMS.len());
    for (id, name, durability, weapon, slot, profile) in ITEMS {
        let id = Identifier::parse(id).map_err(|error| error.to_string())?;
        if id.namespace() != OWNER {
            return Err(format!("startup item {id} belongs to another namespace"));
        }
        let carrier = Identifier::parse("minecraft:paper").map_err(|error| error.to_string())?;
        let profile: mc_data::item_components::PhysicalItemProfile =
            serde_json::from_str(profile).map_err(|error| format!("startup item {id}: {error}"))?;
        items.push(mc_data::item_components::CustomItemDefinition {
            id,
            carrier,
            name: name.to_owned(),
            crafting_ingredient: None,
            facts: mc_data::item_components::ItemFacts {
                max_stack_size: Some(1),
                max_damage: Some(durability),
                weapon,
                equippable_slot: slot.map(str::to_owned),
                physical_profile: Some(Box::new(profile)),
                ..Default::default()
            },
        });
    }
    Ok(items)
}

/// Build the work-recipe JSON strings the sibling `configure` hands to
/// `convert_work_recipes`: the fourteen `production_recipes.json` bases plus
/// the six manufacturing recipes above, expanded exactly as the sibling
/// `authored_definitions` expands them. The four quality tiers below are the
/// authored expansion table only (tier name + durability scaling that lands
/// in the recipe JSON and its `custom_data` stamp); the provisional
/// decision-status/weight tuning that drives runtime manufacturing rolls
/// stays out. Plain bases pass through unchanged.
pub fn authored_recipes() -> Vec<String> {
    let mut recipes: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("startup_recipes.json"))
            .expect("S1 settlement startup recipes");
    for (id, durability, duration, materials) in MANUFACTURING_RECIPES {
        recipes.push(serde_json::json!({
            "id": id, "revision": "ruleset-r2-proposal-1",
            "station": "minecraft:crafting_table", "duration_ticks": duration,
            "inputs": materials.iter().map(|(item, count)| serde_json::json!({
                "item": item, "tag": null, "count": count, "components": null
            })).collect::<Vec<_>>(),
            "outputs": [{"item": id, "count": 1, "components": {
                "damage": 0, "enchantments": [], "custom_name": null,
                "item_model": null, "stew_effects": [], "max_damage": durability,
                "custom_data": null
            }}],
            "remainders": [], "tools": []
        }));
    }
    recipes
        .into_iter()
        .flat_map(expand_recipe)
        .map(|recipe| serde_json::to_string(&recipe).expect("startup recipe JSON"))
        .collect()
}

/// Authored quality expansion table: the tier names and durability scaling
/// the sibling bakes into startup recipe JSON. Weights and decision status
/// are runtime manufacturing policy, not startup data, and stay out.
const QUALITY_TIERS: [(&str, u64); 4] = [
    ("rough", 90),
    ("ordinary", 100),
    ("good", 110),
    ("masterwork", 120),
];

fn quality_stamp(tier: &str) -> Vec<u8> {
    // Same NBT-stamp bytes the sibling `quality_data` writes: a short tag
    // compound carrying `quality = <tier>`, so the native stamp is identical.
    let mut bytes = vec![10, 0, 0, 8, 0, 7, b'q', b'u', b'a', b'l', b'i', b't', b'y'];
    bytes.extend_from_slice(&(tier.len() as u16).to_be_bytes());
    bytes.extend_from_slice(tier.as_bytes());
    bytes.push(0);
    bytes
}

/// The sibling `expand_recipe` over the authored table above: bases whose
/// outputs or inputs carry `max_damage` fan out into the four quality
/// variants; plain bases pass through unchanged.
fn expand_recipe(base: serde_json::Value) -> Vec<serde_json::Value> {
    let graded = base["outputs"].as_array().is_some_and(|outputs| {
        outputs
            .iter()
            .any(|output| output["components"]["max_damage"].is_number())
    }) || base["inputs"].as_array().is_some_and(|inputs| {
        inputs
            .iter()
            .any(|input| input["components"]["max_damage"].is_number())
    });
    if !graded {
        return vec![base];
    }
    QUALITY_TIERS
        .iter()
        .map(|(tier, max_damage_percent)| {
            let mut variant = base.clone();
            variant["id"] = serde_json::Value::String(format!(
                "{base}_{tier}",
                base = base["id"].as_str().expect("recipe id"),
            ));
            variant["revision"] = serde_json::Value::String(format!(
                "{}-ruleset-1",
                base["revision"].as_str().expect("recipe revision"),
            ));
            for field in ["inputs", "outputs"] {
                for stack in variant[field].as_array_mut().expect("recipe stacks") {
                    if let Some(cap) = stack["components"]["max_damage"].as_u64() {
                        stack["components"]["max_damage"] =
                            serde_json::json!(cap * max_damage_percent / 100);
                        stack["components"]["custom_data"] = serde_json::json!(quality_stamp(tier));
                    }
                }
            }
            variant
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_items_convert_through_item_facts() {
        let items = authored_items().unwrap();
        assert_eq!(items.len(), 6);
        for item in &items {
            assert_eq!(item.id.namespace(), OWNER);
            assert_eq!(item.carrier.as_str(), "minecraft:paper");
        }
    }

    #[test]
    fn startup_recipes_cover_production_bases_and_manufacturing() {
        // Same expansion the sibling `authored_recipes` produces: 8 plain
        // bases + 6 graded production bases x4 + 6 manufacturing x4 = 56.
        let recipes = authored_recipes();
        assert_eq!(recipes.len(), 56);
        assert!(
            recipes
                .iter()
                .any(|recipe| recipe.contains("solaris-settlements:oak_planks"))
        );
        assert!(
            recipes
                .iter()
                .any(|recipe| recipe.contains("solaris-settlements:arming_sword_ordinary"))
        );
    }

    #[test]
    fn startup_recipe_outputs_do_not_require_retired_package_models() {
        for recipe in authored_recipes() {
            let recipe: serde_json::Value = serde_json::from_str(&recipe).unwrap();
            for output in recipe["outputs"].as_array().unwrap() {
                assert!(
                    output["components"]["item_model"]
                        .as_str()
                        .is_none_or(|model| !model.starts_with("solaris-settlements:")),
                    "native recipe {} still requires a retired package model",
                    recipe["id"],
                );
            }
        }
    }
}
