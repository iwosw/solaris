use super::*;
use serde_json::json;

fn fixture(root: &Path, id: &str, value: Value) {
    let (namespace, name) = id.split_once(':').unwrap();
    let path = root
        .join("data")
        .join(namespace)
        .join("advancement")
        .join(format!("{name}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn definition(parent: Option<&str>) -> Value {
    let mut value = json!({"criteria":{"a":{"trigger":"minecraft:tick"},"b":{"trigger":"minecraft:tick"},"c":{"trigger":"minecraft:tick"}},"requirements":[["a","b"],["c"]]});
    if let Some(parent) = parent {
        value["parent"] = json!(parent);
    }
    value
}

fn evaluate(trigger: &str, conditions: Value, event: Value, inventory: &[Value]) -> Evaluation {
    evaluate_criterion(
        &Criterion {
            trigger: trigger.to_string(),
            conditions,
        },
        &EventContext {
            trigger,
            event: &event,
            inventory,
        },
    )
}

#[test]
fn tree_loads_all_namespaces_and_recipe_subdirectories_deterministically() {
    let dir = tempfile::tempdir().unwrap();
    fixture(
        dir.path(),
        "minecraft:recipes/root",
        json!({"criteria":{"impossible":{"trigger":"minecraft:impossible"}}}),
    );
    fixture(
        dir.path(),
        "custom:recipes/tools/axe",
        json!({"parent":"minecraft:recipes/root","criteria":{"inventory":{"trigger":"minecraft:inventory_changed","conditions":{"items":[{"items":"minecraft:oak_planks"}]}}},"rewards":{"recipes":["custom:tools/axe"],"experience":3}}),
    );
    std::fs::create_dir_all(dir.path().join("data/unrelated/recipe")).unwrap();
    let tree = AdvancementTree::load(dir.path()).unwrap();
    assert_eq!(
        tree.definitions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["custom:recipes/tools/axe", "minecraft:recipes/root"]
    );
    assert_eq!(
        tree.trigger_index["minecraft:inventory_changed"],
        vec![(
            "custom:recipes/tools/axe".to_string(),
            "inventory".to_string()
        )]
    );
    assert_eq!(
        tree.definitions["custom:recipes/tools/axe"].rewards.recipes,
        ["custom:tools/axe"]
    );
    assert_eq!(tree, AdvancementTree::load(dir.path()).unwrap());
}

#[test]
fn requirements_are_and_of_or_groups() {
    let mut definition: AdvancementDefinition = serde_json::from_value(definition(None)).unwrap();
    validate_definition("test:tree", &mut definition).unwrap();
    for (names, completed) in [
        (vec!["a"], false),
        (vec!["a", "b"], false),
        (vec!["c"], false),
        (vec!["a", "c"], true),
        (vec!["b", "c"], true),
    ] {
        assert_eq!(
            definition.requirements_met(&names.into_iter().map(str::to_string).collect()),
            completed
        );
    }
}

#[test]
fn omitted_requirements_require_every_criterion() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = definition(None);
    value.as_object_mut().unwrap().remove("requirements");
    fixture(dir.path(), "test:root", value);
    let tree = AdvancementTree::load(dir.path()).unwrap();
    assert_eq!(
        tree.definitions["test:root"].requirements,
        vec![vec!["a"], vec!["b"], vec!["c"]]
    );
}

#[test]
fn loader_rejects_bad_requirements_missing_parents_and_cycles() {
    for requirements in [
        json!([]),
        json!([[]]),
        json!([["unknown"]]),
        json!([["a", "b"]]),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut value = definition(None);
        value["requirements"] = requirements;
        fixture(dir.path(), "test:root", value);
        assert!(AdvancementTree::load(dir.path()).is_err());
    }
    let missing = tempfile::tempdir().unwrap();
    fixture(
        missing.path(),
        "test:child",
        definition(Some("test:absent")),
    );
    assert!(AdvancementTree::load(missing.path()).is_err());
    let cycle = tempfile::tempdir().unwrap();
    fixture(cycle.path(), "test:a", definition(Some("test:b")));
    fixture(cycle.path(), "test:b", definition(Some("test:a")));
    assert!(AdvancementTree::load(cycle.path()).is_err());
}

#[test]
fn occupied_slots_are_derived_but_capacity_requires_explicit_facts() {
    let inventory = [json!({"id":"minecraft:stone","count":1})];
    assert!(
        evaluate(
            "minecraft:inventory_changed",
            json!({"slots":{"occupied":{"min":1}}}),
            json!({}),
            &inventory
        )
        .matches
    );
    assert!(
        !evaluate(
            "minecraft:inventory_changed",
            json!({"slots":{"full":{"min":1}}}),
            json!({}),
            &inventory
        )
        .matches
    );
    assert!(
        evaluate(
            "minecraft:inventory_changed",
            json!({"slots":{"full":{"min":1},"empty":{"max":35}}}),
            json!({"slots":{"occupied":1,"full":1,"empty":35}}),
            &inventory
        )
        .matches
    );
}

#[test]
fn support_report_identifies_function_executor_gap() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = definition(None);
    value["rewards"] = json!({"function":"test:reward"});
    fixture(dir.path(), "test:root", value);
    let report = AdvancementTree::load(dir.path()).unwrap().support_report();
    assert_eq!(report.unsupported_functions["test:root"], "test:reward");
}

