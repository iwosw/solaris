use super::*;

use std::sync::Arc;

use mc_data::item_components::ItemFacts;
use mc_script::{
    BiomeSpawns, ClayRule as PluginClayRule, ClientLoader, SpawnEntry, SpawnPlacement,
};

fn id(value: &str) -> Identifier {
    Identifier::parse(value).unwrap()
}

fn spawn_rules(biome: &str, group: &str, entity: &str) -> GameplayRules {
    GameplayRules::new(
        vec![BiomeSpawns::new(
            biome,
            BTreeMap::from([(group.to_owned(), vec![SpawnEntry::new(entity, 2, 3, 17)])]),
        )],
        Vec::new(),
        None,
        None,
    )
}

fn bundle(
    owner: &str,
    content: Vec<ClientContentKind>,
    permissions: Vec<ClientPermission>,
) -> ClientBundle {
    ClientBundle::new(
        owner,
        "client",
        "1",
        "client.zip",
        "a".repeat(64),
        1,
        "client.zip".into(),
        Arc::from([0u8]),
        vec![ClientLoader::Native],
        content,
        permissions,
    )
}

fn item(owner: &str) -> CustomItemDefinition {
    CustomItemDefinition {
        id: id(&format!("{owner}:token")),
        carrier: id("minecraft:paper"),
        name: "Token".into(),
        crafting_ingredient: Some(id("minecraft:diamond")),
        facts: ItemFacts {
            max_stack_size: Some(16),
            ..ItemFacts::default()
        },
    }
}

#[test]
fn no_configure_contribution_is_not_a_refusal() {
    let mut registry = CoreRegistry::load(None).unwrap();
    let recipes = registry.recipes.len();
    apply(&mut registry, &DeploymentContribution::default(), None, &[]).unwrap();
    assert_eq!(registry.recipes.len(), recipes);
    assert!(registry.plugin_rules.is_none());
    assert!(registry.plugin_ore_profile.is_none());
    assert!(registry.plugin_client_bundles.is_empty());
    validate_outcome("example", &ContributionOutcome::NoContribution).unwrap();
}
#[test]
fn native_settlement_catalog_admits_items_and_recipes_with_no_package() {
    let mut registry = CoreRegistry::load(None).unwrap();
    apply(&mut registry, &DeploymentContribution::default(), None, &[]).unwrap();
    // Six native items admitted through the facts authority, not a package.
    assert_eq!(registry.plugin_item_ids.len(), 6);
    for item in registry.plugin_items() {
        assert_eq!(item.id.namespace(), "solaris-settlements");
        assert!(
            std::ptr::eq(item, registry.item_facts.custom(&item.id).unwrap()),
            "native definitions live in the canonical native item-facts authority",
        );
    }
    // Fourteen bases (eight plain + six graded production + six
    // manufacturing) expanded to 56 native work recipes, validated by the
    // same host conversion the deployment path uses.
    assert_eq!(registry.plugin_work_recipe_ids.len(), 56);
    for id in registry.plugin_work_recipe_ids.clone() {
        let recipe = registry
            .item_facts
            .work_recipe(&id)
            .unwrap_or_else(|| panic!("native work recipe missing from native authority: {id}"));
        assert_eq!(&recipe.id, &id);
        assert!(recipe.duration_ticks > 0);
    }
}

#[test]
fn deployment_claim_on_native_identity_is_refused_not_merged() {
    let mut registry = CoreRegistry::load(None).unwrap();
    apply(&mut registry, &DeploymentContribution::default(), None, &[]).unwrap();
    let native_item = registry.plugin_items().next().unwrap().clone();
    // A package re-declaring a native identity collides in the table builder.
    // Probe on a clone: `mem::take` would leave the registry empty and the
    // work-recipe lookup below would fail on the missing table, not the refusal.
    let error = registry
        .item_facts
        .clone()
        .with_custom_items([native_item.clone()], &registry.items)
        .unwrap_err()
        .to_string();
    assert!(error.contains(&native_item.id.to_string()));
    let native_recipe_id = registry.plugin_work_recipe_ids[0].clone();
    let native_recipe = registry
        .item_facts
        .work_recipe(&native_recipe_id)
        .unwrap()
        .clone();
    let error = registry
        .item_facts
        .clone()
        .with_work_recipes([native_recipe], &registry.items)
        .unwrap_err()
        .to_string();
    assert!(error.contains(&native_recipe_id.to_string()));
}

#[test]
fn configure_refusal_is_a_startup_error_with_package_identity() {
    let refusal = ContributionOutcome::Refused(mc_plugin_host::ContributionRefusal::InvalidItem(
        "invalid carrier".into(),
    ));
    let error = validate_outcome("example", &refusal)
        .unwrap_err()
        .to_string();
    assert!(error.contains("example"));
    assert!(error.contains("invalid carrier"));
}

