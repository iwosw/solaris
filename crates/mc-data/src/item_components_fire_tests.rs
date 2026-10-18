use super::*;

#[test]
fn fire_resistance_embedded_defaults_match_all_sixteen_vanilla_reports() {
    let reports = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/reports/minecraft/components/item");
    let full = load_item_facts(reports).unwrap();
    let embedded = solaris_required_item_facts();
    let mut fire_defaults = 0;
    for (item, facts) in &full.items {
        let fallback = embedded.get(item).is_some_and(|facts| facts.fire_resistant);
        assert_eq!(fallback, facts.fire_resistant, "{item}");
        fire_defaults += usize::from(facts.fire_resistant);
    }
    assert_eq!(fire_defaults, 16);
}

#[test]
fn fire_resistance_is_component_authored_not_inferred_from_item_name() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("stone.json"),
        r##"{
        "components":{"minecraft:damage_resistant":{"types":"#minecraft:is_fire"}}
    }"##,
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("netherite_ingot.json"),
        r##"{
        "components":{"minecraft:damage_resistant":{"types":"#minecraft:is_explosion"}}
    }"##,
    )
    .unwrap();
    let facts = load_item_facts(tmp.path()).unwrap();
    assert!(
        facts
            .get(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap()
            .fire_resistant
    );
    assert!(
        !facts
            .get(&Identifier::parse("minecraft:netherite_ingot").unwrap())
            .unwrap()
            .fire_resistant
    );
}
