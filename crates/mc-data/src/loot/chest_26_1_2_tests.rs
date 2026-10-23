use super::*;
use std::path::PathBuf;

fn id(value: &str) -> Identifier {
    Identifier::parse(value).unwrap()
}

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(rel)
}

#[test]
fn rejects_non_chest_tables() {
    let error = ChestLootTable::compile(
        id("minecraft:chests/test"),
        r#"{"type":"minecraft:entity","pools":[]}"#,
    )
    .expect_err("entity tables must not compile as chest tables");
    assert!(matches!(error, ChestLootError::Unsupported { .. }));
}

#[test]
fn advancement_reward_loot_uses_existing_rolls_without_weakening_chest_type() {
    let raw = r#"{"type":"minecraft:advancement_reward","pools":[{"rolls":1,
        "entries":[{"type":"minecraft:item","name":"minecraft:bread",
            "functions":[{"function":"minecraft:set_count","count":3}]}]}]}"#;
    let table = ChestLootTable::compile_advancement_reward(id("test:reward"), raw).unwrap();
    let items = crate::items::solaris_required_items();
    let drops = table.roll(&mut ChestRng::new(7), &items, 0.0).unwrap();
    assert_eq!(drops.len(), 1);
    assert_eq!(
        items.name_of(drops[0].item_id),
        Some(&id("minecraft:bread"))
    );
    assert_eq!(drops[0].count, 3);
    assert!(ChestLootTable::compile(id("test:reward"), raw).is_err());
    assert!(ChestLootTable::compile_advancement_reward(
        id("test:reward"),
        r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:bread"}]}]}"#
    ).is_err());
    assert!(ChestLootTable::compile_advancement_reward(
        id("test:reward"),
        r#"{"type":"minecraft:advancement_reward","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:bread","functions":[{"function":"minecraft:unsupported"}]}]}]}"#
    ).is_err());
}

#[test]
fn rejects_unsupported_entry_types_conditions_and_functions() {
    for raw in [
        r#"{"type":"minecraft:chest","pools":[{"rolls":1.0,"entries":[{"type":"minecraft:loot_table","value":"minecraft:chests/other"}]}]}"#,
        r#"{"type":"minecraft:chest","pools":[{"rolls":1.0,"conditions":[{"condition":"minecraft:random_chance"}],"entries":[{"type":"minecraft:item","name":"minecraft:bread"}]}]}"#,
        r#"{"type":"minecraft:chest","pools":[{"rolls":1.0,"entries":[{"type":"minecraft:item","name":"minecraft:bread","conditions":[{"condition":"minecraft:random_chance"}]}]}]}"#,
        r#"{"type":"minecraft:chest","pools":[{"rolls":1.0,"entries":[{"type":"minecraft:item","name":"minecraft:bread","functions":[{"function":"minecraft:set_contents"}]}]}]}"#,
    ] {
        let error = ChestLootTable::compile(id("minecraft:chests/test"), raw)
            .expect_err("unsupported constructs must fail closed");
        assert!(matches!(error, ChestLootError::Unsupported { .. }), "{raw}");
    }
}

#[test]
fn random_book_loot_is_an_enchanted_book_with_canonical_stored_enchantments() {
    let table = ChestLootTable::compile(
        id("minecraft:chests/test"),
        r##"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{
            "type":"minecraft:item","name":"minecraft:book","functions":[{
                "function":"minecraft:enchant_randomly","options":"#minecraft:on_random_loot"
            }]
        }]}]}"##,
    )
    .unwrap();
    let items = crate::items::solaris_required_items();
    let stack = table
        .roll(&mut ChestRng::new(7), &items, 0.0)
        .unwrap()
        .remove(0);
    assert_eq!(
        items.name_of(stack.item_id),
        Some(&id("minecraft:enchanted_book"))
    );
    assert!(stack.enchantments.is_empty());
    assert_eq!(stack.extended().stored_enchantments.len(), 1);
    let enchantment = &stack.extended().stored_enchantments[0];
    let candidates = crate::stations::random_loot_enchantment_candidates(
        &id("minecraft:book"),
        Some(&id("minecraft:on_random_loot")),
    );
    let maximum = candidates
        .iter()
        .find(|(id, _)| *id == enchantment.id)
        .unwrap()
        .1;
    assert!((1..=maximum).contains(&enchantment.level));
}