#[test]
fn two_gameplay_rule_providers_fail_instead_of_selecting_one() {
    let rules = spawn_rules("minecraft:plains", "creature", "minecraft:cow");
    assert!(resolve_rules(std::iter::empty()).unwrap().is_none());
    assert_eq!(
        resolve_rules([("one", &rules)].into_iter())
            .unwrap()
            .unwrap()
            .0,
        "one"
    );
    let error = resolve_rules([("one", &rules), ("two", &rules)].into_iter())
        .unwrap_err()
        .to_string();
    assert!(error.contains("one"));
    assert!(error.contains("two"));
}

#[test]
fn unmaterialized_spawn_placement_is_refused() {
    let registry = CoreRegistry::load(None).unwrap();
    let rules = GameplayRules::new(
        Vec::new(),
        Vec::new(),
        None,
        Some(SpawnPlacement::new(1, 1, 1)),
    );
    assert!(
        validate_rules(&registry, "example", &rules)
            .unwrap_err()
            .to_string()
            .contains("spawn placement")
    );
}

#[test]
fn spawning_requires_native_biome_and_spawnable_blueprint() {
    let registry = CoreRegistry::load(None).unwrap();
    for rules in [
        spawn_rules("example:absent", "creature", "minecraft:cow"),
        spawn_rules("minecraft:plains", "creature", "example:absent"),
        spawn_rules("minecraft:plains", "creature", "minecraft:item"),
        spawn_rules("minecraft:plains", "creature", "minecraft:zombie"),
    ] {
        assert!(validate_rules(&registry, "example", &rules).is_err());
    }
    validate_rules(
        &registry,
        "example",
        &spawn_rules("minecraft:plains", "creature", "minecraft:cow"),
    )
    .unwrap();
}

#[test]
fn biome_contribution_replaces_groups_without_retaining_omitted_vanilla_entries() {
    let mut registry = CoreRegistry::load(None).unwrap();
    let biome = id("minecraft:plains");
    let rules = spawn_rules("minecraft:plains", "creature", "minecraft:cow");
    validate_rules(&registry, "example", &rules).unwrap();
    apply_spawning(&mut registry, &rules).unwrap();
    let entries = registry.biome_spawns.exact_entries(&biome, "creature");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].entity_type, id("minecraft:cow"));
    assert_eq!(entries[0].min_count, 2);
    assert_eq!(entries[0].max_count, 3);
    assert_eq!(entries[0].weight, 17);
    for group in [
        "monster",
        "ambient",
        "axolotls",
        "underground_water_creature",
        "water_creature",
        "water_ambient",
    ] {
        assert!(
            registry
                .biome_spawns
                .exact_entries(&biome, group)
                .is_empty()
        );
    }
}

#[test]
fn native_generator_receives_the_declared_ore_tree_and_clay_policy() {
    let mut registry = CoreRegistry::load(None).unwrap();
    registry.plugin_rules = Some(GameplayRules::new(
        Vec::new(),
        vec![mc_script::TreeRule::new(
            vec!["minecraft:plains".into()],
            23,
            0.25,
        )],
        Some(PluginClayRule::new(11, 1, 2, 3)),
        None,
    ));
    registry.plugin_ore_profile = Some(PluginWorldgenOreProfile::RealisticDeposits);
    let terrain = TerrainGenerator::new(7, Arc::clone(&registry.blocks));
    let terrain = apply_worldgen(&registry, terrain).unwrap();
    assert_eq!(terrain.ore_generation_profile(), "realistic_deposits");
}

#[test]
fn unavailable_tree_biome_fails_before_generation() {
    let mut registry = CoreRegistry::load(None).unwrap();
    registry.plugin_rules = Some(GameplayRules::new(
        Vec::new(),
        vec![mc_script::TreeRule::new(
            vec!["example:absent".into()],
            1,
            0.0,
        )],
        None,
        None,
    ));
    let terrain = TerrainGenerator::new(7, Arc::clone(&registry.blocks));
    let error = apply_worldgen(&registry, terrain)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("example:absent"));
}

#[test]
fn client_blocks_need_permission_and_a_valid_carrier_declaration() {
    let blocks = bundle(
        "example",
        vec![ClientContentKind::Blocks],
        vec![ClientPermission::RegisterBlocks],
    );
    validate_client_bundles(std::slice::from_ref(&blocks)).unwrap();
    // The fixture artifact carries no carrier block index: unsupported shape.
    assert!(mc_loader::declared_carrier_block(&blocks).is_err());
    let unpermitted = bundle("example", vec![ClientContentKind::Blocks], Vec::new());
    assert!(validate_client_bundles(&[unpermitted]).is_err());
    let sounds = bundle("example", vec![ClientContentKind::Sounds], Vec::new());
    assert!(validate_client_bundles(&[sounds]).is_err());
}