#[test]
#[cfg(unix)]
fn loader_rejects_symlinked_advancement_sources() {
    let dir = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fixture(external.path(), "test:root", definition(None));
    std::fs::create_dir_all(dir.path().join("data/test")).unwrap();
    std::os::unix::fs::symlink(
        external.path().join("data/test/advancement"),
        dir.path().join("data/test/advancement"),
    )
    .unwrap();
    assert!(AdvancementTree::load(dir.path()).is_err());
}

#[test]
fn inventory_items_tags_and_count_require_actual_evidence() {
    let predicate = json!({"items":[{"items":"#minecraft:logs","count":{"min":2}},{"items":["minecraft:stone"]}]});
    let inventory = [
        json!({"id":"minecraft:oak_log","count":2,"tags":["minecraft:logs"]}),
        json!({"id":"minecraft:stone","count":1,"tags":[]}),
    ];
    assert!(
        evaluate(
            "minecraft:inventory_changed",
            predicate.clone(),
            json!({}),
            &inventory
        )
        .matches
    );
    assert!(
        !evaluate(
            "minecraft:inventory_changed",
            predicate.clone(),
            json!({}),
            &inventory[..1]
        )
        .matches
    );
    assert!(
        !evaluate(
            "minecraft:inventory_changed",
            predicate,
            json!({}),
            &[
                json!({"id":"minecraft:oak_log","count":2}),
                inventory[1].clone()
            ]
        )
        .matches
    );
    assert!(
        !evaluate(
            "minecraft:inventory_changed",
            json!({"items":[{}]}),
            json!({}),
            &[json!({"id":"minecraft:stone","count":0})]
        )
        .matches
    );
}

#[test]
fn dimension_recipe_and_unconditional_trigger_matching() {
    assert!(
        evaluate(
            "minecraft:changed_dimension",
            json!({"from":"minecraft:overworld","to":"minecraft:the_nether"}),
            json!({"from":"minecraft:overworld","to":"minecraft:the_nether"}),
            &[]
        )
        .matches
    );
    assert!(
        !evaluate(
            "minecraft:changed_dimension",
            json!({"to":"minecraft:the_nether"}),
            json!({}),
            &[]
        )
        .matches
    );
    assert!(
        evaluate(
            "minecraft:recipe_crafted",
            json!({"recipe_id":"minecraft:bread"}),
            json!({"recipe":"minecraft:bread"}),
            &[]
        )
        .matches
    );
    assert!(!evaluate("minecraft:impossible", json!({}), json!({}), &[]).matches);
    assert!(evaluate("minecraft:tick", Value::Null, json!({}), &[]).matches);
    let criterion = Criterion {
        trigger: "minecraft:tick".to_string(),
        conditions: Value::Null,
    };
    assert!(
        !evaluate_criterion(
            &criterion,
            &EventContext {
                trigger: "minecraft:location",
                event: &Value::Null,
                inventory: &[]
            }
        )
        .matches
    );
}

