use super::*;

fn tables() -> TrialLootTables {
    TrialLootTables::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"),
    )
    .unwrap()
}
#[test]
fn all_runtime_trial_tables_roll_canonical_components() {
    let items = crate::items::solaris_required_items();
    let facts = crate::item_components::solaris_required_item_facts();
    let tables = tables();
    for name in tables.tables.keys() {
        for seed in 0..100 {
            let output = tables.roll(name, seed, &items, &facts).unwrap();
            for item in output {
                let fields = crate::item_nbt::stack_to_fields(&items, &item).unwrap();
                assert_eq!(
                    crate::item_nbt::stack_from_fields(&items, &fields).unwrap(),
                    Some(item)
                );
            }
        }
    }
}
#[test]
fn normal_and_ominous_key_tables_are_distinct_and_exact() {
    let items = crate::items::solaris_required_items();
    let facts = crate::item_components::solaris_required_item_facts();
    let tables = tables();
    for (table, item) in [
        ("spawners/trial_chamber/key", "trial_key"),
        ("spawners/ominous/trial_chamber/key", "ominous_trial_key"),
    ] {
        let drops = tables
            .roll(&format!("minecraft:{table}"), 42, &items, &facts)
            .unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(items.name_of(drops[0].item_id).unwrap().path(), item);
        assert_eq!(drops[0].count, 1);
    }
}

#[test]
fn trial_catalog_requires_its_own_root_and_preserves_loaded_sources() {
    let vanilla = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let temporary = tempfile::tempdir().unwrap();
    for &(_, relative) in SOURCES {
        let path = temporary.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(vanilla.join(relative), path).unwrap();
    }
    let loaded = TrialLootTables::load(temporary.path()).unwrap();
    let (name, relative) = SOURCES[0];
    std::fs::remove_file(temporary.path().join(relative)).unwrap();
    let error = TrialLootTables::load(temporary.path())
        .err()
        .expect("a previous load cannot mask a missing source");
    assert!(error.contains(relative), "{error}");
    assert!(loaded.tables.contains_key(name));
    std::fs::write(temporary.path().join(relative), "{").unwrap();
    let error = TrialLootTables::load(temporary.path())
        .err()
        .expect("invalid source is rejected during loading");
    assert!(error.contains(relative), "{error}");
}