#[test]
fn verified_client_package_catalog_is_retained_in_native_registry() {
    let mut registry = CoreRegistry::load(None).unwrap();
    let bundles = [bundle(
        "example",
        vec![ClientContentKind::Sounds],
        vec![ClientPermission::PlaySounds],
    )];
    apply(
        &mut registry,
        &DeploymentContribution::default(),
        None,
        &bundles,
    )
    .unwrap();
    assert_eq!(registry.plugin_client_bundles, bundles);
    assert!(Arc::ptr_eq(
        &registry.plugin_client_bundles[0].artifact_bytes_arc(),
        &bundles[0].artifact_bytes_arc(),
    ));
}

#[test]
fn custom_items_require_their_own_authorized_item_bundle() {
    let definition = item("example");
    assert!(validate_item("example", &definition, &[]).is_err());
    let foreign = bundle(
        "other",
        vec![ClientContentKind::Items],
        vec![ClientPermission::RegisterItems],
    );
    assert!(validate_item("example", &definition, &[foreign]).is_err());
    let owned = bundle(
        "example",
        vec![ClientContentKind::Items],
        vec![ClientPermission::RegisterItems],
    );
    validate_item("example", &definition, std::slice::from_ref(&owned)).unwrap();
    assert!(validate_item("other", &definition, &[owned]).is_err());
}

#[test]
fn authored_physical_item_is_admitted_and_unsupported_predicates_fail_closed() {
    let mut definition = item("example");
    definition.facts.max_stack_size = Some(1);
    definition.facts.max_damage = Some(320);
    definition.facts.physical_profile = Some(Box::new(authored_melee_profile()));
    let owned = bundle(
        "example",
        vec![ClientContentKind::Items],
        vec![ClientPermission::RegisterItems],
    );
    validate_item("example", &definition, std::slice::from_ref(&owned)).unwrap();
    definition
        .facts
        .physical_profile
        .as_deref_mut()
        .unwrap()
        .attacks[0]
        .brace = Some(mc_data::item_components::BracePredicate {
        on_ground: true,
        max_speed_blocks_per_tick: 0.0,
    });
    assert!(
        validate_item("example", &definition, std::slice::from_ref(&owned))
            .unwrap_err()
            .to_string()
            .contains("brace/charge")
    );
    let attack = &mut definition
        .facts
        .physical_profile
        .as_deref_mut()
        .unwrap()
        .attacks[0];
    attack.brace = None;
    attack.charge = Some(mc_data::item_components::ChargePredicate {
        min_speed_blocks_per_tick: 0.5,
    });
    assert!(
        validate_item("example", &definition, std::slice::from_ref(&owned))
            .unwrap_err()
            .to_string()
            .contains("brace/charge")
    );
}

#[test]
fn loader_item_iterator_borrows_the_native_facts_authority() {
    let mut registry = CoreRegistry::load(None).unwrap();
    let definition = item("example");
    registry.plugin_item_ids.push(definition.id.clone());
    registry.item_facts = std::mem::take(&mut registry.item_facts)
        .with_custom_items([definition], &registry.items)
        .unwrap();
    let mut admitted = registry.plugin_items();
    let borrowed = admitted.next().unwrap();
    assert_eq!(borrowed.id, id("example:token"));
    assert!(std::ptr::eq(
        borrowed,
        registry.item_facts.custom(&borrowed.id).unwrap(),
    ));
    assert!(admitted.next().is_none());
}

#[test]
fn native_world_identity_includes_materialized_item_facts_and_not_discovery_order() {
    let mut registry = CoreRegistry::load(None).unwrap();
    let first = item("first");
    let mut second = item("second");
    second.facts.max_stack_size = Some(8);
    registry.plugin_item_ids = vec![first.id.clone(), second.id.clone()];
    registry.item_facts = std::mem::take(&mut registry.item_facts)
        .with_custom_items([first, second], &registry.items)
        .unwrap();
    let original = world_identity(&registry).unwrap();
    registry.plugin_item_ids.reverse();
    assert_eq!(original, world_identity(&registry).unwrap());
    let decoded: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(decoded["items"][0]["id"], "first:token");
    assert_eq!(decoded["items"][1]["max_stack_size"], 8);
}

#[test]
fn native_world_identity_changes_with_gameplay_rules_and_ore_policy() {
    let mut registry = CoreRegistry::load(None).unwrap();
    let baseline = world_identity(&registry).unwrap();
    registry.plugin_rules = Some(spawn_rules("minecraft:plains", "creature", "minecraft:cow"));
    let rules_identity = world_identity(&registry).unwrap();
    assert_ne!(baseline, rules_identity);
    registry.plugin_ore_profile = Some(PluginWorldgenOreProfile::RealisticDeposits);
    assert_ne!(rules_identity, world_identity(&registry).unwrap());
}

