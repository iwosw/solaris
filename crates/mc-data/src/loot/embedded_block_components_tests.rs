use super::*;

fn copies_components(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => {
            matches!(
                fields.get("function").and_then(serde_json::Value::as_str),
                Some("minecraft:copy_components" | "minecraft:copy_state")
            ) || fields.values().any(copies_components)
        }
        serde_json::Value::Array(values) => values.iter().any(copies_components),
        _ => false,
    }
}
#[test]
fn runtime_owner_set_covers_every_vanilla_copied_block_table() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/data/minecraft/loot_table/blocks");
    let mut expected = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        if copies_components(&value) {
            expected.insert(path.file_stem().unwrap().to_str().unwrap().to_owned());
        }
    }
    let actual: std::collections::BTreeSet<_> =
        SOURCES.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(actual.len(), 71);
    assert_eq!(actual, expected);
}
#[test]
fn runtime_copied_rules_are_identical_to_sidecar_compilation_for_all_owners() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let tables = super::super::load_builtin(&root).unwrap();
    for &(name, relative) in SOURCES {
        let raw = crate::runtime_resources::read_json(&root, relative).unwrap();
        let id = Identifier::parse(format!("minecraft:{name}")).unwrap();
        let value = serde_json::from_str(&raw).unwrap();
        let compiled = block_loot_from_table(&root.join(relative), &value)
            .unwrap()
            .unwrap();
        assert_eq!(tables.block_loot(&id), Some(&compiled), "{name}");
        assert!(tables.block_needs_entity(&id), "{name}");
    }
}
