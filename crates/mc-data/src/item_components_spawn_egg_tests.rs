use super::*;

#[test]
fn spawn_egg_embedded_defaults_match_all_87_vanilla_reports() {
    let reports = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/reports/minecraft/components/item");
    let full = load_item_facts(reports).unwrap();
    let embedded = solaris_required_item_facts();
    let mut eggs = 0;
    for (item, facts) in &full.items {
        if !item.path().ends_with("_spawn_egg") {
            continue;
        }
        let default = facts.spawn_entity.as_ref().expect("egg has entity_data.id");
        let fallback = embedded.get(item).expect("every native egg is embedded");
        assert_eq!(fallback.spawn_entity.as_ref(), Some(default), "{item}");
        assert_eq!(fallback.max_stack_size, facts.max_stack_size, "{item}");
        eggs += 1;
    }
    assert_eq!(eggs, 87);
}

#[test]
fn spawn_egg_report_entity_id_is_not_inferred_from_item_name() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("villager_spawn_egg.json"), r#"{
        "components": {"minecraft:entity_data":{"id":"minecraft:cow"}, "minecraft:max_stack_size":64}
    }"#).unwrap();
    let facts = load_item_facts(tmp.path()).unwrap();
    let item = Identifier::parse("minecraft:villager_spawn_egg").unwrap();
    assert_eq!(
        facts
            .get(&item)
            .unwrap()
            .spawn_entity
            .as_ref()
            .unwrap()
            .as_str(),
        "minecraft:cow"
    );
}

#[test]
fn spawn_egg_report_invalid_entity_identifier_is_a_parse_error() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("villager_spawn_egg.json"),
        r#"{
        "components": {"minecraft:entity_data":{"id":"invalid space"}}
    }"#,
    )
    .unwrap();
    assert!(matches!(
        load_item_facts(tmp.path()),
        Err(ItemComponentsError::Parse { .. })
    ));
}