#[test]
fn entity_location_equipment_flags_and_components() {
    let predicate = json!({"entity":[{"condition":"minecraft:entity_properties","entity":"this","predicate":{"type":"minecraft:frog","flags":{"is_baby":false},"components":{"minecraft:frog/variant":"minecraft:warm"},"equipment":{"head":{"items":"minecraft:carved_pumpkin"}},"location":{"dimension":"minecraft:overworld","biomes":["minecraft:swamp"],"position":{"y":{"min":60,"max":100}}}}}]});
    let event = json!({"entity":{"type":"minecraft:frog","flags":{"is_baby":false},"components":{"minecraft:frog/variant":"minecraft:warm"},"equipment":{"head":{"id":"minecraft:carved_pumpkin","count":1}},"location":{"dimension":"minecraft:overworld","biomes":["minecraft:swamp"],"position":{"x":0,"y":64,"z":0}}}});
    let result = evaluate(
        "minecraft:player_killed_entity",
        predicate.clone(),
        event,
        &[],
    );
    assert!(result.matches, "{result:?}");
    assert!(result.unsupported.is_empty());
    assert!(
        !evaluate(
            "minecraft:player_killed_entity",
            predicate,
            json!({"entity":{"type":"minecraft:frog"}}),
            &[]
        )
        .matches
    );
}

#[test]
fn damage_predicates_and_negative_tags_fail_on_missing_evidence() {
    let predicate = json!({"damage":{"blocked":true,"dealt":{"min":10},"type":{"tags":[{"id":"minecraft:is_projectile","expected":true},{"id":"minecraft:is_fire","expected":false}],"direct_entity":{"type":"minecraft:arrow"}}}});
    let mut event = json!({"damage":{"blocked":true,"dealt":12,"type":{"tags":["minecraft:is_projectile"],"direct_entity":{"type":"minecraft:arrow"}}}});
    assert!(
        evaluate(
            "minecraft:player_hurt_entity",
            predicate.clone(),
            event.clone(),
            &[]
        )
        .matches
    );
    event["damage"]["type"]
        .as_object_mut()
        .unwrap()
        .remove("tags");
    assert!(!evaluate("minecraft:player_hurt_entity", predicate, event, &[]).matches);
}

#[test]
fn block_tool_conditions_and_enchantment_predicates() {
    let predicate = json!({"location":[{"condition":"minecraft:location_check","predicate":{"block":{"blocks":"minecraft:campfire","state":{"lit":"true"}},"smokey":true}},{"condition":"minecraft:match_tool","predicate":{"items":"minecraft:diamond_axe","predicates":{"minecraft:enchantments":[{"enchantments":"minecraft:silk_touch","levels":{"min":1}}]}}}]});
    let event = json!({"location":{"block":{"id":"minecraft:campfire","state":{"lit":"true"}},"smokey":true},"item":{"id":"minecraft:diamond_axe","count":1,"components":{"minecraft:enchantments":{"minecraft:silk_touch":1}}}});
    let result = evaluate("minecraft:item_used_on_block", predicate, event, &[]);
    assert!(result.matches, "{result:?}");
}

#[test]
fn effect_distance_and_distinct_recipe_ingredients() {
    assert!(evaluate("minecraft:effects_changed", json!({"effects":{"minecraft:levitation":{"amplifier":{"min":0},"duration":{"min":20}}}}), json!({"effects":{"minecraft:levitation":{"amplifier":0,"duration":40}}}), &[]).matches);
    assert!(
        evaluate(
            "minecraft:levitation",
            json!({"distance":{"y":{"min":50}},"duration":{"max":200}}),
            json!({"distance":{"y":51},"duration":100}),
            &[]
        )
        .matches
    );
    let conditions = json!({"recipe_id":"minecraft:decorated_pot","ingredients":[{"items":"#minecraft:decorated_pot_sherds"},{"items":"#minecraft:decorated_pot_sherds"}]});
    let stack = json!({"id":"minecraft:angler_pottery_sherd","count":1,"tags":["minecraft:decorated_pot_sherds"]});
    assert!(
        !evaluate(
            "minecraft:recipe_crafted",
            conditions.clone(),
            json!({"recipe_id":"minecraft:decorated_pot","ingredients":[stack.clone()]}),
            &[]
        )
        .matches
    );
    assert!(
        evaluate(
            "minecraft:recipe_crafted",
            conditions,
            json!({"recipe_id":"minecraft:decorated_pot","ingredients":[stack.clone(),stack]}),
            &[]
        )
        .matches
    );
}

