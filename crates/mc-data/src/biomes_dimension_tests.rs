use super::*;

#[test]
fn embedded_dimension_tables_and_costs_retain_canonical_values() {
    let rules = solaris_required_biome_spawn_rules();
    for (biome, characteristic, weight, minimum, maximum) in [
        ("nether_wastes", "zombified_piglin", 100, 4, 4),
        ("soul_sand_valley", "skeleton", 20, 5, 5),
        ("crimson_forest", "hoglin", 9, 3, 4),
        ("warped_forest", "enderman", 1, 4, 4),
        ("basalt_deltas", "magma_cube", 100, 2, 5),
        ("the_end", "enderman", 10, 4, 4),
        ("end_highlands", "enderman", 10, 4, 4),
        ("end_midlands", "enderman", 10, 4, 4),
        ("small_end_islands", "enderman", 10, 4, 4),
        ("end_barrens", "enderman", 10, 4, 4),
    ] {
        let biome = Identifier::parse(format!("minecraft:{biome}")).unwrap();
        let entry = rules
            .exact_entries(&biome, "monster")
            .iter()
            .find(|entry| entry.entity_type.path() == characteristic)
            .unwrap();
        assert_eq!(
            (entry.weight, entry.min_count, entry.max_count),
            (weight, minimum, maximum)
        );
        if biome.path() == "soul_sand_valley" {
            for entity in ["enderman", "ghast", "skeleton", "strider"] {
                assert_eq!(
                    rules.spawn_cost(&biome, &format!("minecraft:{entity}")),
                    Some(BiomeSpawnCost {
                        charge: 0.7,
                        energy_budget: 0.15
                    })
                );
            }
        } else if biome.path() == "warped_forest" {
            assert_eq!(
                rules.spawn_cost(&biome, "minecraft:enderman"),
                Some(BiomeSpawnCost {
                    charge: 1.0,
                    energy_budget: 0.12
                })
            );
        } else {
            assert_eq!(
                rules.spawn_cost(&biome, &format!("minecraft:{characteristic}")),
                None
            );
        }
        if !biome.path().contains("end") && biome.path() != "small_end_islands" {
            let entry = &rules.exact_entries(&biome, "creature")[0];
            assert_eq!(entry.entity_type.as_str(), "minecraft:strider");
            assert_eq!((entry.weight, entry.min_count, entry.max_count), (60, 1, 2));
        }
    }
}

#[test]
fn full_biome_loader_keeps_costs_and_rejects_invalid_energy() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("mc-biome-costs-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("warped_forest.json");
    let source = r#"{"spawners":{"monster":[{"type":"minecraft:enderman","minCount":4,"maxCount":4,"weight":1}]},"spawn_costs":{"minecraft:enderman":{"charge":1.0,"energy_budget":0.12}}}"#;
    std::fs::write(&path, source).unwrap();
    let biome = Identifier::parse("minecraft:warped_forest").unwrap();
    assert_eq!(
        load_biome_spawn_rules(&directory)
            .unwrap()
            .spawn_cost(&biome, "minecraft:enderman"),
        Some(BiomeSpawnCost {
            charge: 1.0,
            energy_budget: 0.12
        })
    );
    std::fs::write(&path, source.replace("0.12", "-0.12")).unwrap();
    let error = load_biome_spawn_rules(&directory).unwrap_err().to_string();
    assert!(error.contains("minecraft:warped_forest"));
    assert!(error.contains("minecraft:enderman"));
    assert!(error.contains("energy_budget -0.12"));
    std::fs::remove_dir_all(&directory).unwrap();
}
