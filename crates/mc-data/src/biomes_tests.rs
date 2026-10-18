use super::*;

#[test]
#[ignore = "requires local 26.1.2 data/vanilla biome sidecars"]
fn loads_real_plains_spawns_when_present() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("data/vanilla/data/minecraft/worldgen/biome");
    assert!(
        root.is_dir(),
        "{} missing; run tools/extract-vanilla-data.sh",
        root.display()
    );

    let rules = load_biome_spawn_rules(root).unwrap();
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let creature = rules.entries(&plains, "creature");

    assert!(
        creature
            .iter()
            .any(|entry| entry.entity_type.as_str() == "minecraft:pig")
    );
    assert!(
        creature
            .iter()
            .any(|entry| entry.entity_type.as_str() == "minecraft:chicken")
    );
    assert!(
        creature
            .iter()
            .any(|entry| entry.entity_type.as_str() == "minecraft:cow")
    );
}

#[test]
fn required_spawn_rules_fall_back_to_embedded_land_and_water_defaults() {
    let rules = solaris_required_biome_spawn_rules();
    let forest = Identifier::parse("minecraft:forest").unwrap();
    let deep_ocean = Identifier::parse("minecraft:deep_ocean").unwrap();

    let forest_creatures = rules.entries(&forest, "creature");
    assert!(
        forest_creatures
            .iter()
            .any(|entry| entry.entity_type.as_str() == "minecraft:cow")
    );
    assert!(
        rules
            .entries(&forest, "monster")
            .iter()
            .any(|entry| entry.entity_type.as_str() == "minecraft:zombie")
    );
    assert!(
        rules
            .entries(&deep_ocean, "water_ambient")
            .iter()
            .any(|entry| entry.entity_type.as_str() == "minecraft:cod")
    );
    assert!(
        rules.entries(&deep_ocean, "creature").is_empty(),
        "water biomes must not inherit land creature spawns"
    );
    assert!(
        rules.entries(&forest, "water_ambient").is_empty(),
        "land biomes must not inherit aquatic spawns"
    );
}

#[test]
fn exact_spawn_entries_never_inherit_default_biome_rules() {
    let rules = solaris_required_biome_spawn_rules();
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let snowy = Identifier::parse("minecraft:snowy_plains").unwrap();
    let desert = Identifier::parse("minecraft:desert").unwrap();
    assert_eq!(
        rules.exact_entries(&plains, "creature"),
        rules.entries(&plains, "creature")
    );
    assert!(
        !rules.entries(&snowy, "creature").is_empty(),
        "legacy fallback is unchanged"
    );
    assert!(rules.exact_entries(&snowy, "creature").is_empty());
    assert!(rules.exact_entries(&desert, "creature").is_empty());
    assert!(rules.exact_entries(&plains, "unknown_category").is_empty());
}

#[test]
fn required_spawn_rules_cover_common_overworld_biomes_with_distinct_tables() {
    let rules = solaris_required_biome_spawn_rules();
    assert_eq!(rules.len(), 18); // Eight Overworld, five Nether, five End.

    let plains = Identifier::parse("minecraft:plains").unwrap();
    let forest = Identifier::parse("minecraft:forest").unwrap();
    let desert = Identifier::parse("minecraft:desert").unwrap();
    let river = Identifier::parse("minecraft:river").unwrap();
    let ocean = Identifier::parse("minecraft:ocean").unwrap();

    let plains_zombie = rules
        .entries(&plains, "monster")
        .iter()
        .find(|entry| entry.entity_type.as_str() == "minecraft:zombie")
        .unwrap();
    let forest_zombie = rules
        .entries(&forest, "monster")
        .iter()
        .find(|entry| entry.entity_type.as_str() == "minecraft:zombie")
        .unwrap();
    let desert_zombie = rules
        .entries(&desert, "monster")
        .iter()
        .find(|entry| entry.entity_type.as_str() == "minecraft:zombie")
        .unwrap();
    assert_eq!((plains_zombie.min_count, plains_zombie.weight), (4, 90));
    assert_eq!((forest_zombie.min_count, forest_zombie.weight), (4, 95));
    assert_eq!((desert_zombie.min_count, desert_zombie.weight), (4, 19));

    assert_eq!(
        rules.entries(&river, "water_ambient"),
        &[BiomeSpawnEntry {
            entity_type: Identifier::parse("minecraft:salmon").unwrap(),
            min_count: 1,
            max_count: 5,
            weight: 5,
        }]
    );
    assert_eq!(
        rules.entries(&ocean, "water_ambient"),
        &[BiomeSpawnEntry {
            entity_type: Identifier::parse("minecraft:cod").unwrap(),
            min_count: 3,
            max_count: 6,
            weight: 10,
        }]
    );
}

