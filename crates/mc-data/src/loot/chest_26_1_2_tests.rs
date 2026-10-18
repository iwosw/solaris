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
