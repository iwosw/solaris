use super::*;

#[test]
fn every_builtin_component_id_matches_the_26_1_2_report_and_inverse() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/reports/registries.json");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let entries = report["minecraft:data_component_type"]["entries"]
        .as_object()
        .unwrap();
    assert_eq!(entries.len(), COUNT);
    assert_eq!(BY_NAME.len(), COUNT);
    for (entry, value) in entries {
        let protocol_id = value["protocol_id"].as_u64().unwrap() as u32;
        let identifier = Identifier::parse(entry).unwrap();
        assert_eq!(id(&identifier), Some(protocol_id), "{entry}");
        assert_eq!(name(protocol_id), Some(entry.as_str()), "{entry}");
    }
    assert_eq!(id(&Identifier::parse("solaris:unknown").unwrap()), None);
    assert_eq!(name(COUNT as u32), None);
    assert_eq!(name(u32::MAX), None);
}