#[test]
fn opening_luck_adds_floored_bonus_rolls_and_rejects_unbounded_context() {
    let table = ChestLootTable::compile(
        id("minecraft:chests/test"),
        r#"{"type":"minecraft:chest","pools":[{"rolls":1,"bonus_rolls":1,
            "entries":[{"type":"minecraft:item","name":"minecraft:bread"}]}]}"#,
    )
    .unwrap();
    let items = crate::items::solaris_required_items();
    let drops = table.roll(&mut ChestRng::new(7), &items, 2.5).unwrap();
    assert_eq!(drops.iter().map(|stack| stack.count).sum::<i32>(), 3);
    assert!(table.roll(&mut ChestRng::new(7), &items, f32::NAN).is_err());
    assert!(table.roll(&mut ChestRng::new(7), &items, 100.0).is_err());
}

#[test]
fn all_empty_pool_rolls_nothing() {
    let table = ChestLootTable::compile(
        id("minecraft:chests/test"),
        r#"{"type":"minecraft:chest","pools":[{"rolls":4.0,"entries":[{"type":"minecraft:empty"}]}]}"#,
    )
    .expect("empty pool compiles");
    assert!(
        table
            .roll(
                &mut ChestRng::new(1),
                &crate::items::solaris_required_items(),
                0.0
            )
            .unwrap()
            .is_empty()
    );
}

#[test]
fn missing_table_file_fails_closed() {
    let missing = id("minecraft:chests/no_such_table");
    let error =
        ChestLootCatalog::load_vanilla_tables(workspace_path("data/vanilla/data"), &[missing])
            .expect_err("missing tables must not fall back silently");
    assert!(matches!(error, ChestLootError::MissingTable { .. }));
}

#[test]
fn luck_quality_can_suppress_entries_and_an_entire_pool() {
    let items = crate::items::solaris_required_items();
    let table = ChestLootTable::compile(
        id("minecraft:chests/quality"),
        r#"{
        "type":"minecraft:chest","pools":[{"rolls":4,"entries":[
            {"type":"minecraft:item","name":"minecraft:bread","quality":1},
            {"type":"minecraft:item","name":"minecraft:coal","quality":-1}
        ]}]}"#,
    )
    .unwrap();
    for (luck, expected) in [(1.0, "minecraft:bread"), (-1.0, "minecraft:coal")] {
        let drops = table.roll(&mut ChestRng::new(7), &items, luck).unwrap();
        assert_eq!(drops.iter().map(|stack| stack.count).sum::<i32>(), 4);
        assert!(
            drops
                .iter()
                .all(|stack| items.name_of(stack.item_id) == Some(&id(expected)))
        );
    }
    let zero = ChestLootTable::compile(
        id("minecraft:chests/zero"),
        r#"{
        "type":"minecraft:chest","pools":[{"rolls":1,"entries":[
            {"type":"minecraft:item","name":"minecraft:bread","quality":1}
        ]}]}"#,
    )
    .unwrap();
    assert!(
        zero.roll(&mut ChestRng::new(7), &items, -1.0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn fractional_bonus_uses_vanilla_float_multiplication_before_floor() {
    let table = ChestLootTable::compile(
        id("minecraft:chests/fractional"),
        r#"{
        "type":"minecraft:chest","pools":[{"rolls":1,"bonus_rolls":0.29,"entries":[
            {"type":"minecraft:item","name":"minecraft:bread"}
        ]}]}"#,
    )
    .unwrap();
    let items = crate::items::solaris_required_items();
    let drops = table.roll(&mut ChestRng::new(7), &items, 100.0).unwrap();
    assert_eq!(drops.iter().map(|stack| stack.count).sum::<i32>(), 30);
}

#[test]
fn functions_keep_order_and_conditions_at_each_scope() {
    let raw = r#"{"type":"minecraft:chest","functions":[
        {"function":"minecraft:set_count","count":2,"add":true}
    ],"pools":[{"rolls":1,"conditions":[{"condition":"minecraft:random_chance","chance":1}],
        "functions":[{"function":"minecraft:limit_count","limit":{"max":4}}],
        "entries":[{"type":"minecraft:item","name":"minecraft:bread","functions":[
            {"function":"minecraft:set_count","count":{"type":"minecraft:constant","value":2}},
            {"function":"minecraft:set_count","count":{"type":"minecraft:binomial","n":3,"p":1},"add":true},
            {"function":"minecraft:set_count","count":60,"conditions":[{"condition":"minecraft:random_chance","chance":0}]}
        ]}]}]}"#;
    let table = ChestLootTable::compile(id("test:ordered"), raw).unwrap();
    let items = crate::items::solaris_required_items();
    let drops = table.roll(&mut ChestRng::new(4), &items, 0.0).unwrap();
    assert_eq!(drops[0].count, 6);
}