#[test]
fn ui_artifact_catalog_does_not_select_native_world_semantics() {
    let mut registry = CoreRegistry::load(None).unwrap();
    let baseline = world_identity(&registry).unwrap();
    registry.plugin_client_bundles.push(bundle(
        "example",
        vec![ClientContentKind::Sounds],
        vec![ClientPermission::PlaySounds],
    ));
    assert_eq!(baseline, world_identity(&registry).unwrap());
}

#[test]
fn native_world_identity_contains_complete_work_recipe_semantics() {
    use mc_data::recipes::work::{
        WorkRecipe, WorkRecipeComponents, WorkRecipeInput, WorkRecipeStack,
    };

    let mut registry = CoreRegistry::load(None).unwrap();
    let baseline = world_identity(&registry).unwrap();
    let recipe = WorkRecipe {
        id: id("example:planks"),
        revision: "one".into(),
        station: id("minecraft:crafting_table"),
        duration_ticks: 23,
        inputs: vec![WorkRecipeInput {
            item: Some(id("minecraft:oak_log")),
            tag: None,
            alternatives: Vec::new(),
            count: 1,
            components: None,
        }],
        outputs: vec![WorkRecipeStack {
            item: id("minecraft:oak_planks"),
            count: 4,
            components: WorkRecipeComponents::default(),
        }],
        remainders: Vec::new(),
        tools: Vec::new(),
    };
    registry.plugin_work_recipe_ids.push(recipe.id.clone());
    registry.item_facts = std::mem::take(&mut registry.item_facts)
        .with_work_recipes([recipe], &registry.items)
        .unwrap();
    let admitted = world_identity(&registry).unwrap();
    assert_ne!(baseline, admitted);
    let decoded: serde_json::Value = serde_json::from_slice(&admitted).unwrap();
    assert_eq!(decoded["work_recipes"][0]["duration_ticks"], 23);
    assert_eq!(decoded["work_recipes"][0]["outputs"][0]["count"], 4);
    registry.plugin_work_recipe_ids.push(id("example:missing"));
    assert!(world_identity(&registry).is_err());
}

#[test]
fn native_world_identity_excludes_client_only_packages() {
    let mut registry = CoreRegistry::load(None).unwrap();
    let baseline = world_identity(&registry).unwrap();
    registry.plugin_client_bundles.push(bundle(
        "example",
        vec![ClientContentKind::Sounds],
        vec![ClientPermission::PlaySounds],
    ));
    assert_eq!(world_identity(&registry).unwrap(), baseline);

    registry.plugin_rules = Some(spawn_rules("minecraft:plains", "creature", "minecraft:cow"));
    assert_ne!(world_identity(&registry).unwrap(), baseline);
    registry.plugin_rules = None;
    registry.plugin_ore_profile = Some(PluginWorldgenOreProfile::RealisticDeposits);
    assert_ne!(world_identity(&registry).unwrap(), baseline);
}

fn authored_melee_profile() -> mc_data::item_components::PhysicalItemProfile {
    serde_json::from_str(
        r#"{
            "attacks":[{
                "mode":"melee","damage_kind":"cut","base_damage_hp":5.0,
                "windup_ticks":6,"active_ticks":2,"recovery_ticks":10,
                "min_range_blocks":0.0,"reach_blocks":1.9,"stamina_cost":12.0,
                "shape":{"kind":"cone","half_angle_degrees":45.0},
                "projectile":null,"consumption":null,"brace":null,"charge":null,
                "two_handed":false,"wear":1
            }],
            "armor":null,"shield":null,
            "stamina_capacity":100.0,"stamina_recovery_per_tick":0.5
        }"#,
    )
    .unwrap()
}

#[test]
fn native_world_identity_retains_complete_authored_physical_profile() {
    let profile = authored_melee_profile();
    let identity = |profile| {
        let mut registry = CoreRegistry::load(None).unwrap();
        let mut definition = item("example");
        definition.facts.max_stack_size = Some(1);
        definition.facts.max_damage = Some(320);
        definition.facts.weapon = true;
        definition.facts.physical_profile = Some(Box::new(profile));
        registry.plugin_item_ids.push(definition.id.clone());
        registry.item_facts = std::mem::take(&mut registry.item_facts)
            .with_custom_items([definition], &registry.items)
            .unwrap();
        world_identity(&registry).unwrap()
    };
    let original = identity(profile.clone());
    let decoded: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(
        decoded["items"][0]["physical_profile"]["attacks"][0]["windup_ticks"],
        6
    );
    assert_eq!(
        decoded["items"][0]["physical_profile"]["stamina_capacity"],
        100.0
    );
    let mut changed = profile;
    changed.attacks[0].windup_ticks = 7;
    assert_ne!(original, identity(changed));
}