#[test]
fn unsupported_never_grants_even_inside_inverted_and_matching_or() {
    let conditions = json!({"player":[{"condition":"minecraft:any_of","terms":[{"condition":"minecraft:entity_properties","entity":"this","predicate":{"type":"minecraft:player"}},{"condition":"minecraft:inverted","term":{"condition":"minecraft:entity_properties","entity":"this","predicate":{"type_specific":{"type":"minecraft:player","looking_at":{"type":"minecraft:ender_dragon"}}}}}]}]});
    let result = evaluate(
        "minecraft:using_item",
        conditions,
        json!({"player":{"type":"minecraft:player"}}),
        &[],
    );
    assert!(!result.matches);
    assert!(
        result
            .unsupported
            .iter()
            .any(|path| path.ends_with("type_specific"))
    );
    assert!(!evaluate("minecraft:tick", json!({"unknown":{}}), json!({}), &[]).matches);
    assert!(!evaluate("custom:unknown", json!({}), json!({}), &[]).matches);
    let offset = evaluate(
        "minecraft:placed_block",
        json!({"location":[{"condition":"minecraft:location_check","offsetY":1,"predicate":{"block":{"blocks":"minecraft:stone"}}}]}),
        json!({"location":{"block":{"id":"minecraft:stone"}}}),
        &[],
    );
    assert!(!offset.matches);
    assert!(
        offset
            .unsupported
            .iter()
            .any(|field| field.ends_with("offsetY"))
    );
}

#[test]
fn inverted_missing_entity_is_not_positive_evidence() {
    let conditions = json!({"player":[{"condition":"minecraft:inverted","term":{"condition":"minecraft:entity_properties","entity":"this","predicate":{"equipment":{"head":{"items":"minecraft:diamond_helmet"}}}}}]});
    assert!(!evaluate("minecraft:tick", conditions, json!({}), &[]).matches);
}

#[test]
#[ignore = "requires locally extracted Mojang 26.1.2 advancement data"]
fn vanilla_26_1_2_tree_count_and_supported_subset() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let tree = AdvancementTree::load(&root).unwrap();
    assert_eq!(tree.definitions.len(), 1617);
    assert_eq!(
        tree.definitions
            .keys()
            .filter(|id| id.starts_with("minecraft:recipes/"))
            .count(),
        1492
    );
    let report = tree.support_report();
    assert_eq!(report.trigger_counts.len(), 54);
    assert_eq!(report.trigger_counts["minecraft:inventory_changed"], 1668);
    assert_eq!(report.trigger_counts["minecraft:recipe_unlocked"], 1491);
    assert_eq!(report.unsupported_criteria, 7);
    assert!(
        report
            .unsupported_fields
            .keys()
            .all(|field| field.ends_with("type_specific")
                || field.ends_with("offsetX")
                || field.ends_with("offsetY")
                || field.ends_with("offsetZ")),
        "{report:?}"
    );
}

#[test]
fn passenger_predicate_matches_any_authoritative_passenger_without_recursive_vehicle_cycles() {
    let predicate = json!({"player":[{"condition":"minecraft:entity_properties","entity":"this","predicate":{"vehicle":{"type":"minecraft:oak_boat","passenger":{"type":"minecraft:goat"}}}}]});
    let event = json!({"player":{"type":"minecraft:player","vehicle":{"type":"minecraft:oak_boat","passenger":[{"type":"minecraft:player"},{"type":"minecraft:goat"}]}}});
    assert!(evaluate("minecraft:location", predicate.clone(), event, &[]).matches);
    let empty = json!({"player":{"type":"minecraft:player","vehicle":{"type":"minecraft:oak_boat","passenger":[]}}});
    assert!(!evaluate("minecraft:location", predicate, empty, &[]).matches);
}

#[test]
fn known_empty_equipment_is_false_not_missing_but_unknown_predicates_stay_fenced() {
    let item = json!({"id":"minecraft:air","count":0,"tags":[],"components":{}});
    let inventory = json!({"items":[{"items":"minecraft:air"}]});
    assert!(
        !evaluate(
            "minecraft:inventory_changed",
            inventory,
            json!({}),
            std::slice::from_ref(&item)
        )
        .matches
    );
    let event = json!({"player":{"type":"minecraft:player","equipment":{"head":item}}});
    let inverted = json!({"player":[{"condition":"minecraft:inverted","term":{"condition":"minecraft:entity_properties","entity":"this","predicate":{"equipment":{"head":{"items":"#minecraft:piglin_safe_armor"}}}}}]});
    assert!(evaluate("minecraft:location", inverted, event.clone(), &[]).matches);
    let unknown = json!({"player":[{"condition":"minecraft:inverted","term":{"condition":"minecraft:entity_properties","entity":"this","predicate":{"equipment":{"head":{"future_unknown":{}}}}}}]});
    let result = evaluate("minecraft:location", unknown, event, &[]);
    assert!(!result.matches);
    assert!(!result.unsupported.is_empty());
}