#[test]
fn explicit_random_options_produce_swift_sneak_not_default_book_pool() {
    let raw = r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{
        "type":"minecraft:item","name":"minecraft:book","functions":[{
            "function":"minecraft:enchant_randomly","options":"minecraft:swift_sneak"
        }]
    }]}]}"#;
    let table = ChestLootTable::compile(id("test:swift"), raw).unwrap();
    let items = crate::items::solaris_required_items();
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..64 {
        let a = table.roll(&mut ChestRng::new(seed), &items, 0.0).unwrap();
        let b = table.roll(&mut ChestRng::new(seed), &items, 0.0).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            items.name_of(a[0].item_id),
            Some(&id("minecraft:enchanted_book"))
        );
        let enchantment = &a[0].extended().stored_enchantments[0];
        assert_eq!(enchantment.id, id("minecraft:swift_sneak"));
        assert!((1..=3).contains(&enchantment.level));
        seen.insert(enchantment.level);
    }
    assert_eq!(seen.len(), 3);
}

#[test]
fn component_functions_survive_native_nbt_roundtrip() {
    let raw = r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{
        "type":"minecraft:item","name":"minecraft:potion","functions":[
            {"function":"minecraft:set_potion","id":"minecraft:water_breathing"},
            {"function":"minecraft:set_name","target":"item_name","name":{"translate":"test.loot"}},
            {"function":"minecraft:set_lore","mode":"replace_all","lore":[{"text":"first"}]},
            {"function":"minecraft:set_lore","mode":"append","lore":[{"text":"second"}]},
            {"function":"minecraft:set_components","components":{"minecraft:custom_data":{"loot":true}}}
        ]
    }]}]}"#;
    let table = ChestLootTable::compile(id("test:components"), raw).unwrap();
    let items = crate::items::solaris_required_items();
    let drops = table.roll(&mut ChestRng::new(3), &items, 0.0).unwrap();
    let stack = &drops[0];
    assert_eq!(
        stack.extended().potion,
        Some(id("minecraft:water_breathing"))
    );
    let fields = crate::item_nbt::stack_to_fields(&items, stack).unwrap();
    let decoded = crate::item_nbt::stack_from_fields(&items, &fields)
        .unwrap()
        .unwrap();
    assert_eq!(*stack, decoded);
    let Some(mc_nbt::Tag::Compound(components)) = crate::item_nbt::field(&fields, "components")
    else {
        panic!("components");
    };
    let Some(mc_nbt::Tag::List(lore)) = crate::item_nbt::field(components, "minecraft:lore") else {
        panic!("lore");
    };
    assert_eq!(lore.elements.len(), 2);
    assert!(crate::item_nbt::field(components, "minecraft:item_name").is_some());
    assert!(crate::item_nbt::field(components, "minecraft:custom_data").is_some());
}