#[test]
fn custom_spawn_rules_do_not_get_implicit_defaults() {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let forest = Identifier::parse("minecraft:forest").unwrap();
    let pig = Identifier::parse("minecraft:pig").unwrap();
    let rules = BiomeSpawnRules::from_entries(BTreeMap::from([(
        plains,
        BTreeMap::from([(
            "creature".to_string(),
            vec![BiomeSpawnEntry {
                entity_type: pig,
                min_count: 1,
                max_count: 1,
                weight: 1,
            }],
        )]),
    )]));

    assert!(rules.entries(&forest, "creature").is_empty());
}

#[test]
fn embedded_sheep_color_climates_follow_vanilla_variant_tags() {
    let rules = solaris_required_biome_spawn_rules();

    for biome in [
        "minecraft:desert",
        "minecraft:mangrove_swamp",
        "minecraft:jungle",
        "minecraft:savanna",
        "minecraft:badlands",
        "minecraft:nether_wastes",
    ] {
        assert_eq!(
            rules.sheep_color_climate(&Identifier::parse(biome).unwrap()),
            SheepColorClimate::Warm,
            "{biome}"
        );
    }
    for biome in [
        "minecraft:snowy_plains",
        "minecraft:taiga",
        "minecraft:cold_ocean",
        "minecraft:stony_peaks",
        "minecraft:the_end",
    ] {
        assert_eq!(
            rules.sheep_color_climate(&Identifier::parse(biome).unwrap()),
            SheepColorClimate::Cold,
            "{biome}"
        );
    }
    assert_eq!(
        rules.sheep_color_climate(&Identifier::parse("minecraft:plains").unwrap()),
        SheepColorClimate::Temperate
    );
}

#[test]
#[ignore = "requires local 26.1.2 data/vanilla biome and tag sidecars"]
fn real_sidecar_sheep_color_climates_match_resolved_variant_tags_when_present() {
    let data_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("data/vanilla/data/minecraft");
    let biome_dir = data_root.join("worldgen/biome");
    let tags_dir = data_root.join("tags/worldgen/biome");
    assert!(
        biome_dir.is_dir() && tags_dir.is_dir(),
        "need both {} and {}; run tools/extract-vanilla-data.sh",
        biome_dir.display(),
        tags_dir.display()
    );

    let rules = load_biome_spawn_rules(&biome_dir).unwrap();
    let embedded = solaris_required_biome_spawn_rules();
    let worldgen = load_biome_worldgen_data(&biome_dir, &tags_dir).unwrap();
    let warm = Identifier::parse("minecraft:spawns_warm_variant_farm_animals").unwrap();
    let cold = Identifier::parse("minecraft:spawns_cold_variant_farm_animals").unwrap();

    assert!(!worldgen.tag(&warm).is_empty());
    assert!(!worldgen.tag(&cold).is_empty());
    for biome in worldgen.tag(&warm) {
        assert_eq!(
            rules.sheep_color_climate(biome),
            SheepColorClimate::Warm,
            "{biome}"
        );
        assert_eq!(
            embedded.sheep_color_climate(biome),
            SheepColorClimate::Warm,
            "embedded {biome}"
        );
    }
    for biome in worldgen.tag(&cold) {
        assert_eq!(
            rules.sheep_color_climate(biome),
            SheepColorClimate::Cold,
            "{biome}"
        );
        assert_eq!(
            embedded.sheep_color_climate(biome),
            SheepColorClimate::Cold,
            "embedded {biome}"
        );
    }
    for biome in worldgen.biomes() {
        let expected = if worldgen.tag(&warm).contains(biome) {
            SheepColorClimate::Warm
        } else if worldgen.tag(&cold).contains(biome) {
            SheepColorClimate::Cold
        } else {
            SheepColorClimate::Temperate
        };
        assert_eq!(
            embedded.sheep_color_climate(biome),
            expected,
            "embedded {biome}"
        );
    }
}