#[test]
fn jukebox_playable_empty_predicate_requires_authoritative_song_and_rejects_unknown_fields() {
    let predicate = json!({"item":{"predicates":{"minecraft:jukebox_playable":{}}}});
    let item = json!({"id":"minecraft:music_disc_cat","count":1,"components":{"minecraft:jukebox_playable":{"song":"minecraft:cat"}}});
    assert!(
        evaluate(
            "minecraft:consume_item",
            predicate.clone(),
            json!({"item":item}),
            &[]
        )
        .matches
    );
    assert!(
        !evaluate(
            "minecraft:consume_item",
            predicate,
            json!({"item":{"id":"minecraft:paper","count":1,"components":{}}}),
            &[]
        )
        .matches
    );
    let song =
        json!({"item":{"predicates":{"minecraft:jukebox_playable":{"song":"minecraft:cat"}}}});
    assert!(evaluate("minecraft:consume_item", song, json!({"item":item}), &[]).matches);
    let unknown = json!({"item":{"predicates":{"minecraft:jukebox_playable":{"future":true}}}});
    let result = evaluate("minecraft:consume_item", unknown, json!({"item":item}), &[]);
    assert!(!result.matches);
    assert!(!result.unsupported.is_empty());
}

fn dispatch_tree(
    entries: impl IntoIterator<Item = (String, &'static str, Value)>,
) -> AdvancementTree {
    let mut tree = AdvancementTree::default();
    for (id, trigger, conditions) in entries {
        tree.definitions.insert(
            id,
            AdvancementDefinition {
                criteria: BTreeMap::from([(
                    "test".into(),
                    Criterion {
                        trigger: trigger.into(),
                        conditions,
                    },
                )]),
                requirements: vec![vec!["test".into()]],
                ..Default::default()
            },
        );
    }
    tree.rebuild_trigger_index();
    tree
}

#[test]
fn event_candidate_dispatch_preserves_full_evaluator_results() {
    let inventory = "minecraft:inventory_changed";
    let location = "minecraft:location";
    let recipe = "minecraft:recipe_unlocked";
    let conditions = [
        (inventory, json!({"items":[{"items":"minecraft:stone"}]})),
        (inventory, json!({"items":[{"items":"#minecraft:logs"}]})),
        (
            inventory,
            json!({"items":[{"items":["minecraft:stone","minecraft:oak_log"]}]}),
        ),
        (inventory, json!({"items":[{},{"items":"minecraft:stone"}]})),
        (inventory, json!({"items":[]})),
        (inventory, json!({"slots":{"occupied":{"min":1}}})),
        (
            inventory,
            json!({"items":[{"items":"minecraft:stone","unknown":true}]}),
        ),
        (
            inventory,
            json!({"items":[{"items":["minecraft:stone",7]}]}),
        ),
        (recipe, json!({"recipe":"minecraft:crafting_table"})),
        (recipe, json!({})),
        (
            recipe,
            json!({"recipe":"minecraft:crafting_table","unknown":true}),
        ),
        (
            location,
            json!({"player":{"location":{"biomes":"minecraft:plains"}}}),
        ),
        (
            location,
            json!({"player":{"location":{"dimension":"minecraft:the_nether"}}}),
        ),
        (
            location,
            json!({"player":{"location":{"structures":"minecraft:fortress"}}}),
        ),
        (
            location,
            json!({"player":{"location":{"position":{"x":{"min":2,"max":4}}}}}),
        ),
        (
            location,
            json!({"player":{"location":{"biomes":"#minecraft:is_overworld"}}}),
        ),
        (
            location,
            json!({"player":{"stepping_on":{"block":{"blocks":"minecraft:stone"}}}}),
        ),
        (
            location,
            json!({"player":[{"condition":"minecraft:entity_properties","entity":"this","predicate":{"location":{"biomes":["minecraft:plains","minecraft:forest"]}}}]}),
        ),
        (
            location,
            json!({"player":[{"condition":"minecraft:all_of","terms":[{"condition":"minecraft:entity_properties","entity":"this","predicate":{"location":{"biomes":"minecraft:plains"}}}]}]}),
        ),
        (
            location,
            json!({"player":[{"condition":"minecraft:any_of","terms":[{"condition":"minecraft:entity_properties","entity":"this","predicate":{"location":{"biomes":"minecraft:plains"}}},{"condition":"minecraft:entity_properties","entity":"this","predicate":{"location":{"biomes":"minecraft:forest"}}}]}]}),
        ),
        (
            location,
            json!({"player":[{"condition":"minecraft:inverted","term":{"condition":"minecraft:entity_properties","entity":"this","predicate":{"location":{"biomes":"minecraft:forest"}}}}]}),
        ),
        (
            location,
            json!({"player":[{"condition":"minecraft:location_check","predicate":{"biomes":"minecraft:forest"}}]}),
        ),
        (
            location,
            json!({"player":{"location":{"biomes":"minecraft:plains"},"unknown":true}}),
        ),
    ];
    let tree = dispatch_tree(
        conditions
            .into_iter()
            .enumerate()
            .map(|(index, (trigger, conditions))| {
                (format!("test:{index:03}"), trigger, conditions)
            }),
    );
    let mut selected = Vec::new();
    for items in [
        vec![],
        vec![json!({"id":"minecraft:stone","count":1,"tags":[]})],
        vec![json!({"id":"minecraft:oak_log","count":1,"tags":["minecraft:logs"]})],
        vec![json!({"id":"minecraft:stone","count":0,"tags":[]})],
        vec![json!({"id":"minecraft:oak_log","count":1})],
    ] {
        for biome in ["minecraft:plains", "minecraft:forest"] {
            for x in [0, 3, 5] {
                let event = json!({
                    "recipe":"minecraft:crafting_table",
                    "location":{"biomes":["minecraft:forest"]},
                    "player":{"location":{"dimension":"minecraft:overworld","biomes":[biome],"position":{"x":x,"y":64,"z":0}},"stepping_on":{"block":{"id":"minecraft:stone"}}}
                });
                for trigger in [inventory, location, recipe] {
                    let context = EventContext {
                        trigger,
                        event: &event,
                        inventory: &items,
                    };
                    let indexed = &tree.trigger_index[trigger];
                    let expected: Vec<_> = indexed
                        .iter()
                        .filter(|(id, name)| {
                            evaluate_criterion(&tree.definitions[id].criteria[name], &context)
                                .matches
                        })
                        .collect();
                    tree.candidate_indices(&context, &mut selected);
                    let actual: Vec<_> = selected
                        .iter()
                        .map(|&index| &indexed[index])
                        .filter(|(id, name)| {
                            evaluate_criterion(&tree.definitions[id].criteria[name], &context)
                                .matches
                        })
                        .collect();
                    assert_eq!(actual, expected, "{trigger}: {event} inventory={items:?}");
                }
            }
        }
    }
}

#[test]
fn event_candidate_dispatch_is_bounded_by_matching_discriminators_and_rebuilds() {
    let mut entries = Vec::new();
    for index in 0..2000 {
        entries.push((
            format!("test:item_{index:04}"),
            "minecraft:inventory_changed",
            json!({"items":[{"items":format!("test:item_{index}")}]}),
        ));
        entries.push((
            format!("test:recipe_{index:04}"),
            "minecraft:recipe_unlocked",
            json!({"recipe":format!("test:recipe_{index}")}),
        ));
        entries.push((
            format!("test:biome_{index:04}"),
            "minecraft:location",
            json!({"player":{"location":{"biomes":format!("test:biome_{index}")}}}),
        ));
    }
    let mut tree = dispatch_tree(entries);
    let event = json!({"recipe":"test:recipe_7","player":{"location":{"biomes":["test:biome_7"]}}});
    let items = [
        json!({"id":"test:item_7","count":1}),
        json!({"id":"test:item_7","count":1}),
    ];
    let mut selected = Vec::new();
    for trigger in [
        "minecraft:inventory_changed",
        "minecraft:recipe_unlocked",
        "minecraft:location",
    ] {
        tree.candidate_indices(
            &EventContext {
                trigger,
                event: &event,
                inventory: &items,
            },
            &mut selected,
        );
        assert_eq!(selected.len(), 1, "{trigger}");
    }
    tree.definitions.clear();
    tree.rebuild_trigger_index();
    tree.candidate_indices(
        &EventContext {
            trigger: "minecraft:inventory_changed",
            event: &event,
            inventory: &items,
        },
        &mut selected,
    );
    assert!(
        selected.is_empty(),
        "removed definitions must not retain dispatch entries"
    );
}