#[test]
fn maps_require_native_owner_and_never_fabricate_empty_fallback() {
    let raw = r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{
        "type":"minecraft:item","name":"minecraft:map","functions":[{
            "function":"minecraft:exploration_map","decoration":"minecraft:red_x","zoom":1,"skip_existing_chunks":false
        }]
    }]}]}"#;
    let table = ChestLootTable::compile(id("test:map"), raw).unwrap();
    let items = crate::items::solaris_required_items();
    assert!(table.requires_world_context());
    let mut rng = ChestRng::new(3);
    let error = table.roll(&mut rng, &items, 0.0).unwrap_err().to_string();
    assert!(error.contains("minecraft:exploration_map"), "{error}");
    assert_eq!(
        rng.next_u64(),
        ChestRng::new(3).next_u64(),
        "missing context must not consume seed"
    );
    let mut unresolved = |_: &ExplorationMap| Ok(None);
    assert!(
        table
            .roll_with_context(
                &mut ChestRng::new(3),
                &items,
                &mut ChestLootContext {
                    luck: 0.0,
                    exploration_map: Some(&mut unresolved)
                }
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(table.exploration_maps().len(), 1);
    let mut calls = 0;
    let mut resolver = |request: &ExplorationMap| {
        calls += 1;
        assert_eq!(request.destination, id("minecraft:on_treasure_maps"));
        assert_eq!(request.decoration, id("minecraft:red_x"));
        assert_eq!(request.zoom, 1);
        assert!(!request.skip_existing_chunks);
        Ok(Some(ItemStack::new(
            items.id_of(&id("minecraft:filled_map")).unwrap(),
            1,
        )))
    };
    let filled = table
        .roll_with_context(
            &mut ChestRng::new(3),
            &items,
            &mut ChestLootContext {
                luck: 0.0,
                exploration_map: Some(&mut resolver),
            },
        )
        .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(
        items.name_of(filled[0].item_id),
        Some(&id("minecraft:filled_map"))
    );
}

#[test]
fn unsupported_function_error_names_table_function_and_path() {
    let raw = r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{
        "type":"minecraft:item","name":"minecraft:bread","functions":[{"function":"test:no_silent_drop"}]
    }]}]}"#;
    let error = ChestLootTable::compile(id("test:broken"), raw)
        .unwrap_err()
        .to_string();
    for expected in [
        "test:broken",
        "test:no_silent_drop",
        "$.pools[0]",
        "entries[0]",
    ] {
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn catalog_rejects_reference_cycles_during_load() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("minecraft/loot_table/chests");
    std::fs::create_dir_all(&directory).unwrap();
    for (name, target) in [("a", "b"), ("b", "a")] {
        std::fs::write(directory.join(format!("{name}.json")),format!(
            r#"{{"type":"minecraft:chest","pools":[{{"rolls":1,"entries":[{{"type":"minecraft:loot_table","value":"minecraft:chests/{target}"}}]}}]}}"#
        )).unwrap();
    }
    let error = ChestLootCatalog::load_vanilla_tables(
        root.path(),
        &[id("minecraft:chests/a"), id("minecraft:chests/b")],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("cycle"), "{error}");
    assert!(error.contains("minecraft:chests/a"), "{error}");
}

#[test]
#[ignore = "requires selected local vanilla data"]
fn every_vanilla_chest_compiles_rolls_and_is_seed_deterministic() {
    let root = workspace_path("data/vanilla");
    let catalog = load_vanilla(&root).unwrap();
    let mut ids = Vec::new();
    collect_tables(
        &root.join("data/minecraft/loot_table/chests"),
        &root.join("data/minecraft/loot_table"),
        &mut ids,
    )
    .unwrap();
    assert_eq!(ids.len(), 56, "26.1.2 chest table inventory changed");
    assert_eq!(catalog.len(), 77, "56 chests, 6 archaeology, 15 hero gifts");
    let items = crate::items::solaris_required_items();
    for (id, table) in &catalog.tables {
        for seed in 0..16 {
            // Test-only deterministic map owner; real saved-map/locate integration
            // is deliberately not claimed by this compiler regression.
            let mut map_owner = |_: &ExplorationMap| {
                Ok(Some(ItemStack::new(
                    items
                        .id_of(&super::tests::id("minecraft:filled_map"))
                        .unwrap(),
                    1,
                )))
            };
            let first = table
                .roll_with_context(
                    &mut ChestRng::new(seed),
                    &items,
                    &mut ChestLootContext {
                        luck: 0.0,
                        exploration_map: Some(&mut map_owner),
                    },
                )
                .unwrap_or_else(|e| panic!("{id} seed {seed}: {e}"));
            let second = table
                .roll_with_context(
                    &mut ChestRng::new(seed),
                    &items,
                    &mut ChestLootContext {
                        luck: 0.0,
                        exploration_map: Some(&mut map_owner),
                    },
                )
                .unwrap();
            assert_eq!(first, second, "{id} seed {seed}");
            for stack in &first {
                let fields = crate::item_nbt::stack_to_fields(&items, stack).unwrap();
                assert_eq!(
                    *stack,
                    crate::item_nbt::stack_from_fields(&items, &fields)
                        .unwrap()
                        .unwrap(),
                    "{id}"
                );
            }
        }
    }
    for id in ids {
        assert!(catalog.get(&id).is_some(), "{id}");
    }
}

