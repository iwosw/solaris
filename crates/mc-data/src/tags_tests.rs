use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn make_tiny_sidecar() -> TempDir {
    // Minimal vanilla sidecar: registries.json with one item, one
    // tags file that references it, and a `#tag`-ref to a sibling.
    let dir = TempDir::new().unwrap();
    let reports = dir.path().join("reports");
    fs::create_dir_all(&reports).unwrap();
    fs::write(
        reports.join("registries.json"),
        r#"{
            "minecraft:item": {
                "entries": {
                    "minecraft:apple": { "protocol_id": 5 },
                    "minecraft:carrot": { "protocol_id": 7 }
                }
            }
        }"#,
    )
    .unwrap();
    let tags_item = dir
        .path()
        .join("data")
        .join("minecraft")
        .join("tags")
        .join("item");
    fs::create_dir_all(tags_item.join("food")).unwrap();
    fs::write(
        tags_item.join("food").join("snacks.json"),
        r##"{ "values": [ "minecraft:apple" ] }"##,
    )
    .unwrap();
    fs::write(
        tags_item.join("everything.json"),
        r##"{ "values": [ "#minecraft:food/snacks", "minecraft:carrot" ] }"##,
    )
    .unwrap();
    // Tag that points at a missing entry to make sure we keep going.
    fs::write(
        tags_item.join("hopeful.json"),
        r##"{ "values": [ { "id": "minecraft:nope", "required": false } ] }"##,
    )
    .unwrap();
    dir
}

fn empty_vanilla_data() -> VanillaData {
    VanillaData::from_registries("", vec![])
}

#[test]
fn contains_raw_id_handles_hash_prefix_and_invalid_identifiers() {
    let registry = Identifier::parse("minecraft:block").unwrap();
    let tag = Identifier::parse("minecraft:mineable/pickaxe").unwrap();
    let tags = TagsData::from_registries(BTreeMap::from([(
        registry,
        BTreeMap::from([(tag, vec![1, 4, 9])]),
    )]));

    assert!(tags.contains_raw_id("minecraft:block", "minecraft:mineable/pickaxe", 4));
    assert!(tags.contains_raw_id("minecraft:block", "#minecraft:mineable/pickaxe", 9));
    assert!(!tags.contains_raw_id("minecraft:block", "minecraft:mineable/pickaxe", 8));
    assert!(!tags.contains_raw_id("bad registry", "minecraft:mineable/pickaxe", 4));
    assert!(!tags.contains_raw_id("minecraft:block", "bad tag", 4));
}

#[test]
fn resolves_direct_and_transitive_references() {
    let dir = make_tiny_sidecar();
    let ours = empty_vanilla_data();
    let tags = load(dir.path(), &ours).unwrap();
    let item_reg = tags
        .registries
        .get(&Identifier::parse("minecraft:item").unwrap())
        .expect("item tags present");
    let snacks = item_reg
        .get(&Identifier::parse("minecraft:food/snacks").unwrap())
        .unwrap();
    assert_eq!(snacks, &[5]);
    let everything = item_reg
        .get(&Identifier::parse("minecraft:everything").unwrap())
        .unwrap();
    assert_eq!(everything, &[5, 7], "sorted, deduped, transitive");
    let hopeful = item_reg
        .get(&Identifier::parse("minecraft:hopeful").unwrap())
        .unwrap();
    assert!(
        hopeful.is_empty(),
        "missing optional entry yields empty tag"
    );
}

#[test]
fn cycles_fail_with_dependency_path() {
    let dir = TempDir::new().unwrap();
    let reports = dir.path().join("reports");
    fs::create_dir_all(&reports).unwrap();
    fs::write(reports.join("registries.json"), "{}").unwrap();
    let tags_item = dir
        .path()
        .join("data")
        .join("minecraft")
        .join("tags")
        .join("item");
    fs::create_dir_all(&tags_item).unwrap();
    fs::write(
        tags_item.join("a.json"),
        r##"{ "values": [ "#minecraft:b" ] }"##,
    )
    .unwrap();
    fs::write(
        tags_item.join("b.json"),
        r##"{ "values": [ "#minecraft:a" ] }"##,
    )
    .unwrap();

    let ours = empty_vanilla_data();
    let error = load(dir.path(), &ours).unwrap_err();
    assert!(matches!(
        error,
        TagError::DependencyCycle { cycle, .. }
            if cycle.contains("minecraft:item#a") && cycle.contains("minecraft:item#b")
    ));
}