#[test]
fn loads_biome_features_and_identifier_tags() {
    let tmp = tempfile::tempdir().unwrap();
    let biome_dir = tmp.path().join("worldgen/biome");
    let tags_dir = tmp.path().join("tags/worldgen/biome");
    std::fs::create_dir_all(&biome_dir).unwrap();
    std::fs::create_dir_all(&tags_dir).unwrap();
    std::fs::write(
        biome_dir.join("plains.json"),
        r#"{
          "features": [
            ["minecraft:ore_diamond"],
            ["minecraft:patch_grass", "minecraft:flower_plain"]
          ],
          "spawners": {}
        }"#,
    )
    .unwrap();
    std::fs::write(
        biome_dir.join("forest.json"),
        r#"{ "features": [["minecraft:ore_diamond"]], "spawners": {} }"#,
    )
    .unwrap();
    std::fs::write(
        tags_dir.join("is_overworld.json"),
        r#"{ "values": ["minecraft:plains", "minecraft:forest"] }"#,
    )
    .unwrap();
    std::fs::write(
        tags_dir.join("is_forest.json"),
        r##"{ "values": ["#minecraft:forest_like"] }"##,
    )
    .unwrap();
    std::fs::write(
        tags_dir.join("forest_like.json"),
        r#"{ "values": ["minecraft:forest"] }"#,
    )
    .unwrap();

    let data = load_biome_worldgen_data(&biome_dir, &tags_dir).unwrap();
    let diamond = Identifier::parse("minecraft:ore_diamond").unwrap();
    let biomes = data.biomes_for_feature(&diamond);

    assert_eq!(biomes.len(), 2);
    assert!(biomes.contains(&Identifier::parse("minecraft:plains").unwrap()));
    assert_eq!(
        data.tag(&Identifier::parse("minecraft:is_forest").unwrap()),
        &[Identifier::parse("minecraft:forest").unwrap()]
    );
}

#[test]
#[ignore = "requires local 26.1.2 data/vanilla biome and tag sidecars"]
fn loads_real_overworld_biome_tags_when_present() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("data/vanilla/data/minecraft");
    let biome_dir = root.join("worldgen/biome");
    let tags_dir = root.join("tags/worldgen/biome");
    assert!(
        biome_dir.is_dir() && tags_dir.is_dir(),
        "need both {} and {}; run tools/extract-vanilla-data.sh",
        biome_dir.display(),
        tags_dir.display()
    );

    let data = load_biome_worldgen_data(&biome_dir, &tags_dir).unwrap();
    let overworld = data.tag(&Identifier::parse("minecraft:is_overworld").unwrap());

    assert!(overworld.contains(&Identifier::parse("minecraft:plains").unwrap()));
    assert!(overworld.contains(&Identifier::parse("minecraft:deep_dark").unwrap()));
    assert!(overworld.contains(&Identifier::parse("minecraft:cherry_grove").unwrap()));
    assert!(overworld.len() >= 50);
}