#[test]
#[ignore = "requires selected local vanilla data"]
fn vanilla_signature_rewards_are_preserved() {
    let catalog = load_vanilla(&workspace_path("data/vanilla")).unwrap();
    let items = crate::items::solaris_required_items();
    let ancient = catalog.get(&id("minecraft:chests/ancient_city")).unwrap();
    let end = catalog
        .get(&id("minecraft:chests/end_city_treasure"))
        .unwrap();
    let buried = catalog
        .get(&id("minecraft:chests/buried_treasure"))
        .unwrap();
    let trial = catalog
        .get(&id("minecraft:chests/trial_chambers/reward_ominous"))
        .unwrap();
    let mut swift = false;
    let mut gear = false;
    let mut multiple = false;
    let mut wind = false;
    for seed in 0..256 {
        let ancient = ancient.roll(&mut ChestRng::new(seed), &items, 0.0).unwrap();
        swift |= ancient.iter().any(|s| {
            s.extended()
                .stored_enchantments
                .iter()
                .any(|e| e.id == id("minecraft:swift_sneak"))
        });
        let end = end.roll(&mut ChestRng::new(seed), &items, 0.0).unwrap();
        gear |= end.iter().any(|s| !s.enchantments.is_empty());
        multiple |= end.iter().any(|s| s.enchantments.len() > 1);
        let buried = buried.roll(&mut ChestRng::new(seed), &items, 0.0).unwrap();
        assert_eq!(
            buried
                .iter()
                .filter(|s| items.name_of(s.item_id) == Some(&id("minecraft:heart_of_the_sea")))
                .map(|s| s.count)
                .sum::<i32>(),
            1
        );
        let trial = trial.roll(&mut ChestRng::new(seed), &items, 0.0).unwrap();
        wind |= trial.iter().any(|s| {
            s.extended()
                .stored_enchantments
                .iter()
                .any(|e| e.id == id("minecraft:wind_burst") && e.level == 1)
        });
    }
    assert!(swift, "ancient city swift sneak missing");
    assert!(
        gear && multiple,
        "End City enchanted gear / multiple compatible enchantments missing"
    );
    assert!(wind, "nested ominous trial wind burst reward missing");
}

#[test]
fn selected_catalog_load_follows_references_and_propagates_map_preflight() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("minecraft/loot_table/chests");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("parent.json"),r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:loot_table","value":"minecraft:chests/child"}]}]}"#).unwrap();
    std::fs::write(directory.join("child.json"),r#"{"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:map","functions":[{"function":"minecraft:exploration_map"}]}]}]}"#).unwrap();
    let parent = id("minecraft:chests/parent");
    let catalog =
        ChestLootCatalog::load_vanilla_tables(root.path(), std::slice::from_ref(&parent)).unwrap();
    let table = catalog.get(&parent).unwrap();
    assert!(table.requires_world_context());
    assert_eq!(table.exploration_maps().len(), 1);
    assert_eq!(
        table.exploration_maps()[0].destination,
        id("minecraft:on_treasure_maps")
    );
    assert!(
        table
            .roll(
                &mut ChestRng::new(1),
                &crate::items::solaris_required_items(),
                0.0
            )
            .is_err()
    );
}

#[test]
fn set_damage_changes_durability_but_leaves_nondurable_trial_loot_intact() {
    let items = crate::items::solaris_required_items();
    for (item, expected) in [
        ("minecraft:iron_pickaxe", Some(125)),
        ("minecraft:compass", None),
    ] {
        let raw = format!(
            r#"{{"type":"minecraft:chest","pools":[{{"rolls":1,"entries":[{{"type":"minecraft:item","name":"{item}","functions":[{{"function":"minecraft:set_damage","damage":0.5}}]}}]}}]}}"#
        );
        let table = ChestLootTable::compile(id("test:damage"), &raw).unwrap();
        let drops = table.roll(&mut ChestRng::new(1), &items, 0.0).unwrap();
        assert_eq!(drops[0].damage, expected);
        assert_eq!(items.name_of(drops[0].item_id), Some(&id(item)));
    }
}