#[test]
fn required_missing_nested_tag_fails_but_optional_missing_is_empty() {
    let dir = TempDir::new().unwrap();
    let reports = dir.path().join("reports");
    fs::create_dir_all(&reports).unwrap();
    fs::write(reports.join("registries.json"), "{}").unwrap();
    let tags_item = dir
        .path()
        .join("data")
        .join("minecraft")
        .join("tags")
        .join("item");
    fs::create_dir_all(&tags_item).unwrap();
    fs::write(
        tags_item.join("required.json"),
        r##"{ "values": [ "#minecraft:missing" ] }"##,
    )
    .unwrap();

    let error = load(dir.path(), &empty_vanilla_data()).unwrap_err();
    assert!(matches!(
        error,
        TagError::MissingRequiredTag { referenced, .. } if referenced == "minecraft:missing"
    ));

    fs::remove_file(tags_item.join("required.json")).unwrap();
    fs::write(
        tags_item.join("optional.json"),
        r##"{ "values": [ { "id": "#minecraft:missing", "required": false } ] }"##,
    )
    .unwrap();
    let tags = load(dir.path(), &empty_vanilla_data()).unwrap();
    let item_reg = tags
        .registries
        .get(&Identifier::parse("minecraft:item").unwrap())
        .unwrap();
    assert!(
        item_reg
            .get(&Identifier::parse("minecraft:optional").unwrap())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn replace_true_is_rejected_until_layered_datapacks_are_supported() {
    let dir = TempDir::new().unwrap();
    let reports = dir.path().join("reports");
    fs::create_dir_all(&reports).unwrap();
    fs::write(reports.join("registries.json"), "{}").unwrap();
    let tags_item = dir
        .path()
        .join("data")
        .join("minecraft")
        .join("tags")
        .join("item");
    fs::create_dir_all(&tags_item).unwrap();
    let path = tags_item.join("replace.json");
    fs::write(&path, r#"{ "replace": true, "values": [] }"#).unwrap();

    let error = load(dir.path(), &empty_vanilla_data()).unwrap_err();
    assert!(matches!(error, TagError::ReplaceUnsupported { path: actual } if actual == path));
}

#[test]
fn required_missing_registry_entry_fails() {
    let dir = TempDir::new().unwrap();
    let reports = dir.path().join("reports");
    fs::create_dir_all(&reports).unwrap();
    fs::write(
        reports.join("registries.json"),
        r#"{ "minecraft:item": { "entries": {} } }"#,
    )
    .unwrap();
    let tags_item = dir
        .path()
        .join("data")
        .join("minecraft")
        .join("tags")
        .join("item");
    fs::create_dir_all(&tags_item).unwrap();
    fs::write(
        tags_item.join("bad.json"),
        r#"{ "values": [ "minecraft:not_real" ] }"#,
    )
    .unwrap();

    let error = load(dir.path(), &empty_vanilla_data()).unwrap_err();
    assert!(matches!(
        error,
        TagError::MissingRequiredEntry { value, .. } if value == "minecraft:not_real"
    ));
}

#[test]
fn missing_registries_json_is_reported_clearly() {
    let dir = TempDir::new().unwrap();
    let ours = empty_vanilla_data();
    let err = load(dir.path(), &ours).unwrap_err();
    assert!(matches!(err, TagError::RegistriesMissing(_)));
}

#[test]
fn solaris_required_item_tags_cover_recipe_baseline() {
    let items = crate::items::solaris_required_items();
    let tags = solaris_required_item_tags(&items);
    let item_tags = tags
        .registries
        .get(&Identifier::parse("minecraft:item").unwrap())
        .expect("item tag registry present");

    assert_eq!(
        item_tags
            .get(&Identifier::parse("minecraft:birch_logs").unwrap())
            .unwrap(),
        &[165, 205, 180, 192]
    );
    let birch_log = items
        .id_of(&Identifier::parse("minecraft:birch_log").unwrap())
        .expect("birch log is in the required item registry") as i32;
    assert!(
        item_tags
            .get(&Identifier::parse("minecraft:logs_that_burn").unwrap())
            .unwrap()
            .contains(&birch_log),
        "generated birch logs must satisfy the vanilla logs_that_burn tag used by charcoal smelting"
    );
    let oak_planks = items
        .id_of(&Identifier::parse("minecraft:oak_planks").unwrap())
        .expect("oak planks are in the required item registry") as i32;
    assert!(
        item_tags
            .get(&Identifier::parse("minecraft:planks").unwrap())
            .unwrap()
            .contains(&oak_planks)
    );
    let egg_id = items
        .id_of(&Identifier::parse("minecraft:egg").unwrap())
        .expect("egg is in the required item registry");
    assert!(
        tags.contains_raw_id("minecraft:item", "minecraft:eggs", egg_id as i32),
        "recipe tag membership must not depend on source ordering"
    );
}

#[test]
fn recipe_book_tag_references_resolve_in_required_item_tags() {
    use crate::recipes::{Ingredient, Recipe, RecipeKind};

    fn ingredient_tags(recipe_id: &str, ingredient: &Ingredient) -> Vec<String> {
        ingredient
            .alternatives
            .iter()
            .filter_map(|alternative| match alternative {
                crate::recipes::IngredientAlternative::Tag(tag) => {
                    Some(format!("{recipe_id} -> {}", tag.as_str()))
                }
                crate::recipes::IngredientAlternative::Item(_) => None,
            })
            .collect()
    }

    fn recipe_tags(recipe: &Recipe) -> Vec<String> {
        let id = recipe.id.as_str();
        let ingredients: Vec<&Ingredient> = match &recipe.kind {
            RecipeKind::Shaped(shaped) => shaped.key.values().collect(),
            RecipeKind::Shapeless(shapeless) => shapeless.ingredients.iter().collect(),
            RecipeKind::Smelting(cooking)
            | RecipeKind::Blasting(cooking)
            | RecipeKind::Smoking(cooking)
            | RecipeKind::CampfireCooking(cooking) => vec![&cooking.ingredient],
            RecipeKind::Stonecutting(stonecutting) => vec![&stonecutting.ingredient],
        };
        ingredients
            .into_iter()
            .flat_map(|ingredient| ingredient_tags(id, ingredient))
            .collect()
    }

    let items = crate::items::solaris_required_items();
    let tags = solaris_required_item_tags(&items);
    let item_tags = tags
        .registries
        .get(&Identifier::parse("minecraft:item").unwrap())
        .expect("item tag registry present");
    // Embedded baseline always resolves; the full sidecar set additionally
    // resolves whenever the local vanilla checkout is present. A missing
    // tag kicks real clients in recipe_book_add (vanilla HolderSet tag
    // lookup throws on unknown tags).
    let mut recipes = crate::recipes::solaris_required_recipes();
    let sidecar =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla/data/minecraft/recipe");
    if sidecar.is_dir() {
        recipes.extend(crate::recipes::load_recipes(&sidecar).expect("sidecar recipes load"));
    }
    let mut missing = Vec::new();
    for recipe in &recipes {
        for reference in recipe_tags(recipe) {
            let tag = reference.split(" -> ").nth(1).expect("reference shape");
            let key = Identifier::parse(tag.to_string()).expect("valid tag id");
            if !item_tags.contains_key(&key) {
                missing.push(reference);
            }
        }
    }
    assert!(
        missing.is_empty(),
        "recipe tags missing from UpdateTags (kicks clients): {missing:?}"
    );
}

#[test]
fn embedded_fuel_snapshot_covers_canonical_vanilla_2612_set() {
    let items = crate::items::solaris_required_items();
    let tags = solaris_required_item_tags(&items).with_vanilla_fuel_values(&items);
    let fuels = tags.fuel_values();

    assert_eq!(
        items
            .iter()
            .filter(|(_, item_id)| fuels.is_fuel(*item_id))
            .count(),
        280
    );
    for (name, duration) in [
        ("minecraft:lava_bucket", 20_000),
        ("minecraft:oak_log", 300),
        ("minecraft:oak_slab", 150),
        ("minecraft:oak_hanging_sign", 800),
        ("minecraft:oak_boat", 1_200),
        ("minecraft:white_carpet", 67),
        ("minecraft:dried_kelp_block", 4_001),
        ("minecraft:bamboo", 50),
    ] {
        let item_id = items
            .id_of(&Identifier::parse(name).unwrap())
            .expect("embedded registry contains canonical fuel");
        assert_eq!(fuels.burn_duration(item_id), Some(duration), "{name}");
    }

    let crimson_planks = items
        .id_of(&Identifier::parse("minecraft:crimson_planks").unwrap())
        .unwrap();
    assert!(!fuels.is_fuel(crimson_planks));
}

#[test]
fn fuel_snapshot_does_not_change_wire_registries() {
    let items = crate::items::solaris_required_items();
    let tags = solaris_required_item_tags(&items);
    let registries = tags.registries.clone();

    let tags = tags.with_vanilla_fuel_values(&items);

    assert_eq!(tags.registries, registries);
}

#[test]
fn solaris_required_client_tags_bind_vanilla_261_configuration_dependencies() {
    let items = crate::items::solaris_required_items();
    let blocks = crate::blocks::solaris_required_blocks_report();
    let tags = solaris_required_client_tags(&items, &blocks);

    for (registry, required_tags) in [
        (
            "minecraft:banner_pattern",
            &[
                "minecraft:no_item_required",
                "minecraft:pattern_item/bordure_indented",
                "minecraft:pattern_item/creeper",
                "minecraft:pattern_item/field_masoned",
                "minecraft:pattern_item/flow",
                "minecraft:pattern_item/flower",
                "minecraft:pattern_item/globe",
                "minecraft:pattern_item/guster",
                "minecraft:pattern_item/mojang",
                "minecraft:pattern_item/piglin",
                "minecraft:pattern_item/skull",
            ][..],
        ),
        (
            "minecraft:block",
            &[
                "minecraft:blocks_wind_charge_explosions",
                "minecraft:infiniburn_end",
                "minecraft:infiniburn_nether",
                "minecraft:infiniburn_overworld",
                "minecraft:lightning_rods",
                "minecraft:soul_speed_blocks",
            ][..],
        ),
        (
            "minecraft:dialog",
            &[
                "minecraft:pause_screen_additions",
                "minecraft:quick_actions",
            ][..],
        ),
        (
            "minecraft:damage_type",
            &[
                "minecraft:always_hurts_ender_dragons",
                "minecraft:always_kills_armor_stands",
                "minecraft:always_most_significant_fall",
                "minecraft:always_triggers_silverfish",
                "minecraft:avoids_guardian_thorns",
                "minecraft:burn_from_stepping",
                "minecraft:burns_armor_stands",
                "minecraft:bypasses_armor",
                "minecraft:bypasses_effects",
                "minecraft:bypasses_enchantments",
                "minecraft:bypasses_invulnerability",
                "minecraft:bypasses_resistance",
                "minecraft:bypasses_shield",
                "minecraft:bypasses_wolf_armor",
                "minecraft:can_break_armor_stand",
                "minecraft:damages_helmet",
                "minecraft:ignites_armor_stands",
                "minecraft:is_drowning",
                "minecraft:is_explosion",
                "minecraft:is_fall",
                "minecraft:is_fire",
                "minecraft:is_freezing",
                "minecraft:is_lightning",
                "minecraft:is_player_attack",
                "minecraft:is_projectile",
                "minecraft:mace_smash",
                "minecraft:no_anger",
                "minecraft:no_impact",
                "minecraft:no_knockback",
                "minecraft:panic_causes",
                "minecraft:panic_environmental_causes",
                "minecraft:witch_resistant_to",
                "minecraft:wither_immune_to",
            ][..],
        ),
        (
            "minecraft:enchantment",
            &[
                "minecraft:exclusive_set/armor",
                "minecraft:exclusive_set/boots",
                "minecraft:exclusive_set/bow",
                "minecraft:exclusive_set/crossbow",
                "minecraft:exclusive_set/damage",
                "minecraft:exclusive_set/mining",
                "minecraft:exclusive_set/riptide",
            ][..],
        ),
        (
            "minecraft:entity_type",
            &[
                "minecraft:arrows",
                "minecraft:sensitive_to_bane_of_arthropods",
                "minecraft:sensitive_to_impaling",
                "minecraft:sensitive_to_smite",
            ][..],
        ),
        (
            "minecraft:fluid",
            &["minecraft:water", "minecraft:lava"][..],
        ),
        (
            "minecraft:item",
            &[
                "minecraft:enchantable/armor",
                "minecraft:enchantable/bow",
                "minecraft:enchantable/chest_armor",
                "minecraft:enchantable/crossbow",
                "minecraft:enchantable/durability",
                "minecraft:enchantable/equippable",
                "minecraft:enchantable/fire_aspect",
                "minecraft:enchantable/fishing",
                "minecraft:enchantable/foot_armor",
                "minecraft:enchantable/head_armor",
                "minecraft:enchantable/leg_armor",
                "minecraft:enchantable/lunge",
                "minecraft:enchantable/mace",
                "minecraft:enchantable/melee_weapon",
                "minecraft:enchantable/mining",
                "minecraft:enchantable/mining_loot",
                "minecraft:enchantable/sharp_weapon",
                "minecraft:enchantable/sweeping",
                "minecraft:enchantable/trident",
                "minecraft:enchantable/vanishing",
                "minecraft:enchantable/weapon",
            ][..],
        ),
        (
            "minecraft:timeline",
            &[
                "minecraft:in_end",
                "minecraft:in_nether",
                "minecraft:in_overworld",
                "minecraft:universal",
            ][..],
        ),
    ] {
        let registry_id = Identifier::parse(registry).unwrap();
        let registry_tags = tags
            .registries
            .get(&registry_id)
            .unwrap_or_else(|| panic!("{registry} fallback tag registry missing"));
        for required_tag in required_tags {
            let tag_id = Identifier::parse((*required_tag).to_string()).unwrap();
            assert!(
                registry_tags.contains_key(&tag_id),
                "{registry} fallback tag {required_tag} missing"
            );
        }
    }
}

#[test]
fn solaris_required_client_tags_bind_builtin_fluid_membership() {
    let items = crate::items::solaris_required_items();
    let blocks = crate::blocks::solaris_required_blocks_report();
    let tags = solaris_required_client_tags(&items, &blocks);
    let fluid_tags = tags
        .registries
        .get(&Identifier::parse("minecraft:fluid").unwrap())
        .expect("fluid tag registry present");

    assert_eq!(
        fluid_tags
            .get(&Identifier::parse("minecraft:water").unwrap())
            .expect("water tag present"),
        &[FLOWING_WATER_RAW_ID, WATER_RAW_ID]
    );
    assert_eq!(
        fluid_tags
            .get(&Identifier::parse("minecraft:lava").unwrap())
            .expect("lava tag present"),
        &[FLOWING_LAVA_RAW_ID, LAVA_RAW_ID]
    );
}

#[test]
fn solaris_required_client_tags_bind_pickaxe_progression_blocks() {
    let items = crate::items::solaris_required_items();
    let blocks = crate::blocks::solaris_required_blocks_report();
    let tags = solaris_required_client_tags(&items, &blocks);
    let block_tags = tags
        .registries
        .get(&Identifier::parse("minecraft:block").unwrap())
        .expect("block tag registry present");
    let pickaxe = block_tags
        .get(&Identifier::parse("minecraft:mineable/pickaxe").unwrap())
        .expect("pickaxe tag present");

    for block_id in [1, 12, 44, 45] {
        assert!(
            pickaxe.contains(&block_id),
            "pickaxe tag must contain block raw id {block_id}"
        );
    }
}

#[test]
fn solaris_required_client_tags_bind_complete_pickaxe_oracle_membership() {
    let items = crate::items::solaris_required_items();
    let blocks = crate::blocks::solaris_required_blocks_report();
    let tags = solaris_required_client_tags(&items, &blocks);
    let pickaxe = tags
        .registries
        .get(&Identifier::parse("minecraft:block").unwrap())
        .and_then(|block_tags| {
            block_tags.get(&Identifier::parse("minecraft:mineable/pickaxe").unwrap())
        })
        .expect("pickaxe tag present");

    // The local vanilla 26.1.2 tag oracle resolves nested tags to 482 blocks.
    assert_eq!(pickaxe.len(), 482);

    let mut blocks_in_raw_id_order: Vec<_> = blocks.iter().collect();
    blocks_in_raw_id_order.sort_by_key(|block| {
        block
            .states
            .iter()
            .map(|state| state.id)
            .min()
            .expect("embedded block has at least one state")
    });
    for block_name in [
        "minecraft:sandstone",
        "minecraft:iron_door",
        "minecraft:mud_bricks",
        "minecraft:copper_door",
        "minecraft:white_shulker_box",
    ] {
        let raw_id = blocks_in_raw_id_order
            .iter()
            .position(|block| block.id.as_str() == block_name)
            .and_then(|raw_id| i32::try_from(raw_id).ok())
            .unwrap_or_else(|| panic!("embedded block {block_name} missing"));
        assert!(
            pickaxe.contains(&raw_id),
            "pickaxe tag must contain {block_name}"
        );
    }
}

#[test]
fn solaris_required_client_tags_bind_common_survival_tools() {
    let items = crate::items::solaris_required_items();
    let blocks = crate::blocks::solaris_required_blocks_report();
    let tags = solaris_required_client_tags(&items, &blocks);
    let block_tags = tags
        .registries
        .get(&Identifier::parse("minecraft:block").unwrap())
        .expect("block tag registry present");
    let mut blocks_in_raw_id_order: Vec<_> = blocks.iter().collect();
    blocks_in_raw_id_order.sort_by_key(|block| {
        block
            .states
            .iter()
            .map(|state| state.id)
            .min()
            .expect("embedded block has at least one state")
    });

    for (tag, block_names) in [
        (
            "minecraft:mineable/axe",
            &["minecraft:oak_planks", "minecraft:oak_log"][..],
        ),
        (
            "minecraft:mineable/shovel",
            &["minecraft:grass_block", "minecraft:dirt"][..],
        ),
        ("minecraft:mineable/hoe", &["minecraft:oak_leaves"][..]),
    ] {
        let entries = block_tags
            .get(&Identifier::parse(tag).unwrap())
            .unwrap_or_else(|| panic!("{tag} tag present"));
        for block_name in block_names {
            let raw_id = blocks_in_raw_id_order
                .iter()
                .position(|block| block.id.as_str() == *block_name)
                .and_then(|raw_id| i32::try_from(raw_id).ok())
                .unwrap_or_else(|| panic!("embedded block {block_name} missing"));
            assert!(
                entries.contains(&raw_id),
                "{tag} must contain {block_name} (raw id {raw_id})"
            );
        }
    }
}

#[test]
fn solaris_required_client_tags_bind_complete_axe_oracle_membership() {
    let items = crate::items::solaris_required_items();
    let blocks = crate::blocks::solaris_required_blocks_report();
    let tags = solaris_required_client_tags(&items, &blocks);
    let axe = tags
        .registries
        .get(&Identifier::parse("minecraft:block").unwrap())
        .and_then(|block_tags| {
            block_tags.get(&Identifier::parse("minecraft:mineable/axe").unwrap())
        })
        .expect("axe tag present");

    // The local vanilla 26.1.2 tag oracle resolves nested tags to 286 blocks.
    assert_eq!(axe.len(), 286);

    let mut blocks_in_raw_id_order: Vec<_> = blocks.iter().collect();
    blocks_in_raw_id_order.sort_by_key(|block| {
        block
            .states
            .iter()
            .map(|state| state.id)
            .min()
            .expect("embedded block has at least one state")
    });
    for block_name in [
        "minecraft:oak_fence",
        "minecraft:birch_stairs",
        "minecraft:spruce_trapdoor",
        "minecraft:white_banner",
    ] {
        let raw_id = blocks_in_raw_id_order
            .iter()
            .position(|block| block.id.as_str() == block_name)
            .and_then(|raw_id| i32::try_from(raw_id).ok())
            .unwrap_or_else(|| panic!("embedded block {block_name} missing"));
        assert!(axe.contains(&raw_id), "axe tag must contain {block_name}");
    }
}

#[test]
fn real_cache_263_block_tags_include_cinnabar_when_present() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("data/vanilla");
    let pickaxe_tag = root.join("data/minecraft/tags/block/mineable/pickaxe.json");
    if !pickaxe_tag.is_file() {
        eprintln!(
            "SKIP real_cache_263_block_tags_include_cinnabar_when_present: missing {}",
            pickaxe_tag.display()
        );
        return;
    }

    let data = crate::load(&root).expect("real cache registry data loads");
    let tags = load(&root, &data).expect("real cache tags resolve");
    let cinnabar = load_vanilla_id_index(&root).expect("real cache block registry ids load")["minecraft:block"]
        ["minecraft:cinnabar"];
    assert!(
        tags.contains_raw_id("minecraft:block", "minecraft:mineable/pickaxe", cinnabar),
        "26.3 cinnabar must be in minecraft:mineable/pickaxe"
    );
}
