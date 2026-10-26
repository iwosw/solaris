use super::*;
use crate::dimensions::Dimension;
use crate::journal::CheckpointManifest;
use crate::native_checkpoint as bridge;
use mc_data::entity_nbt::{VanillaEntity, entity_from_fields};
use mc_data::player_nbt::{VanillaPlayer, player_from_fields};
use mc_nbt::{ListTag, tag_type};
use mc_world::native::NativeStore;
use mc_world::{BlockPos, ChunkPos};

fn compound(fields: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        fields
            .into_iter()
            .map(|(key, tag)| (key.into(), tag))
            .collect(),
    )
}

fn string(value: &str) -> Tag {
    Tag::String(value.into())
}

fn list(element_type: u8, elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type,
        elements,
    })
}

fn fields(tag: &Tag) -> &[(String, Tag)] {
    let Tag::Compound(fields) = tag else {
        panic!("expected compound: {tag:?}")
    };
    fields
}

fn field<'a>(tag: &'a Tag, key: &str) -> &'a Tag {
    fields(tag)
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("missing {key}: {tag:?}"))
}

fn absent(tag: &Tag, key: &str) -> bool {
    !fields(tag).iter().any(|(name, _)| name == key)
}

fn elements(tag: &Tag) -> &[Tag] {
    let Tag::List(list) = tag else {
        panic!("expected list: {tag:?}")
    };
    &list.elements
}

fn legacy_item(id: &str, count: i8, slot: Option<i8>) -> Tag {
    let mut fields = vec![
        ("id", string(id)),
        ("Count", Tag::Byte(count)),
        (
            "tag",
            compound(vec![
                (
                    "display",
                    compound(vec![(
                        "Name",
                        string("{\"text\":\"Old stone\",\"bold\":true}"),
                    )]),
                ),
                ("mod:opaque", Tag::Int(37)),
            ]),
        ),
    ];
    if let Some(slot) = slot {
        fields.push(("Slot", Tag::Byte(slot)));
    }
    compound(fields)
}

fn legacy_level() -> Tag {
    let dimensions = [
        (
            "minecraft:overworld",
            "minecraft:overworld",
            "minecraft:multi_noise",
        ),
        (
            "minecraft:the_nether",
            "minecraft:nether",
            "minecraft:multi_noise",
        ),
        ("minecraft:the_end", "minecraft:end", "minecraft:the_end"),
    ]
    .into_iter()
    .map(|(dimension, settings, biome_type)| {
        let mut biome = vec![("type", string(biome_type))];
        if biome_type == "minecraft:multi_noise" {
            biome.push(("preset", string(settings)));
        } else {
            biome.push(("seed", Tag::Long(42)));
        }
        (
            dimension,
            compound(vec![
                ("type", string(dimension)),
                (
                    "generator",
                    compound(vec![
                        ("type", string("minecraft:noise")),
                        ("seed", Tag::Long(42)),
                        ("settings", string(settings)),
                        ("biome_source", compound(biome)),
                    ]),
                ),
            ]),
        )
    })
    .collect();
    // These are the pre-saved-data-split 1.18 fields, not modern templates
    // with a downgraded version number. Legacy border warnings are seconds.
    compound(vec![(
        "Data",
        compound(vec![
            ("DataVersion", Tag::Int(2860)),
            ("LevelName", string("Hand-built Java 1.18 converter world")),
            ("Time", Tag::Long(79)),
            ("DayTime", Tag::Long(18000)),
            ("Difficulty", Tag::Byte(3)),
            ("DifficultyLocked", Tag::Byte(1)),
            ("hardcore", Tag::Byte(0)),
            ("SpawnX", Tag::Int(12)),
            ("SpawnY", Tag::Int(75)),
            ("SpawnZ", Tag::Int(-45)),
            ("SpawnAngle", Tag::Float(90.0)),
            (
                "GameRules",
                compound(vec![
                    ("doDaylightCycle", string("false")),
                    ("doWeatherCycle", string("true")),
                    ("randomTickSpeed", string("7")),
                ]),
            ),
            ("clearWeatherTime", Tag::Int(0)),
            ("rainTime", Tag::Int(2345)),
            ("thunderTime", Tag::Int(1234)),
            ("raining", Tag::Byte(1)),
            ("thundering", Tag::Byte(1)),
            ("BorderCenterX", Tag::Double(0.0)),
            ("BorderCenterZ", Tag::Double(0.0)),
            ("BorderSize", Tag::Double(59_999_968.0)),
            ("BorderSizeLerpTarget", Tag::Double(59_999_968.0)),
            ("BorderSizeLerpTime", Tag::Long(0)),
            ("BorderSafeZone", Tag::Double(5.0)),
            ("BorderDamagePerBlock", Tag::Double(0.2)),
            ("BorderWarningBlocks", Tag::Double(9.0)),
            ("BorderWarningTime", Tag::Double(15.0)),
            (
                "WorldGenSettings",
                compound(vec![
                    ("seed", Tag::Long(42)),
                    ("generate_features", Tag::Byte(1)),
                    ("bonus_chest", Tag::Byte(0)),
                    ("dimensions", compound(dimensions)),
                ]),
            ),
            ("mod:level", Tag::LongArray(vec![9, 8, 7])),
        ]),
    )])
}

fn legacy_chunk() -> Tag {
    let sections = (-4_i8..20)
        .map(|y| {
            let block_states = if y == 0 {
                // Java 1.18 uses padded four-bit palette indices. Only (1,0,1)
                // is a chest; all other entries in this section are air.
                let mut data = vec![0_i64; 256];
                data[1] = 1 << 4;
                compound(vec![
                    (
                        "palette",
                        list(
                            tag_type::COMPOUND,
                            vec![
                                compound(vec![("Name", string("minecraft:air"))]),
                                compound(vec![
                                    ("Name", string("minecraft:chest")),
                                    (
                                        "Properties",
                                        compound(vec![
                                            ("facing", string("north")),
                                            ("type", string("single")),
                                            ("waterlogged", string("false")),
                                        ]),
                                    ),
                                ]),
                            ],
                        ),
                    ),
                    ("data", Tag::LongArray(data)),
                ])
            } else {
                compound(vec![(
                    "palette",
                    list(
                        tag_type::COMPOUND,
                        vec![compound(vec![(
                            "Name",
                            string(if y == 1 {
                                "minecraft:grass"
                            } else {
                                "minecraft:air"
                            }),
                        )])],
                    ),
                )])
            };
            compound(vec![
                ("Y", Tag::Byte(y)),
                ("block_states", block_states),
                (
                    "biomes",
                    compound(vec![(
                        "palette",
                        list(tag_type::STRING, vec![string("minecraft:plains")]),
                    )]),
                ),
            ])
        })
        .collect();
    let nested = legacy_item("minecraft:stone", 3, Some(0));
    let container = compound(vec![
        ("id", string("minecraft:chest")),
        ("Count", Tag::Byte(1)),
        ("Slot", Tag::Byte(0)),
        (
            "tag",
            compound(vec![(
                "BlockEntityTag",
                compound(vec![
                    ("id", string("minecraft:chest")),
                    ("Items", list(tag_type::COMPOUND, vec![nested])),
                ]),
            )]),
        ),
    ]);
    let tick = |id: &str, x, y, delay, priority| {
        compound(vec![
            ("i", string(id)),
            ("x", Tag::Int(x)),
            ("y", Tag::Int(y)),
            ("z", Tag::Int(1)),
            ("t", Tag::Int(delay)),
            ("p", Tag::Int(priority)),
        ])
    };
    compound(vec![
        ("DataVersion", Tag::Int(2860)),
        ("xPos", Tag::Int(0)),
        ("zPos", Tag::Int(0)),
        ("yPos", Tag::Int(-4)),
        ("Status", string("full")),
        ("LastUpdate", Tag::Long(79)),
        ("InhabitedTime", Tag::Long(800)),
        ("sections", list(tag_type::COMPOUND, sections)),
        (
            "block_entities",
            list(
                tag_type::COMPOUND,
                vec![compound(vec![
                    ("id", string("minecraft:chest")),
                    ("x", Tag::Int(1)),
                    ("y", Tag::Int(0)),
                    ("z", Tag::Int(1)),
                    ("Items", list(tag_type::COMPOUND, vec![container])),
                ])],
            ),
        ),
        (
            "block_ticks",
            list(
                tag_type::COMPOUND,
                vec![tick("minecraft:grass", 0, 16, 12, -1)],
            ),
        ),
        (
            "fluid_ticks",
            list(
                tag_type::COMPOUND,
                vec![tick("minecraft:flowing_water", 2, 0, 16, 1)],
            ),
        ),
        ("mod:terrain", string("retained")),
    ])
}

fn legacy_entities() -> Tag {
    compound(vec![
        ("DataVersion", Tag::Int(2860)),
        ("Position", Tag::IntArray(vec![0, 0])),
        (
            "Entities",
            list(
                tag_type::COMPOUND,
                vec![compound(vec![
                    ("id", string("minecraft:item")),
                    ("UUID", Tag::IntArray(vec![0, 0, 0, 7])),
                    (
                        "Pos",
                        list(
                            tag_type::DOUBLE,
                            vec![Tag::Double(1.0), Tag::Double(65.0), Tag::Double(1.0)],
                        ),
                    ),
                    ("Motion", list(tag_type::DOUBLE, vec![Tag::Double(0.0); 3])),
                    ("Rotation", list(tag_type::FLOAT, vec![Tag::Float(0.0); 2])),
                    ("OnGround", Tag::Byte(1)),
                    ("FallDistance", Tag::Float(2.5)),
                    ("Age", Tag::Short(12)),
                    ("PickupDelay", Tag::Short(23)),
                    ("Item", legacy_item("minecraft:stone", 4, None)),
                    ("mod:entity", string("retained")),
                ])],
            ),
        ),
    ])
}

fn legacy_player() -> Tag {
    compound(vec![
        ("DataVersion", Tag::Int(2860)),
        ("UUID", Tag::IntArray(vec![0, 0, 0, 3])),
        ("Dimension", string("minecraft:overworld")),
        (
            "Pos",
            list(
                tag_type::DOUBLE,
                vec![Tag::Double(12.0), Tag::Double(80.0), Tag::Double(-7.0)],
            ),
        ),
        ("Motion", list(tag_type::DOUBLE, vec![Tag::Double(0.0); 3])),
        (
            "Rotation",
            list(tag_type::FLOAT, vec![Tag::Float(45.0), Tag::Float(10.0)]),
        ),
        ("Health", Tag::Float(18.0)),
        ("foodLevel", Tag::Int(17)),
        ("foodSaturationLevel", Tag::Float(3.0)),
        ("SelectedItemSlot", Tag::Int(0)),
        ("playerGameType", Tag::Int(0)),
        (
            "Inventory",
            list(
                tag_type::COMPOUND,
                vec![
                    legacy_item("minecraft:stone", 5, Some(0)),
                    legacy_item("minecraft:iron_boots", 1, Some(100)),
                    legacy_item("minecraft:stone", 2, Some(-106)),
                ],
            ),
        ),
        ("EnderItems", list(tag_type::COMPOUND, vec![])),
        ("SpawnX", Tag::Int(10)),
        ("SpawnY", Tag::Int(70)),
        ("SpawnZ", Tag::Int(20)),
        ("SpawnDimension", string("minecraft:overworld")),
        ("SpawnAngle", Tag::Float(30.0)),
        ("SpawnForced", Tag::Byte(0)),
        ("mod:player", string("retained")),
    ])
}

fn write_legacy_region(root: &Path, directory: &str, tag: &Tag) {
    fs::create_dir_all(root.join(directory)).unwrap();
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", tag).unwrap();
    mc_world::anvil::write_region(
        root.join(directory).join("r.0.0.mca"),
        &[mc_world::anvil::ChunkPayload {
            local_x: 0,
            local_z: 0,
            timestamp: 0,
            uncompressed_nbt: bytes,
        }],
    )
    .unwrap();
}

fn legacy_saved_data(data: Tag) -> Tag {
    compound(vec![("DataVersion", Tag::Int(2860)), ("data", data)])
}

fn legacy_poi(version: i32) -> Tag {
    compound(vec![
        ("DataVersion", Tag::Int(version)),
        (
            "Sections",
            compound(vec![(
                "4",
                compound(vec![
                    ("Valid", Tag::Byte(1)),
                    (
                        "Records",
                        list(
                            tag_type::COMPOUND,
                            vec![compound(vec![
                                ("pos", Tag::IntArray(vec![1, 65, 1])),
                                ("type", string("minecraft:home")),
                                ("free_tickets", Tag::Int(1)),
                            ])],
                        ),
                    ),
                ]),
            )]),
        ),
    ])
}

const RAID_SAVED_PATHS: [(&str, &str); 3] = [
    (
        "data/raids.dat",
        "dimensions/minecraft/overworld/data/minecraft/raids.dat",
    ),
    (
        "DIM-1/data/raids.dat",
        "dimensions/minecraft/the_nether/data/minecraft/raids.dat",
    ),
    (
        "DIM1/data/raids_end.dat",
        "dimensions/minecraft/the_end/data/minecraft/raids.dat",
    ),
];

const UPGRADED_MAP_PATH: (&str, &str) = ("data/map_7.dat", "data/minecraft/maps/7.dat");

const DEFERRED_SAVED_PATHS: [(&str, &str); 3] = [
    ("data/scoreboard.dat", "data/minecraft/scoreboard.dat"),
    (
        "data/command_storage_test.dat",
        "data/test/command_storage.dat",
    ),
    (
        "data/random_sequences.dat",
        "data/minecraft/random_sequences.dat",
    ),
];

const DEFERRED_POI_PATHS: [(&str, &str); 2] = [
    (
        "poi/r.0.0.mca",
        "dimensions/minecraft/overworld/poi/r.0.0.mca",
    ),
    (
        "DIM-1/poi/r.0.0.mca",
        "dimensions/minecraft/the_nether/poi/r.0.0.mca",
    ),
];

fn write_legacy_auxiliary(root: &Path) {
    for (path, _) in &RAID_SAVED_PATHS {
        write_nbt(
            &root.join(path),
            &legacy_saved_data(compound(vec![
                ("NextAvailableID", Tag::Int(4)),
                ("Tick", Tag::Int(19)),
                ("Raids", list(tag_type::COMPOUND, vec![])),
            ])),
        )
        .unwrap();
    }
    let saved_data = [
        (
            "data/map_7.dat",
            compound(vec![
                ("dimension", string("minecraft:overworld")),
                ("xCenter", Tag::Int(128)),
                ("zCenter", Tag::Int(-128)),
                ("scale", Tag::Byte(1)),
                ("trackingPosition", Tag::Byte(1)),
                ("unlimitedTracking", Tag::Byte(0)),
                ("locked", Tag::Byte(0)),
                ("colors", Tag::ByteArray(vec![0; 128 * 128])),
            ]),
        ),
        ("data/idcounts.dat", compound(vec![("map", Tag::Int(7))])),
        (
            "data/scoreboard.dat",
            compound(vec![
                (
                    "Objectives",
                    list(
                        tag_type::COMPOUND,
                        vec![compound(vec![
                            ("Name", string("old_score")),
                            ("CriteriaName", string("dummy")),
                            ("DisplayName", string("{\"text\":\"Old score\"}")),
                            ("RenderType", string("integer")),
                        ])],
                    ),
                ),
                (
                    "PlayerScores",
                    list(
                        tag_type::COMPOUND,
                        vec![compound(vec![
                            ("Name", string("LegacyPlayer")),
                            ("Objective", string("old_score")),
                            ("Score", Tag::Int(23)),
                            ("Locked", Tag::Byte(0)),
                        ])],
                    ),
                ),
                ("Teams", list(tag_type::COMPOUND, vec![])),
            ]),
        ),
        (
            "data/command_storage_test.dat",
            compound(vec![(
                "contents",
                compound(vec![("old", compound(vec![("value", Tag::Int(37))]))]),
            )]),
        ),
    ];
    for (path, data) in saved_data {
        write_nbt(&root.join(path), &legacy_saved_data(data)).unwrap();
    }
    // Random sequences appeared after 1.18; this later old-layout record
    // also retains its real declared version rather than being stamped 4790.
    write_nbt(
        &root.join("data/random_sequences.dat"),
        &compound(vec![
            ("DataVersion", Tag::Int(3700)),
            (
                "data",
                compound(vec![
                    ("salt", Tag::Long(0)),
                    ("include_world_seed", Tag::Byte(1)),
                    ("include_sequence_id", Tag::Byte(1)),
                    ("sequences", compound(vec![])),
                ]),
            ),
        ]),
    )
    .unwrap();
    write_legacy_region(root, "poi", &legacy_poi(2860));
    write_legacy_region(root, "DIM-1/poi", &legacy_poi(2860));
    fs::write(root.join("poi/c.0.0.mcc"), b"opaque external POI payload").unwrap();
}

fn assert_modern_stone(tag: &Tag, count: i32, items: &mc_data::items::ItemRegistry) {
    assert_eq!(field(tag, "id"), &string("minecraft:stone"));
    assert_eq!(field(tag, "count"), &Tag::Int(count));
    assert!(absent(tag, "Count"));
    assert!(absent(tag, "tag"));
    let components = field(tag, "components");
    assert_eq!(
        field(field(components, "minecraft:custom_data"), "mod:opaque"),
        &Tag::Int(37)
    );
    assert!(matches!(
        field(components, "minecraft:custom_name"),
        Tag::Compound(_)
    ));
    let decoded = mc_data::item_nbt::stack_from_fields(items, fields(tag))
        .unwrap()
        .unwrap();
    assert_eq!(decoded.count, count);
}

fn owner_player_source(source: &Path) {
    fs::create_dir(source).unwrap();
    let mut level = legacy_level();
    let Tag::Compound(root) = &mut level else {
        unreachable!()
    };
    let Tag::Compound(data) = &mut root.iter_mut().find(|(key, _)| key == "Data").unwrap().1 else {
        unreachable!()
    };
    data.push(("Player".into(), legacy_player()));
    write_nbt(&source.join("level.dat"), &level).unwrap();
    write_legacy_region(source, "region", &legacy_chunk());
    let mut external = legacy_player();
    let Tag::Compound(fields) = &mut external else {
        unreachable!()
    };
    fields
        .iter_mut()
        .find(|(key, _)| key == "Health")
        .unwrap()
        .1 = Tag::Float(1.0);
    write_nbt(
        &source.join("playerdata/00000000-0000-0000-0000-000000000003.dat"),
        &external,
    )
    .unwrap();
}

#[test]
fn embedded_owner_player_wins_over_external_copy_with_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    owner_player_source(&source);
    let backup = b"opaque backup, intentionally not parsed as an active player";
    fs::write(
        source.join("playerdata/00000000-0000-0000-0000-000000000003.dat_old"),
        backup,
    )
    .unwrap();
    let original = hashes(&source).unwrap();
    let config = CoreConfig::new(native.clone(), 42);
    let receipt =
        import_anvil(&config, &source, &native, &temp.path().join("receipt.json")).unwrap();
    assert_eq!(receipt.counts["players"], 1);
    assert!(
        receipt
            .upgrades
            .iter()
            .flat_map(|entry| &entry.preserved)
            .any(|entry| {
                entry.path == "playerdata/00000000-0000-0000-0000-000000000003.dat"
                    && entry.reason.contains("omitted")
                    && entry.reason.contains("level.dat#Data.Player")
            })
    );
    let store = NativeStore::open(
        &native.join("native/state.redb"),
        bridge::native_identity(&config),
    )
    .unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    assert_eq!(checkpoint.players.len(), 1);
    let player = bridge::decode_tag(&checkpoint.players[0].record, "player").unwrap();
    assert_eq!(field(&player, "Health"), &Tag::Float(18.0));
    let backup_path = "players/data/00000000-0000-0000-0000-000000000003.dat_old";
    assert!(
        receipt
            .opaque_retention
            .iter()
            .any(|path| path == backup_path)
    );
    let extras = bridge::decode_tag(&checkpoint.globals["conversion_extras"], "extras").unwrap();
    assert_eq!(
        field(&extras, backup_path),
        &Tag::ByteArray(backup.iter().map(|byte| *byte as i8).collect())
    );
    assert_eq!(hashes(&source).unwrap(), original);
}

#[test]
fn versionless_player_runs_full_legacy_chain_with_numeric_items() {
    // No DataVersion at all: caller reports from=-1 (vanilla codec default),
    // which runs the full legacy fixer chain instead of skipping to 1343.
    // Synthetic: legacy numeric item ids + short Health + string dimension.
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    write_legacy_region(&source, "region", &legacy_chunk());
    let mut player = legacy_player();
    let Tag::Compound(fields) = &mut player else {
        unreachable!()
    };
    fields.retain(|(key, _)| key != "DataVersion");
    fields
        .iter_mut()
        .find(|(key, _)| key == "Health")
        .unwrap()
        .1 = Tag::Short(20);
    write_nbt(
        &source.join("playerdata/00000000-0000-0000-0000-000000000003.dat"),
        &player,
    )
    .unwrap();
    let original = hashes(&source).unwrap();
    let receipt = import_anvil(
        &CoreConfig::new(native.clone(), 42),
        &source,
        &native,
        &temp.path().join("receipt.json"),
    )
    .unwrap();
    assert_eq!(receipt.status, "PASS");
    let upgrade = receipt
        .upgrades
        .iter()
        .find(|entry| entry.record == "playerdata/00000000-0000-0000-0000-000000000003.dat")
        .unwrap();
    assert_eq!(upgrade.from, -1);
    assert_eq!(upgrade.to, 4790);
    let store = NativeStore::open(
        &native.join("native/state.redb"),
        bridge::native_identity(&CoreConfig::new(native.clone(), 42)),
    )
    .unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    assert_eq!(checkpoint.players.len(), 1);
    assert_eq!(hashes(&source).unwrap(), original);
}

#[test]
#[ignore = "requires local .analysis/maps/falcons-rock/vanilla-input real player"]
fn real_versionless_player_without_egg_tag_imports_and_exports_mooshroom_egg() {
    let filename = "06e6159a-ba55-4c07-b2ac-90a83aee984d.dat";
    let real_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.analysis/maps/falcons-rock/vanilla-input/players/data")
        .join(filename);
    assert!(real_path.is_file(), "requires {}", real_path.display());
    let real_bytes = fs::read(&real_path).unwrap();
    let real_player = read_nbt(&real_path).unwrap();
    assert!(absent(&real_player, "DataVersion"));
    assert_eq!(player_source_version(&real_player, &real_path).unwrap(), -1);
    let egg = &elements(field(&real_player, "Inventory"))[8];
    assert_eq!(field(egg, "id"), &string("minecraft:spawn_egg"));
    assert_eq!(field(egg, "Damage"), &Tag::Short(96));
    assert_eq!(field(egg, "Count"), &Tag::Byte(1));
    assert_eq!(field(egg, "Slot"), &Tag::Byte(8));
    assert!(absent(egg, "tag"));

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    write_legacy_region(&source, "region", &legacy_chunk());
    let relative = Path::new("players/data").join(filename);
    fs::create_dir_all(source.join("players/data")).unwrap();
    fs::write(source.join(&relative), &real_bytes).unwrap();
    let original = hashes(&source).unwrap();
    let config = CoreConfig::new(native.clone(), 42);
    let receipt =
        import_anvil(&config, &source, &native, &temp.path().join("import.json")).unwrap();
    assert_eq!(receipt.status, "PASS");
    let upgraded = receipt
        .upgrades
        .iter()
        .find(|entry| entry.record == relative.to_str().unwrap())
        .unwrap();
    assert_eq!((upgraded.from, upgraded.to), (-1, 4790));
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let egg_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:mooshroom_spawn_egg").unwrap())
        .unwrap();
    let store = NativeStore::open(
        &native.join("native/state.redb"),
        bridge::native_identity(&config),
    )
    .unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    assert_eq!(checkpoint.players.len(), 1);
    assert_eq!(
        checkpoint.players[0].uuid,
        *uuid::Uuid::parse_str(filename.trim_end_matches(".dat"))
            .unwrap()
            .as_bytes()
    );
    let native_player = bridge::decode_tag(&checkpoint.players[0].record, "player").unwrap();
    assert_eq!(field(&native_player, "DataVersion"), &Tag::Int(4790));
    let player = player_from_fields(
        &registry.items,
        VanillaPlayer::default(),
        fields(&native_player),
    )
    .unwrap();
    let (_, egg) = player
        .inventory
        .iter()
        .find(|(slot, _)| *slot == 8)
        .unwrap();
    assert_eq!((egg.item_id, egg.count), (egg_id, 1));
    drop(checkpoint);
    drop(store);

    let exported = temp.path().join("exported");
    export_anvil(
        &config,
        &native,
        &exported,
        &temp.path().join("export.json"),
    )
    .unwrap();
    let exported_player = read_nbt(&exported.join(&relative)).unwrap();
    let exported = player_from_fields(
        &registry.items,
        VanillaPlayer::default(),
        fields(&exported_player),
    )
    .unwrap();
    assert_eq!(exported, player);
    assert_eq!(hashes(&source).unwrap(), original);
    assert_eq!(fs::read(real_path).unwrap(), real_bytes);
}

#[test]
fn original_saved_data_unversioned_envelopes_roundtrip_without_source_mutation() {
    let path = Path::new("data/scoreboard.dat");
    assert_eq!(
        saved_data_source_version(&compound(vec![]), path).unwrap(),
        1343
    );
    for version in [-1, 1976, 2230, 2580, 2586, 4790] {
        assert_eq!(
            saved_data_source_version(&compound(vec![("DataVersion", Tag::Int(version))]), path)
                .unwrap(),
            version
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    let exported = temp.path().join("exported");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    write_legacy_region(&source, "region", &legacy_chunk());
    let names = [
        "scoreboard",
        "Mineshaft",
        "Monument",
        "Stronghold",
        "Temple",
        "Village",
        "villages",
        "villages_end",
        "villages_nether",
    ];
    let mut retained = Vec::new();
    for name in names {
        let relative = PathBuf::from(format!("data/{name}.dat"));
        let tag = compound(vec![(
            "data",
            compound(vec![
                ("OpaqueLegacyMarker", Tag::Short(37)),
                ("Features", compound(vec![])),
            ]),
        )]);
        write_nbt(&source.join(&relative), &tag).unwrap();
        retained.push((relative.clone(), fs::read(source.join(relative)).unwrap()));
    }
    let original = hashes(&source).unwrap();
    let config = CoreConfig::new(native.clone(), 42);
    let receipt =
        import_anvil(&config, &source, &native, &temp.path().join("import.json")).unwrap();
    assert_eq!(receipt.status, "PASS");
    for (relative, _) in &retained {
        if import::retired_saved_data(relative) {
            assert!(
                receipt
                    .upgrades
                    .iter()
                    .flat_map(|record| &record.preserved)
                    .any(|entry| entry.path == relative.to_str().unwrap()
                        && entry.reason.contains("retired vanilla")
                        && entry.reason.contains("not decoded"))
            );
            continue;
        }
        let record = receipt
            .upgrades
            .iter()
            .find(|record| record.record == relative.to_str().unwrap())
            .unwrap();
        assert_eq!((record.from, record.to), (1343, 1343));
        assert!(record.steps.is_empty());
        assert!(
            record
                .preserved
                .iter()
                .any(|entry| entry.reason.contains("deferred"))
        );
    }
    assert_eq!(hashes(&source).unwrap(), original);
    let export = export_anvil(
        &config,
        &native,
        &exported,
        &temp.path().join("export.json"),
    )
    .unwrap();
    assert_eq!(export.status, "PASS");
    for (relative, bytes) in retained {
        let destination = auxiliary_destination(&relative).unwrap();
        assert_eq!(fs::read(exported.join(destination)).unwrap(), bytes);
    }
    assert_eq!(hashes(&source).unwrap(), original);
    for (name, tag, expected) in [
        (
            "scoreboard",
            compound(vec![("DataVersion", Tag::Int(4791))]),
            "newer than",
        ),
        (
            "scoreboard",
            compound(vec![("DataVersion", Tag::Short(1343))]),
            "invalid DataVersion",
        ),
        (
            "scoreboard",
            compound(vec![
                ("DataVersion", Tag::Int(1343)),
                ("DataVersion", Tag::Int(4790)),
            ]),
            "duplicate DataVersion",
        ),
        (
            "not_vanilla_saved_data",
            compound(vec![("data", compound(vec![]))]),
            "unsupported saved-data semantics",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let native = temp.path().join("native");
        fs::create_dir(&source).unwrap();
        write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
        write_legacy_region(&source, "region", &legacy_chunk());
        write_nbt(&source.join(format!("data/{name}.dat")), &tag).unwrap();
        let before = hashes(&source).unwrap();
        let error = import_anvil(
            &CoreConfig::new(native.clone(), 42),
            &source,
            &native,
            &temp.path().join("refused.json"),
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert!(!native.exists());
        assert_eq!(hashes(&source).unwrap(), before);
    }
}

#[test]
#[ignore = "requires local .analysis/maps/falcons-rock/vanilla-input real records"]
fn real_vanilla_input_records_upgrade_and_decode_read_only() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.analysis/maps/falcons-rock/vanilla-input");
    real_input_records_upgrade_and_decode_read_only(&root);
}

#[test]
#[ignore = "requires local original Falcon's Rock records"]
fn original_saved_data_real_records_upgrade_and_decode_read_only() {
    let root = std::env::var_os("SOLARIS_REAL_WORLD_PREFLIGHT_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.analysis/maps/falcons-rock/original-input")
        });
    real_input_records_upgrade_and_decode_read_only(&root);
}

fn real_input_records_upgrade_and_decode_read_only(root: &Path) {
    use crate::dimensions::Dimension;
    use mc_world::anvil::upgrade;
    assert!(root.is_dir(), "requires {}", root.display());
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let mut failures = Vec::new();
    let mut metadata = crate::level_metadata::MetadataTags::new();
    let mut counts = BTreeMap::<&str, usize>::new();
    let mut losses = Vec::new();
    let mut manifest = CheckpointManifest::default();
    for dimension in [Dimension::Nether, Dimension::End] {
        manifest
            .dimensions
            .insert(dimension, Box::new(CheckpointManifest::default()));
    }
    for relative in files(root).unwrap() {
        let player = matches!(
            relative.parent().and_then(Path::to_str),
            Some("players/data" | "playerdata")
        );
        let level = relative == Path::new("level.dat");
        let saved_data = !player && relative.components().any(|part| part.as_os_str() == "data");
        if relative.extension().and_then(|value| value.to_str()) != Some("dat")
            || !(player || level || saved_data)
        {
            continue;
        }
        let kind = if player {
            "players"
        } else if level {
            "level"
        } else {
            "saved_data"
        };
        *counts.entry(kind).or_default() += 1;
        let path = root.join(&relative);
        let before = fs::read(&path).unwrap();
        let result = (|| -> Result<(), CoreError> {
            if saved_data && import::retired_saved_data(&auxiliary_destination(&relative)?) {
                *counts.entry("opaque_retired_saved_data").or_default() += 1;
                return Ok(());
            }
            let mut tag = read_nbt(&path)?;
            if player {
                let source_location = import::player_missing_dimension_source(&tag)?;
                let from = player_source_version(&tag, &path)?;
                upgrade::upgrade_player(&mut tag, from).map_err(fail)?;
                if source_location.is_some() {
                    if manifest.world_spawn.is_none() {
                        crate::level_metadata::import_tags(&metadata, &mut manifest)?;
                    }
                    let retained = |tag: &Tag| {
                        fields(tag)
                            .iter()
                            .filter(|(key, _)| {
                                !matches!(key.as_str(), "Dimension" | "Pos" | "respawn")
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    };
                    let unrelated_before = retained(&tag);
                    import::normalize_missing_player_dimensions(
                        &mut tag,
                        source_location,
                        &manifest,
                        &relative.to_string_lossy(),
                        &mut losses,
                    )?;
                    assert_eq!(retained(&tag), unrelated_before, "{}", relative.display());
                    assert_eq!(field(&tag, "Dimension"), &string("minecraft:overworld"));
                    if relative.file_stem().unwrap() == "78f89b8e-7d93-47f7-81fb-4ca6ec4804de" {
                        assert!(absent(&tag, "respawn"));
                    }
                }
                let player = crate::journal::player_from_tag(&tag, &registry.items)?;
                if player.pos.iter().any(|value| !value.is_finite())
                    || !player.yaw.is_finite()
                    || !player.pitch.is_finite()
                    || !player.health.is_finite()
                {
                    return Err(fail("non-finite player"));
                }
            } else {
                let destination = auxiliary_destination(&relative)?;
                if level {
                    let from = source_version(&tag, &path)?;
                    let report = upgrade::upgrade_level(&mut tag, from).map_err(fail)?;
                    for saved in report.saved_data {
                        metadata.insert(saved.path, saved.tag);
                    }
                } else if destination == Path::new("data/minecraft/scoreboard.dat") {
                    let from = saved_data_source_version(&tag, &path)?;
                    if from != mc_world::anvil::TARGET_DATA_VERSION {
                        *counts.entry("deferred_saved_data").or_default() += 1;
                        return Ok(());
                    }
                    crate::scoreboard::Scoreboard::from_nbt(&tag).map_err(fail)?;
                } else if matches!(
                    destination.file_name().and_then(|name| name.to_str()),
                    Some("raids.dat" | "ender_dragon_fight.dat")
                ) {
                    let from = saved_data_source_version(&tag, &path)?;
                    let raids = destination.file_name().unwrap() == "raids.dat";
                    if raids && (2586..4309).contains(&from) {
                        let Tag::Compound(root) = &mut tag else {
                            return Err(fail("expected compound saved-data root"));
                        };
                        let Some((_, Tag::Compound(data))) =
                            root.iter_mut().find(|(key, _)| key == "data")
                        else {
                            return Err(fail("expected compound saved data"));
                        };
                        import::upgrade_raids_payload_for_tests(data, from, &mut Vec::new());
                        let (_, stamp) = root
                            .iter_mut()
                            .find(|(key, _)| key == "DataVersion")
                            .unwrap();
                        *stamp = Tag::Int(mc_world::anvil::TARGET_DATA_VERSION);
                        crate::raids::from_nbt(&tag).map_err(fail)?;
                    } else if from == mc_world::anvil::TARGET_DATA_VERSION {
                        if raids {
                            crate::raids::from_nbt(&tag).map_err(fail)?;
                        } else {
                            crate::end_fight::from_nbt(&tag).map_err(fail)?;
                        }
                    } else {
                        // Exactly like the importer: retain old typed-owner
                        // envelopes byte-for-byte instead of decoding as4790.
                        *counts.entry("deferred_saved_data").or_default() += 1;
                        return Ok(());
                    }
                } else if destination.starts_with("data/minecraft/maps")
                    && destination
                        .file_name()
                        .is_none_or(|name| name != "last_id.dat")
                {
                    let from = saved_data_source_version(&tag, &path)?;
                    upgrade::upgrade_map_data(&mut tag, from).map_err(fail)?;
                } else {
                    let from = saved_data_source_version(&tag, &path)?;
                    if from != mc_world::anvil::TARGET_DATA_VERSION
                        && check_version(&tag, &path).is_err()
                    {
                        *counts.entry("deferred_saved_data").or_default() += 1;
                        return Ok(());
                    }
                }
                metadata.insert(destination.to_string_lossy().into_owned(), tag);
            }
            Ok(())
        })();
        if let Err(error) = result {
            failures.push(format!("{}: {error}", relative.display()));
        }
        assert_eq!(fs::read(&path).unwrap(), before, "{}", path.display());
    }
    if let Err(error) = crate::level_metadata::import_tags(&metadata, &mut manifest) {
        failures.push(format!("metadata typed decode: {error}"));
    }
    eprintln!(
        "real-record preflight: {}",
        serde_json::json!({"counts": counts, "failures": failures, "preservation_losses": losses})
    );
    assert!(counts.get("players").is_some_and(|count| *count > 0));
    assert_eq!(
        losses.len(),
        3,
        "expected all three real absent-dimension players"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn saved_maps_upgrade_activates_native_rows_and_exports_without_source_mutation() {
    // Upgrade and activate native map state, never leave serving maps in extras.
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    write_legacy_region(&source, "region", &legacy_chunk());
    let legacy_map = compound(vec![
        ("DataVersion", Tag::Int(1976)),
        (
            "data",
            compound(vec![
                ("dimension", Tag::Byte(0)),
                ("xCenter", Tag::Int(0)),
                ("zCenter", Tag::Int(0)),
                ("colors", Tag::ByteArray(vec![0; 128 * 128])),
            ]),
        ),
    ]);
    write_nbt(&source.join("data/map_7.dat"), &legacy_map).unwrap();
    let original_bytes = fs::read(source.join("data/map_7.dat")).unwrap();
    let original = hashes(&source).unwrap();
    let config = CoreConfig::new(native.clone(), 42);
    let receipt =
        import_anvil(&config, &source, &native, &temp.path().join("receipt.json")).unwrap();
    assert_eq!(receipt.status, "PASS");
    let map_upgrade = receipt
        .upgrades
        .iter()
        .find(|entry| entry.record == "data/map_7.dat" && entry.kind == "saved_data")
        .unwrap();
    assert_eq!(map_upgrade.from, 1976);
    let store = NativeStore::open(
        &native.join("native/state.redb"),
        bridge::native_identity(&config),
    )
    .unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    let extras = bridge::decode_tag(&checkpoint.globals["conversion_extras"], "extras").unwrap();
    assert!(
        matches!(&extras,Tag::Compound(fields) if fields.iter().all(|(key,_)| key != "data/minecraft/maps/7.dat"))
    );
    let maps = crate::maps::recover(&checkpoint.globals).unwrap();
    assert_eq!(maps[&7].dimension, Dimension::Overworld);
    assert_eq!(maps[&7].colors.len(), 16384);
    assert_eq!(crate::maps::sequence(&checkpoint.globals).unwrap(), 8);
    drop(checkpoint);
    drop(store);
    let output = temp.path().join("exported");
    export_anvil(&config, &native, &output, &temp.path().join("export.json")).unwrap();
    assert_ne!(
        fs::read(output.join("data/minecraft/maps/7.dat")).unwrap(),
        original_bytes
    );
    assert_eq!(hashes(&source).unwrap(), original);
}

#[test]
fn stamped_legacy_embedded_player_uses_own_chain() {
    // level.dat#Data.Player carrying its own stamp upgrades through the
    // typed Player walker at that version, not the level's version.
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    fs::create_dir(&source).unwrap();
    let mut level = legacy_level();
    let Tag::Compound(root) = &mut level else {
        unreachable!()
    };
    let Tag::Compound(data) = &mut root.iter_mut().find(|(key, _)| key == "Data").unwrap().1 else {
        unreachable!()
    };
    let mut player = legacy_player();
    let Tag::Compound(fields) = &mut player else {
        unreachable!()
    };
    fields
        .iter_mut()
        .find(|(key, _)| key == "DataVersion")
        .unwrap()
        .1 = Tag::Int(1976);
    data.push(("Player".into(), player));
    write_nbt(&source.join("level.dat"), &level).unwrap();
    write_legacy_region(&source, "region", &legacy_chunk());
    let receipt = import_anvil(
        &CoreConfig::new(native.clone(), 42),
        &source,
        &native,
        &temp.path().join("receipt.json"),
    )
    .unwrap();
    let upgrade = receipt
        .upgrades
        .iter()
        .find(|entry| entry.record == "level.dat#Data.Player")
        .unwrap();
    assert_eq!(upgrade.from, 1976);
    assert_eq!(receipt.counts["players"], 1);
}

#[test]
fn embedded_owner_does_not_allow_duplicate_external_player_locations() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    owner_player_source(&source);
    write_nbt(
        &source.join("players/data/00000000-0000-0000-0000-000000000003.dat"),
        &legacy_player(),
    )
    .unwrap();
    let error = import_anvil(
        &CoreConfig::new(native.clone(), 42),
        &source,
        &native,
        &temp.path().join("receipt.json"),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("ambiguous old/new active player locations")
    );
    assert!(!native.exists());
}

#[test]
fn embedded_owner_does_not_mask_external_player_identity_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    owner_player_source(&source);
    let mut external = legacy_player();
    let Tag::Compound(fields) = &mut external else {
        unreachable!()
    };
    fields.iter_mut().find(|(key, _)| key == "UUID").unwrap().1 = Tag::IntArray(vec![0, 0, 0, 4]);
    fs::remove_file(source.join("playerdata/00000000-0000-0000-0000-000000000003.dat")).unwrap();
    write_nbt(
        &source.join("playerdata/00000000-0000-0000-0000-000000000003.dat"),
        &external,
    )
    .unwrap();
    let error = import_anvil(
        &CoreConfig::new(native.clone(), 42),
        &source,
        &native,
        &temp.path().join("receipt.json"),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("player UUID disagrees with source identity")
    );
    assert!(!native.exists());
}

#[test]
fn java118_old_layout_import_anvil_upgrades_all_records_and_native_readback() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("java118");
    let native = temp.path().join("native");
    let receipt_path = temp.path().join("import-receipt.json");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    write_legacy_region(&source, "region", &legacy_chunk());
    write_legacy_region(&source, "entities", &legacy_entities());
    write_nbt(
        &source.join("playerdata/00000000-0000-0000-0000-000000000003.dat"),
        &legacy_player(),
    )
    .unwrap();
    write_legacy_auxiliary(&source);
    assert!(!source.join("dimensions").exists());
    assert!(!source.join("players").exists());
    let original: BTreeMap<_, _> = files(&source)
        .unwrap()
        .into_iter()
        .map(|path| {
            let bytes = fs::read(source.join(&path)).unwrap();
            (path, bytes)
        })
        .collect();
    let original_hashes = hashes(&source).unwrap();
    let config = CoreConfig::new(native.clone(), 42);
    let receipt = import_anvil(&config, &source, &native, &receipt_path).unwrap();
    assert_eq!(receipt.status, "PASS");
    assert_eq!(receipt.operation, "import-anvil");
    assert_eq!(receipt.data_version, 4790);
    assert_eq!(receipt.checkpoint_tick, Some(79));
    assert_eq!(receipt.replay_tick, Some(79));
    assert!(receipt.error.is_none());
    assert_eq!(receipt.counts["chunks"], 1);
    assert_eq!(receipt.counts["entities"], 1);
    assert_eq!(receipt.counts["players"], 1);
    assert_eq!(receipt.counts["reminted_ids"], 1);
    assert_eq!(receipt.input_hashes, original_hashes);
    let persisted: ConversionReceipt =
        serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    assert_eq!(persisted.status, "PASS");
    assert_eq!(
        serde_json::to_value(&persisted.upgrades).unwrap(),
        serde_json::to_value(&receipt.upgrades).unwrap()
    );
    let mut kinds: Vec<_> = receipt
        .upgrades
        .iter()
        .filter(|upgrade| upgrade.kind != "saved_data" && upgrade.kind != "poi")
        .map(|upgrade| upgrade.kind.as_str())
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["chunk", "entity_region", "level", "player"]);
    for upgrade in receipt
        .upgrades
        .iter()
        .filter(|upgrade| upgrade.kind != "saved_data" && upgrade.kind != "poi")
    {
        assert_eq!((upgrade.from, upgrade.to), (2860, 4790));
        assert!(!upgrade.record.is_empty());
        // Steps record fixer applications in walker order, not version
        // order (same-version sub-fixers and legacy hooks interleave);
        // each step's version must still be within the upgrade range.
        assert!(
            upgrade
                .steps
                .iter()
                .all(|step| !step.fix.is_empty() && step.version > 2860 && step.version <= 4790)
        );
    }
    for (source_path, canonical_path) in DEFERRED_SAVED_PATHS {
        let upgrade = receipt
            .upgrades
            .iter()
            .find(|upgrade| upgrade.kind == "saved_data" && upgrade.record == source_path)
            .unwrap_or_else(|| panic!("missing deferred receipt for {source_path}"));
        let version = if source_path == "data/random_sequences.dat" {
            3700
        } else {
            2860
        };
        assert_eq!((upgrade.from, upgrade.to), (version, version));
        assert!(upgrade.steps.is_empty(), "{source_path}: fake NBT upgrade");
        assert!(upgrade.saved_data.is_empty());
        assert!(
            upgrade
                .preserved
                .iter()
                .any(|retained| retained.path == canonical_path && !retained.reason.is_empty()),
            "{source_path}: missing explicit byte-retention provenance"
        );
    }
    let (map_source_path, map_canonical_path) = UPGRADED_MAP_PATH;
    let map_upgrade = receipt
        .upgrades
        .iter()
        .find(|upgrade| upgrade.kind == "saved_data" && upgrade.record == map_source_path)
        .expect("map upgrade receipt");
    assert_eq!((map_upgrade.from, map_upgrade.to), (2860, 4790));
    // Raids are no longer deferred raw bytes: the 2860 PascalCase payload
    // runs RaidRenamesDataFix4309 (→ lowercase codec fields, stamped 4790)
    // and lands in typed native custody per dimension.
    for (source_path, _canonical_path) in RAID_SAVED_PATHS {
        assert!(
            receipt.upgrades.iter().all(|upgrade| {
                !(upgrade.kind == "saved_data" && upgrade.record == source_path)
            }),
            "{source_path}: raids must not carry a deferred raw-bytes receipt"
        );
    }
    let level_upgrade = receipt
        .upgrades
        .iter()
        .find(|upgrade| upgrade.kind == "level")
        .unwrap();
    assert!(
        level_upgrade.steps.iter().any(|step| step.version == 4309),
        "raids 4309 rename step missing from level receipt"
    );
    assert!(
        level_upgrade.preserved.iter().any(|retained| retained.path
            == "dimensions/minecraft/overworld/data/minecraft/raids.dat"
            && !retained.reason.is_empty()),
        "overworld raids missing upgrade provenance"
    );
    assert!(
        receipt.upgrades.iter().all(|upgrade| upgrade.kind != "poi"),
        "opaque POI must not claim inspected versions or NBT upgrades"
    );
    let level_retention = &receipt
        .upgrades
        .iter()
        .find(|upgrade| upgrade.kind == "level")
        .unwrap()
        .preserved;
    for (_, canonical_path) in DEFERRED_POI_PATHS.into_iter().chain([(
        "poi/c.0.0.mcc",
        "dimensions/minecraft/overworld/poi/c.0.0.mcc",
    )]) {
        assert!(
            level_retention
                .iter()
                .any(|entry| entry.path == canonical_path && !entry.reason.is_empty()),
            "{canonical_path}: missing opaque POI retention provenance"
        );
    }
    let level_upgrade = receipt
        .upgrades
        .iter()
        .find(|upgrade| upgrade.kind == "level")
        .unwrap();
    assert_eq!(level_upgrade.record, "level.dat");
    assert!(
        level_upgrade
            .steps
            .iter()
            .any(|step| step.version == 4772 && step.fix == "LevelDatToSavedDataFileFix")
    );
    let expected_saved_paths = [
        "data/minecraft/game_rules.dat",
        "data/minecraft/scheduled_events.dat",
        "data/minecraft/wandering_trader.dat",
        "data/minecraft/weather.dat",
        "data/minecraft/world_clocks.dat",
        "data/minecraft/world_gen_settings.dat",
        "dimensions/minecraft/overworld/data/minecraft/world_border.dat",
        "dimensions/minecraft/the_end/data/minecraft/ender_dragon_fight.dat",
        "dimensions/minecraft/the_end/data/minecraft/world_border.dat",
        "dimensions/minecraft/the_nether/data/minecraft/world_border.dat",
    ];
    let mut saved_paths: Vec<_> = level_upgrade
        .saved_data
        .iter()
        .map(String::as_str)
        .collect();
    saved_paths.sort_unstable();
    assert_eq!(saved_paths, expected_saved_paths);

    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let store = NativeStore::open(
        &native.join("native/state.redb"),
        bridge::native_identity(&config),
    )
    .unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    assert_eq!(checkpoint.tick, 79);
    let manifest: CheckpointManifest =
        serde_json::from_slice(&checkpoint.globals["checkpoint"].payload).unwrap();
    // Raids custody: the 2860 PascalCase payload (NextAvailableID 4,
    // Tick 19, empty Raids) upgraded to codec fields per dimension.
    use crate::dimensions::Dimension;
    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let dimension_manifest = if dimension == Dimension::Overworld {
            &manifest
        } else {
            manifest
                .dimensions
                .get(&dimension)
                .unwrap_or_else(|| panic!("{dimension:?}: child manifest missing"))
                .as_ref()
        };
        let raids = dimension_manifest
            .raids
            .as_ref()
            .unwrap_or_else(|| panic!("{dimension:?}: raids custody missing"));
        assert_eq!(raids.next_id, 4, "{dimension:?}: NextAvailableID");
        assert_eq!(raids.tick, 19, "{dimension:?}: Tick");
        assert!(
            raids.raids.is_empty(),
            "{dimension:?}: expected no active raids"
        );
    }
    assert_eq!(manifest.domain_versions, bridge::domain_versions());
    assert_eq!(manifest.difficulty, crate::survival::Difficulty::Hard);
    assert_eq!(manifest.world_spawn, Some([12, 75, -45]));
    assert_eq!(manifest.world_spawn_rotation, [90.0, 0.0]);
    let climate = manifest.climate.unwrap();
    assert_eq!(climate.day_time, 18000);
    assert_eq!(climate.weather_kind, 2);
    assert_eq!(climate.remaining_ticks, 2345);
    assert_eq!(climate.thunder_remaining_ticks, 1234);
    assert_eq!(climate.rain_level_bits, 1.0_f32.to_bits());
    assert_eq!(climate.thunder_level_bits, 1.0_f32.to_bits());
    assert!(!climate.advance_time);
    assert!(climate.advance_weather);
    assert_eq!(manifest.block_ticks.chunks[0].ticks[0].trigger_tick, 91);
    assert_eq!(manifest.block_ticks.chunks[0].ticks[0].priority, -1);
    assert_eq!(manifest.fluid_ticks.due[0].deadline, 95);
    assert_eq!(manifest.border.size, 59_999_968.0);
    let metadata_tag =
        bridge::decode_tag(&checkpoint.globals["level_metadata"], "level_metadata").unwrap();
    let metadata = crate::level_metadata::from_tag(&metadata_tag).unwrap();
    for path in expected_saved_paths {
        assert_eq!(
            field(&metadata[path], "DataVersion"),
            &Tag::Int(4790),
            "{path}"
        );
    }
    let level = field(&metadata["level.dat"], "Data");
    assert_eq!(field(level, "DataVersion"), &Tag::Int(4790));
    assert_eq!(field(level, "mod:level"), &Tag::LongArray(vec![9, 8, 7]));
    for key in [
        "WorldGenSettings",
        "GameRules",
        "DayTime",
        "SpawnX",
        "raining",
    ] {
        assert!(absent(level, key), "legacy field {key} survived cutover");
    }
    let settings = field(&metadata["data/minecraft/world_gen_settings.dat"], "data");
    assert_eq!(field(settings, "seed"), &Tag::Long(42));
    assert_eq!(field(settings, "generate_structures"), &Tag::Byte(1));
    assert_eq!(fields(field(settings, "dimensions")).len(), 3);
    let border_path = "dimensions/minecraft/overworld/data/minecraft/world_border.dat";
    let border = field(&metadata[border_path], "data");
    assert_eq!(field(border, "warning_time"), &Tag::Int(15));
    assert_eq!(field(border, "warning_blocks"), &Tag::Int(9));

    let chunk = store
        .resolve_base(
            0,
            ChunkPos { x: 0, z: 0 },
            &registry.blocks,
            &registry.items,
            &registry.chest_loot,
            || panic!("imported terrain must not fall back to procedural generation"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(chunk.geometry(), mc_world::OVERWORLD_GEOMETRY);
    assert_eq!(chunk.status, "minecraft:full");
    assert_eq!(chunk.inhabited_time(), 800);
    assert_eq!(
        registry
            .blocks
            .by_id(chunk.get_block(0, 16, 0).unwrap())
            .unwrap()
            .block
            .id
            .to_string(),
        "minecraft:short_grass"
    );
    assert_eq!(
        registry
            .blocks
            .by_id(chunk.get_block(1, 0, 1).unwrap())
            .unwrap()
            .block
            .id
            .to_string(),
        "minecraft:chest"
    );
    assert_eq!(chunk.biomes[0].get(0, 0, 0).to_string(), "minecraft:plains");
    assert!(
        chunk
            .extras
            .iter()
            .any(|(name, tag)| name == "mod:terrain" && tag == &string("retained"))
    );
    assert_eq!(
        chunk.scheduled_block_ticks[0].block.to_string(),
        "minecraft:short_grass"
    );
    assert_eq!(chunk.scheduled_block_ticks[0].trigger_tick, 91);
    assert_eq!(
        chunk.scheduled_fluid_ticks[0].fluid.to_string(),
        "minecraft:flowing_water"
    );
    assert_eq!(chunk.scheduled_fluid_ticks[0].trigger_tick, 95);
    let chest = &chunk.chests[&BlockPos { x: 1, y: 0, z: 1 }];
    assert_eq!(chest.slots[0].count, 1);
    let container = chest.slots[0]
        .extended()
        .preserved_components
        .iter()
        .find(|component| component.name == "minecraft:container")
        .unwrap();
    let container = mc_nbt::read_network_value(&mut container.value.as_slice()).unwrap();
    let entry = &elements(&container)[0];
    assert_eq!(field(entry, "slot"), &Tag::Int(0));
    assert_modern_stone(field(entry, "item"), 3, &registry.items);

    assert_eq!(checkpoint.entities.len(), 1);
    let entity_tag = checkpoint.entities[0].payload.as_ref().unwrap();
    let entity = entity_from_fields(
        &registry.items,
        VanillaEntity::default(),
        fields(entity_tag),
    )
    .unwrap();
    assert_eq!(entity.id, "minecraft:item");
    assert_eq!(entity.uuid, uuid::Uuid::from_u128(7));
    assert_eq!(entity.pos, [1.0, 65.0, 1.0]);
    assert_eq!(entity.pickup_delay, 23);
    assert_eq!(entity.item.as_ref().unwrap().count, 4);
    assert!(entity.wire_id > 0);
    assert_eq!(field(entity_tag, "mod:entity"), &string("retained"));
    assert_modern_stone(field(entity_tag, "Item"), 4, &registry.items);

    assert_eq!(checkpoint.players.len(), 1);
    assert_eq!(
        checkpoint.players[0].uuid,
        *uuid::Uuid::from_u128(3).as_bytes()
    );
    let player_tag = bridge::decode_tag(&checkpoint.players[0].record, "player").unwrap();
    assert_eq!(field(&player_tag, "DataVersion"), &Tag::Int(4790));
    let player = player_from_fields(
        &registry.items,
        VanillaPlayer::default(),
        fields(&player_tag),
    )
    .unwrap();
    assert_eq!(player.pos, [12.0, 80.0, -7.0]);
    assert_eq!((player.yaw, player.pitch), (45.0, 10.0));
    assert_eq!(player.health, 18.0);
    assert_eq!(player.food, 17);
    assert_eq!(player.inventory.len(), 1);
    assert_eq!(player.inventory[0].1.count, 5);
    assert!(
        player
            .equipment
            .iter()
            .any(|(slot, stack)| slot == "feet" && stack.count == 1)
    );
    assert!(
        player
            .equipment
            .iter()
            .any(|(slot, stack)| slot == "offhand" && stack.count == 2)
    );
    assert_eq!(field(&player_tag, "mod:player"), &string("retained"));
    assert!(absent(&player_tag, "SpawnX"));
    assert_eq!(
        field(field(&player_tag, "respawn"), "pos"),
        &Tag::IntArray(vec![10, 70, 20])
    );
    assert_modern_stone(
        &elements(field(&player_tag, "Inventory"))[0],
        5,
        &registry.items,
    );
    drop(store);

    // Export refreshes metadata through the current native checkpoint reader;
    // it must not silently replace legacy warning seconds with modern ticks.
    let output = temp.path().join("exported");
    let export_receipt = export_anvil(
        &config,
        &native,
        &output,
        &temp.path().join("export-receipt.json"),
    )
    .unwrap();
    assert_eq!(export_receipt.status, "PASS");
    let exported_border = read_nbt(&output.join(border_path)).unwrap();
    let exported_border = field(&exported_border, "data");
    assert_eq!(field(exported_border, "warning_time"), &Tag::Int(15));
    assert_eq!(field(exported_border, "warning_blocks"), &Tag::Int(9));
    for (source_path, canonical_path) in DEFERRED_SAVED_PATHS {
        assert_eq!(
            fs::read(output.join(canonical_path)).unwrap(),
            original[Path::new(source_path)],
            "{source_path} must export byte-exact at {canonical_path}"
        );
        let source_tag = read_nbt(&source.join(source_path)).unwrap();
        let exported_tag = read_nbt(&output.join(canonical_path)).unwrap();
        assert_eq!(
            field(&exported_tag, "DataVersion"),
            field(&source_tag, "DataVersion"),
            "{canonical_path} must retain original DataVersion"
        );
        assert!(!output.join(source_path).exists());
    }
    let source_map = read_nbt(&source.join(map_source_path)).unwrap();
    let exported_map = read_nbt(&output.join(map_canonical_path)).unwrap();
    assert_eq!(field(&exported_map, "DataVersion"), &Tag::Int(4790));
    let source_map = crate::maps::nbt::from_tag(&source_map).unwrap();
    let exported_map = crate::maps::nbt::from_tag(&exported_map).unwrap();
    assert_eq!(exported_map.colors, source_map.colors);
    assert_eq!(exported_map.center, source_map.center);
    assert_eq!(exported_map.scale, source_map.scale);
    assert!(!output.join(map_source_path).exists());
    // Raids export from native custody (upgraded 4309 codec fields,
    // stamped 4790), not the raw 2860 source bytes.
    for (source_path, canonical_path) in RAID_SAVED_PATHS {
        let exported_tag = read_nbt(&output.join(canonical_path)).unwrap();
        assert_eq!(
            field(&exported_tag, "DataVersion"),
            &Tag::Int(4790),
            "{canonical_path}: raids export must carry current stamp"
        );
        let data = field(&exported_tag, "data");
        assert!(
            fields(data).iter().any(|(name, _)| name == "raids"),
            "{canonical_path}: raids export must use codec fields"
        );
        assert!(
            fields(data).iter().all(|(name, _)| name != "Raids"),
            "{canonical_path}: PascalCase must not survive export"
        );
        assert!(!output.join(source_path).exists());
    }
    for (source_path, canonical_path) in DEFERRED_POI_PATHS {
        assert_eq!(
            fs::read(output.join(canonical_path)).unwrap(),
            original[Path::new(source_path)],
            "{source_path} MCA must export byte-exact at {canonical_path}"
        );
        let payloads = mc_world::anvil::read_region(output.join(canonical_path)).unwrap();
        assert_eq!(payloads.len(), 1);
        let (_, tag) = mc_nbt::read_named(&mut payloads[0].uncompressed_nbt.as_slice()).unwrap();
        assert_eq!(field(&tag, "DataVersion"), &Tag::Int(2860));
        assert_eq!(tag, legacy_poi(2860));
        assert!(!output.join(source_path).exists());
    }
    assert_eq!(
        fs::read(output.join("dimensions/minecraft/overworld/poi/c.0.0.mcc")).unwrap(),
        original[Path::new("poi/c.0.0.mcc")]
    );
    assert_eq!(
        files(&source).unwrap(),
        original.keys().cloned().collect::<Vec<_>>()
    );
    for (path, bytes) in original {
        assert_eq!(fs::read(source.join(path)).unwrap(), bytes);
    }
    assert_eq!(hashes(&source).unwrap(), original_hashes);
}

#[test]
fn modern_layout_poi_with_mixed_old_versions_is_preserved_byte_exact() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    write_legacy_region(
        &source,
        "dimensions/minecraft/overworld/region",
        &legacy_chunk(),
    );
    let poi_path = "dimensions/minecraft/overworld/poi/r.0.0.mca";
    fs::create_dir_all(source.join("dimensions/minecraft/overworld/poi")).unwrap();
    let records: Vec<_> = [2586, 2860, 3700]
        .into_iter()
        .enumerate()
        .map(|(index, version)| {
            let mut bytes = Vec::new();
            mc_nbt::write_named(&mut bytes, "", &legacy_poi(version)).unwrap();
            mc_world::anvil::ChunkPayload {
                local_x: index as u8,
                local_z: 0,
                timestamp: 123 + index as u32,
                uncompressed_nbt: bytes,
            }
        })
        .collect();
    mc_world::anvil::write_region(source.join(poi_path), &records).unwrap();
    let opaque_paths = [
        "dimensions/minecraft/overworld/poi/r.1.0.mca",
        "dimensions/minecraft/overworld/poi/c.0.0.mcc",
    ];
    for path in opaque_paths {
        // POI has no pure-NBT conversion: even uninterpreted headers and
        // external payloads belong to vanilla's deferred load-time handling.
        fs::write(source.join(path), b"opaque POI bytes, not inspected NBT").unwrap();
    }
    let original = fs::read(source.join(poi_path)).unwrap();
    let original_hashes = hashes(&source).unwrap();
    let config = CoreConfig::new(native.clone(), 42);
    let receipt = import_anvil(
        &config,
        &source,
        &native,
        &temp.path().join("import-receipt.json"),
    )
    .unwrap();
    assert_eq!(receipt.status, "PASS");
    assert_eq!(receipt.input_hashes, original_hashes);
    assert!(receipt.upgrades.iter().all(|upgrade| upgrade.kind != "poi"));
    let retained = &receipt
        .upgrades
        .iter()
        .find(|upgrade| upgrade.kind == "level")
        .unwrap()
        .preserved;
    for path in std::iter::once(poi_path).chain(opaque_paths) {
        assert!(
            retained
                .iter()
                .any(|entry| entry.path == path && !entry.reason.is_empty())
        );
    }
    let output = temp.path().join("exported");
    let export_receipt = export_anvil(
        &config,
        &native,
        &output,
        &temp.path().join("export-receipt.json"),
    )
    .unwrap();
    assert_eq!(export_receipt.status, "PASS");
    assert_eq!(fs::read(output.join(poi_path)).unwrap(), original);
    let exported = mc_world::anvil::read_region(output.join(poi_path)).unwrap();
    assert_eq!(exported.len(), 3);
    for (record, version) in exported.iter().zip([2586, 2860, 3700]) {
        let (_, tag) = mc_nbt::read_named(&mut record.uncompressed_nbt.as_slice()).unwrap();
        assert_eq!(tag, legacy_poi(version));
    }
    for path in opaque_paths {
        assert_eq!(
            fs::read(output.join(path)).unwrap(),
            fs::read(source.join(path)).unwrap(),
            "{path}: opaque file must survive without parsing"
        );
    }
    assert_eq!(fs::read(source.join(poi_path)).unwrap(), original);
    assert_eq!(hashes(&source).unwrap(), original_hashes);
}

#[test]
fn legacy_auxiliary_canonical_collisions_fail_without_mutating_source() {
    for (legacy, canonical) in [
        ("data/map_7.dat", "data/minecraft/maps/7.dat"),
        ("data/idcounts.dat", "data/minecraft/maps/last_id.dat"),
        (
            "data/raids.dat",
            "dimensions/minecraft/overworld/data/minecraft/raids.dat",
        ),
        (
            "data/world_border.dat",
            "dimensions/minecraft/overworld/data/minecraft/world_border.dat",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let native = temp.path().join("native");
        fs::create_dir(&source).unwrap();
        write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
        write_legacy_region(&source, "region", &legacy_chunk());
        let tag = legacy_saved_data(compound(vec![("mod:collision", Tag::Int(7))]));
        write_nbt(&source.join(legacy), &tag).unwrap();
        write_nbt(&source.join(canonical), &tag).unwrap();
        let original_hashes = hashes(&source).unwrap();
        let result = import_anvil(
            &CoreConfig::new(native.clone(), 42),
            &source,
            &native,
            &temp.path().join("import-receipt.json"),
        );
        assert!(result.is_err(), "{legacy} aliases {canonical}");
        assert_eq!(hashes(&source).unwrap(), original_hashes);
    }
}

#[test]
fn non_full_empty_import_regenerates_but_non_air_import_keeps_terrain() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    for (has_blocks, has_payload, fluid) in [
        (false, false, "minecraft:empty"),
        (true, false, "minecraft:empty"),
        (false, true, "minecraft:empty"),
        (true, false, "example:unknown_fluid"),
    ] {
        let retained = has_blocks || has_payload;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let native = temp.path().join("native");
        fs::create_dir(&source).unwrap();
        write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
        let mut chunk = mc_world::Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            mc_world::BlockStateId(registry.states.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        );
        chunk.status = "minecraft:empty".into();
        if has_blocks {
            chunk
                .set_block(0, 0, 0, mc_world::BlockStateId(registry.states.stone))
                .unwrap();
        }
        assert!(chunk.schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
            mc_world::BlockPos { x: 0, y: 0, z: 0 },
            mc_data::Identifier::parse(fluid).unwrap(),
            73,
            0,
        )));
        if has_blocks {
            assert!(chunk.schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
                mc_world::BlockPos { x: 1, y: 0, z: 0 },
                mc_data::Identifier::parse("minecraft:water").unwrap(),
                74,
                -2,
            )));
        }
        chunk
            .extras
            .push(("HeightMap".into(), Tag::IntArray(vec![64; 256])));
        chunk.extras.push(("InhabitedTime".into(), Tag::Long(50)));
        chunk
            .extras
            .push(("AuthorNote".into(), string("retained in original source")));
        chunk.extras.push((
            "structures".into(),
            compound(vec![
                ("starts", compound(Vec::new())),
                (
                    "References",
                    compound(vec![("minecraft:mineshaft", Tag::LongArray(vec![1]))]),
                ),
            ]),
        ));
        let mut tag = mc_world::anvil::chunk_to_nbt(&chunk, &registry.blocks).unwrap();
        if !has_blocks {
            let Tag::Compound(fields) = &mut tag else {
                unreachable!()
            };
            fields.retain(|(name, _)| !matches!(name.as_str(), "sections" | "yPos"));
        }
        if has_payload {
            let Tag::Compound(fields) = &mut tag else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(name, _)| name == "block_entities")
                .unwrap()
                .1 = list(
                tag_type::COMPOUND,
                vec![compound(vec![
                    ("id", string("minecraft:chest")),
                    ("x", Tag::Int(0)),
                    ("y", Tag::Int(0)),
                    ("z", Tag::Int(0)),
                    ("Items", list(tag_type::COMPOUND, Vec::new())),
                ])],
            );
        }
        write_legacy_region(&source, "region", &tag);
        let original = hashes(&source).unwrap();
        let config = CoreConfig::new(native.clone(), 42);
        let result = import_anvil(
            &config,
            &source,
            &native,
            &temp.path().join("import-receipt.json"),
        );
        if fluid == "example:unknown_fluid" {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("unknown scheduled fluid example:unknown_fluid")
            );
            assert_eq!(hashes(&source).unwrap(), original);
            assert!(!native.exists());
            continue;
        }
        let receipt = result.unwrap();
        assert_eq!(receipt.counts["dropped_noop_fluid_ticks"], 1);
        assert_eq!(
            receipt.counts["non_full_empty_chunks"],
            u64::from(!retained)
        );
        assert_eq!(
            receipt.counts["non_full_non_air_chunks"],
            u64::from(has_blocks)
        );
        assert_eq!(
            receipt.counts["non_full_air_with_payload_chunks"],
            u64::from(has_payload)
        );
        assert_eq!(receipt.counts["chunks"], u64::from(retained));
        assert_eq!(
            receipt.counts["regenerated_structure_losses"],
            u64::from(!retained)
        );
        if !retained {
            assert_eq!(receipt.counts["regenerated_chunk_tag.HeightMap"], 1);
            assert_eq!(receipt.known_gaps.len(), 3);
            assert_eq!(receipt.counts["preservation_loss.structures"], 1);
            assert_eq!(receipt.counts["preservation_loss.InhabitedTime"], 1);
            assert_eq!(receipt.counts["preservation_loss.AuthorNote"], 1);
            assert!(
                receipt.known_gaps[0].contains("PRESERVATION LOSS: region/r.0.0.mca#chunk(0, 0)")
            );
        }
        let store = std::sync::Arc::new(
            NativeStore::open(
                &native.join("native/state.redb"),
                bridge::native_identity(&config),
            )
            .unwrap(),
        );
        let loader_source = crate::chunk_loader::ChunkSource {
            native: Some(store),
            dimension: 0,
            generator: bridge::generator(
                &config,
                &registry,
                crate::dimensions::Dimension::Overworld,
            )
            .unwrap(),
            registry: std::sync::Arc::clone(&registry.blocks),
            items: std::sync::Arc::clone(&registry.items),
            chest_loot: std::sync::Arc::clone(&registry.chest_loot),
            light: std::sync::Arc::clone(&registry.light),
            geometry: mc_world::OVERWORLD_GEOMETRY,
        };
        let (loaded, generated) = loader_source.load_or_generate(chunk.pos).unwrap();
        assert_eq!(generated, !retained);
        if retained {
            let expected = if has_blocks {
                registry.states.stone
            } else {
                registry.states.air
            };
            assert_eq!(
                loaded.get_block(0, 0, 0),
                Some(mc_world::BlockStateId(expected))
            );
            assert_eq!(loaded.chests.len(), usize::from(has_payload));
            assert_eq!(loaded.scheduled_fluid_ticks.len(), usize::from(has_blocks));
            if has_blocks {
                assert_eq!(
                    loaded.scheduled_fluid_ticks[0].fluid.as_str(),
                    "minecraft:water"
                );
                assert_eq!(loaded.scheduled_fluid_ticks[0].priority, -2);
            }
            assert_eq!(loaded.status, "minecraft:empty");
            assert!(loaded.extras.iter().any(|(name, _)| name == "HeightMap"));
        } else {
            assert!(
                loaded.sections.iter().any(|section| !section.is_empty()),
                "runtime must generate terrain instead of admitting a permanent void"
            );
        }
        assert_eq!(hashes(&source).unwrap(), original);
    }
}

#[test]
fn foreign_scheduled_ticks_follow_vanilla_chunk_ownership() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    for (retained, unknown) in [
        (true, None),
        (false, None),
        (true, Some("block")),
        (true, Some("fluid")),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let native = temp.path().join("native");
        fs::create_dir_all(source.join("region")).unwrap();
        write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
        let empty = |pos| {
            mc_world::Chunk::empty(
                pos,
                mc_world::BlockStateId(registry.states.air),
                mc_data::Identifier::parse("minecraft:plains").unwrap(),
            )
        };
        let mut chunk = empty(ChunkPos { x: 0, z: 0 });
        chunk.status = if retained {
            "minecraft:full"
        } else {
            "minecraft:empty"
        }
        .into();
        if retained {
            chunk
                .set_block(0, 0, 0, mc_world::BlockStateId(registry.states.stone))
                .unwrap();
            chunk
                .scheduled_block_ticks
                .push(mc_world::ScheduledBlockTick::new(
                    BlockPos { x: 0, y: 0, z: 0 },
                    mc_data::Identifier::parse("minecraft:stone").unwrap(),
                    12,
                    -1,
                ));
            chunk
                .scheduled_fluid_ticks
                .push(mc_world::ScheduledFluidTick::new(
                    BlockPos { x: 1, y: 0, z: 0 },
                    mc_data::Identifier::parse("minecraft:water").unwrap(),
                    16,
                    1,
                ));
            let block_duplicate = chunk.scheduled_block_ticks[0].clone();
            let mut block_distinct = block_duplicate.clone();
            block_distinct.trigger_tick += 1;
            chunk.scheduled_block_ticks.push(block_distinct);
            chunk.scheduled_block_ticks.push(block_duplicate);
            let fluid_duplicate = chunk.scheduled_fluid_ticks[0].clone();
            let mut fluid_distinct = fluid_duplicate.clone();
            fluid_distinct.trigger_tick += 1;
            chunk.scheduled_fluid_ticks.push(fluid_distinct);
            chunk.scheduled_fluid_ticks.push(fluid_duplicate);
        }
        // Negative floor division, a present neighboring chunk, and an absent
        // farther chunk all use the same vanilla discard rule, never re-homing.
        for x in [-1, 16] {
            chunk
                .scheduled_block_ticks
                .push(mc_world::ScheduledBlockTick::new(
                    BlockPos { x, y: 7, z: 1 },
                    mc_data::Identifier::parse("minecraft:stone").unwrap(),
                    73,
                    -2,
                ));
            chunk
                .scheduled_fluid_ticks
                .push(mc_world::ScheduledFluidTick::new(
                    BlockPos { x, y: 7, z: 1 },
                    mc_data::Identifier::parse("minecraft:water").unwrap(),
                    74,
                    2,
                ));
        }
        for x in [3, 32] {
            chunk
                .scheduled_fluid_ticks
                .push(mc_world::ScheduledFluidTick::new(
                    BlockPos { x, y: 7, z: 1 },
                    mc_data::Identifier::parse("minecraft:empty").unwrap(),
                    75,
                    0,
                ));
        }
        if unknown == Some("block") {
            chunk
                .scheduled_block_ticks
                .push(mc_world::ScheduledBlockTick::new(
                    BlockPos { x: -2, y: 7, z: 1 },
                    mc_data::Identifier::parse("example:unknown_block").unwrap(),
                    76,
                    0,
                ));
        }
        if unknown == Some("fluid") {
            chunk
                .scheduled_fluid_ticks
                .push(mc_world::ScheduledFluidTick::new(
                    BlockPos { x: -2, y: 7, z: 1 },
                    mc_data::Identifier::parse("example:unknown_fluid").unwrap(),
                    76,
                    0,
                ));
        }
        let mut neighbor = empty(ChunkPos { x: 1, z: 0 });
        neighbor.status = "minecraft:full".into();
        let payloads: Vec<_> = [&chunk, &neighbor]
            .into_iter()
            .map(|column| {
                let tag = mc_world::anvil::chunk_to_nbt(column, &registry.blocks).unwrap();
                let mut bytes = Vec::new();
                mc_nbt::write_named(&mut bytes, "", &tag).unwrap();
                mc_world::anvil::ChunkPayload {
                    local_x: column.pos.x as u8,
                    local_z: 0,
                    timestamp: 0,
                    uncompressed_nbt: bytes,
                }
            })
            .collect();
        mc_world::anvil::write_region(source.join("region/r.0.0.mca"), &payloads).unwrap();
        let original = hashes(&source).unwrap();
        let config = CoreConfig::new(native.clone(), 42);
        let result = import_anvil(&config, &source, &native, &temp.path().join("receipt.json"));
        if let Some(kind) = unknown {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains(&format!("unknown scheduled {kind} example:unknown_{kind}"))
            );
            assert!(!native.exists());
            assert_eq!(hashes(&source).unwrap(), original);
            continue;
        }
        let receipt = result.unwrap();
        assert_eq!(receipt.counts["dropped_foreign_scheduled_ticks"], 5);
        assert_eq!(receipt.counts["dropped_noop_fluid_ticks"], 1);
        assert_eq!(
            receipt.counts["deduplicated_scheduled_ticks"],
            2 * u64::from(retained)
        );
        assert_eq!(receipt.counts["chunks"], 1 + u64::from(retained));
        assert_eq!(
            receipt.counts["non_full_empty_chunks"],
            u64::from(!retained)
        );
        assert!(
            receipt
                .known_gaps
                .iter()
                .all(|gap| !gap.contains("scheduled tick"))
        );
        let store = NativeStore::open(
            &native.join("native/state.redb"),
            bridge::native_identity(&config),
        )
        .unwrap();
        let checkpoint = store.read_checkpoint().unwrap();
        let manifest: CheckpointManifest =
            serde_json::from_slice(&checkpoint.globals["checkpoint"].payload).unwrap();
        manifest.block_ticks.validate().unwrap();
        manifest.fluid_ticks.validate().unwrap();
        assert_eq!(
            manifest
                .block_ticks
                .chunks
                .iter()
                .map(|entry| entry.ticks.len())
                .sum::<usize>(),
            2 * usize::from(retained)
        );
        assert_eq!(manifest.fluid_ticks.due.len(), 2 * usize::from(retained));
        for pos in [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }] {
            let loaded = store
                .resolve_base(
                    0,
                    pos,
                    &registry.blocks,
                    &registry.items,
                    &registry.chest_loot,
                    || {
                        assert!(
                            !retained && pos.x == 0,
                            "imported readback requested procedural fallback"
                        );
                        Ok(None)
                    },
                )
                .unwrap();
            if !retained && pos.x == 0 {
                assert!(loaded.is_none());
                continue;
            }
            let loaded = loaded.unwrap();
            let owned = 2 * usize::from(retained && pos.x == 0);
            assert_eq!(loaded.scheduled_block_ticks.len(), owned);
            assert_eq!(loaded.scheduled_fluid_ticks.len(), owned);
            if owned != 0 {
                assert_eq!(loaded.scheduled_block_ticks[0].trigger_tick, 91);
                assert_eq!(loaded.scheduled_block_ticks[0].priority, -1);
                assert_eq!(loaded.scheduled_fluid_ticks[0].trigger_tick, 95);
                assert_eq!(loaded.scheduled_fluid_ticks[0].priority, 1);
                assert_eq!(loaded.scheduled_block_ticks[1].trigger_tick, 92);
                assert_eq!(loaded.scheduled_fluid_ticks[1].trigger_tick, 96);
            }
        }
        assert_eq!(hashes(&source).unwrap(), original);
    }
}

#[test]
fn foreign_scheduled_ticks_staged_import_crash_before_final_commit_is_unpublished() {
    const CHILD_ROOT: &str = "SOLARIS_TEST_STAGED_IMPORT_CRASH_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let destination = root.join("native");
        let config = CoreConfig::new(destination.clone(), 42);
        let _ = super::convert(
            "import-anvil",
            &config,
            &root.join("source"),
            &destination,
            &root.join("crash-receipt.json"),
            |config, _, stage| {
                let registry = crate::registry::CoreRegistry::load(None).unwrap();
                fs::create_dir_all(stage.join("native")).unwrap();
                let store = NativeStore::create(
                    &stage.join("native/state.redb"),
                    bridge::native_identity(config),
                )
                .unwrap();
                let chunk = mc_world::Chunk::empty(
                    ChunkPos { x: 0, z: 0 },
                    mc_world::BlockStateId(registry.states.air),
                    mc_data::Identifier::parse("minecraft:plains").unwrap(),
                );
                store
                    .import_staged_chunk(0, &chunk, &registry.blocks, &registry.items)
                    .unwrap();
                fs::write(
                    stage.parent().unwrap().join("before-final-commit"),
                    b"staged",
                )
                .unwrap();
                // Deliberately skip Rust drops and the final Immediate checkpoint.
                // The outer converter has not reached its only rename/publication.
                std::process::exit(86);
            },
        );
        panic!("crash adapter unexpectedly returned");
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    write_legacy_region(&source, "region", &legacy_chunk());
    let original = hashes(&source).unwrap();
    let full_name = concat!(
        module_path!(),
        "::foreign_scheduled_ticks_staged_import_crash_before_final_commit_is_unpublished"
    );
    let test_name = full_name.split_once("::").unwrap().1;
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD_ROOT, temp.path())
        .output()
        .unwrap();
    assert_eq!(child.status.code(), Some(86), "{child:?}");
    assert!(temp.path().join("before-final-commit").exists());
    assert!(!temp.path().join("native").exists());
    assert!(!temp.path().join("crash-receipt.json").exists());
    let stage = fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".solaris-conversion-")
        })
        .expect("abrupt exit must leave only unpublished staging");
    let staged_bytes = hashes(&stage).unwrap();
    match crate::Core::open(CoreConfig::new(stage.clone(), 42)) {
        Ok(core) => {
            core.shutdown();
            panic!("crashed unpublished staging admitted");
        }
        Err(error) => assert!(error.to_string().contains("staging"), "{error}"),
    }
    assert_eq!(hashes(&stage).unwrap(), staged_bytes);
    assert_eq!(hashes(&source).unwrap(), original);
    let native = temp.path().join("normal");
    let config = CoreConfig::new(native.clone(), 42);
    let receipt = import_anvil(
        &config,
        &source,
        &native,
        &temp.path().join("normal-receipt.json"),
    )
    .unwrap();
    assert_eq!(receipt.status, "PASS");
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let store = NativeStore::open(
        &native.join("native/state.redb"),
        bridge::native_identity(&config),
    )
    .unwrap();
    let loaded = store
        .resolve_base(
            0,
            ChunkPos { x: 0, z: 0 },
            &registry.blocks,
            &registry.items,
            &registry.chest_loot,
            || panic!("normal imported baseline missing"),
        )
        .unwrap()
        .unwrap();
    let control = NativeStore::create(
        &temp.path().join("immediate-control.redb"),
        bridge::native_identity(&config),
    )
    .unwrap();
    control
        .import_chunk(0, &loaded, &registry.blocks, &registry.items)
        .unwrap();
    let immediate = control
        .resolve_base(
            0,
            loaded.pos,
            &registry.blocks,
            &registry.items,
            &registry.chest_loot,
            || panic!("Immediate control baseline missing"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        mc_world::anvil::chunk_to_nbt(&loaded, &registry.blocks).unwrap(),
        mc_world::anvil::chunk_to_nbt(&immediate, &registry.blocks).unwrap()
    );
    assert_eq!(hashes(&source).unwrap(), original);
}

#[test]
fn saved_maps_unversioned_index_preserves_allocation_fence_without_source_mutation() {
    // Reconstruct the real 26.1.2 hop's twelve raw NBT bytes without embedding
    // third-party map files: a nameless compound containing Short("map", 448).
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &compound(vec![("map", Tag::Short(448))])).unwrap();
    assert_eq!(bytes.len(), 12);
    for source_path in ["data/idcounts.dat", "data/minecraft/maps/last_id.dat"] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let native = temp.path().join("native");
        let output = temp.path().join("export");
        fs::create_dir(&source).unwrap();
        write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
        write_legacy_region(&source, "region", &legacy_chunk());
        let counter = source.join(source_path);
        fs::create_dir_all(counter.parent().unwrap()).unwrap();
        fs::write(&counter, &bytes).unwrap();
        let original = hashes(&source).unwrap();
        let config = CoreConfig::new(native.clone(), 42);
        let imported = import_anvil(
            &config,
            &source,
            &native,
            &temp.path().join("import-receipt.json"),
        )
        .unwrap();
        assert_eq!(imported.status, "PASS");
        let exported = export_anvil(
            &config,
            &native,
            &output,
            &temp.path().join("export-receipt.json"),
        )
        .unwrap();
        assert_eq!(exported.status, "PASS");
        let counter = read_nbt(&output.join("data/minecraft/maps/last_id.dat")).unwrap();
        assert_eq!(crate::maps::nbt::sequence_from_tag(&counter).unwrap(), 449);
        assert_eq!(hashes(&source).unwrap(), original);
    }
}

#[test]
fn legacy_saved_data_versions_outside_supported_range_fail_closed() {
    // Maps accept any older stamp (their upgrade chain floors at -1), so
    // only above-target fails; 2585 is a legitimate map source now.
    let version = 4791;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    write_legacy_region(&source, "region", &legacy_chunk());
    write_nbt(
        &source.join("data/map_7.dat"),
        &compound(vec![
            ("DataVersion", Tag::Int(version)),
            ("data", compound(vec![])),
        ]),
    )
    .unwrap();
    let original_hashes = hashes(&source).unwrap();
    let result = import_anvil(
        &CoreConfig::new(native.clone(), 42),
        &source,
        &native,
        &temp.path().join("import-receipt.json"),
    );
    assert!(result.is_err(), "unsupported saved-data version {version}");
    assert_eq!(hashes(&source).unwrap(), original_hashes);
}

#[test]
fn legacy_auxiliary_collisions_with_level_split_or_other_poi_fail_closed() {
    for poi in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let native = temp.path().join("native");
        fs::create_dir(&source).unwrap();
        write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
        write_legacy_region(&source, "region", &legacy_chunk());
        if poi {
            write_legacy_region(&source, "poi", &legacy_poi(2860));
            write_legacy_region(
                &source,
                "dimensions/minecraft/overworld/poi",
                &legacy_poi(3700),
            );
        } else {
            // A single auxiliary file conflicts with the border produced
            // from legacy level.dat; neither source may silently win.
            write_nbt(
                &source.join("data/world_border.dat"),
                &legacy_saved_data(compound(vec![("BorderSize", Tag::Double(1000.0))])),
            )
            .unwrap();
        }
        let original_hashes = hashes(&source).unwrap();
        let result = import_anvil(
            &CoreConfig::new(native.clone(), 42),
            &source,
            &native,
            &temp.path().join("import-receipt.json"),
        );
        assert!(result.is_err(), "auxiliary collision, POI={poi}");
        assert_eq!(hashes(&source).unwrap(), original_hashes);
    }
}

#[test]
fn inline_region_entities_decode_into_entity_snapshots_without_entities_dir() {
    // 1.16.5-era layout: no entities/ dir; the upgraded region chunk's
    // inline `entities` list must land in modern entity snapshots, not
    // just linger on the chunk.
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    let mut chunk = legacy_chunk();
    if let Tag::Compound(root) = &mut chunk {
        let entity = elements(field(&legacy_entities(), "Entities"))[0].clone();
        root.push(("entities".into(), list(tag_type::COMPOUND, vec![entity])));
    } else {
        unreachable!()
    }
    write_legacy_region(&source, "region", &chunk);
    assert!(!source.join("entities").exists());
    let config = CoreConfig::new(native.clone(), 42);
    let receipt =
        import_anvil(&config, &source, &native, &temp.path().join("receipt.json")).unwrap();
    assert_eq!(receipt.status, "PASS");
    assert_eq!(receipt.counts["entities"], 1);
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let store = NativeStore::open(
        &native.join("native/state.redb"),
        bridge::native_identity(&config),
    )
    .unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    assert_eq!(checkpoint.entities.len(), 1);
    let entity_tag = checkpoint.entities[0].payload.as_ref().unwrap();
    let entity = entity_from_fields(
        &registry.items,
        VanillaEntity::default(),
        fields(entity_tag),
    )
    .unwrap();
    assert_eq!(entity.id, "minecraft:item");
    assert_eq!(entity.uuid, uuid::Uuid::from_u128(7));
    let stored = store
        .resolve_base(
            0,
            ChunkPos { x: 0, z: 0 },
            &registry.blocks,
            &registry.items,
            &registry.chest_loot,
            || panic!("imported terrain must not fall back to procedural generation"),
        )
        .unwrap()
        .unwrap();
    assert!(
        stored.extras.iter().all(|(name, _)| name != "entities"),
        "inline entities lingered on chunk extras"
    );
}

#[test]
fn region_and_entities_dir_conflict_fails_closed() {
    // Storage-domain conflicts are not duplicate-UUID admission: even disjoint
    // identities cannot silently merge different bodies for the same chunk.
    for conflicting_field in ["UUID", "Age"] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let native = temp.path().join("native");
        fs::create_dir(&source).unwrap();
        write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
        let mut chunk = legacy_chunk();
        if let Tag::Compound(root) = &mut chunk {
            let entity = elements(field(&legacy_entities(), "Entities"))[0].clone();
            root.push(("entities".into(), list(tag_type::COMPOUND, vec![entity])));
        } else {
            unreachable!()
        }
        write_legacy_region(&source, "region", &chunk);
        let mut other = legacy_entities();
        if let Tag::Compound(root) = &mut other {
            let entry = root.iter_mut().find(|(key, _)| key == "Entities").unwrap();
            if let Tag::List(list) = &mut entry.1 {
                if let Tag::Compound(entity) = &mut list.elements[0] {
                    let value = entity
                        .iter_mut()
                        .find(|(key, _)| key == conflicting_field)
                        .unwrap();
                    value.1 = if conflicting_field == "UUID" {
                        uuid_tag(8)
                    } else {
                        Tag::Short(13)
                    };
                } else {
                    unreachable!()
                }
            } else {
                unreachable!()
            }
        } else {
            unreachable!()
        }
        write_legacy_region(&source, "entities", &other);
        let original = hashes(&source).unwrap();
        let result = import_anvil(
            &CoreConfig::new(native.clone(), 42),
            &source,
            &native,
            &temp.path().join("receipt.json"),
        );
        let error = result
            .expect_err("conflicting entity sources must fail")
            .to_string();
        assert!(
            error.contains("conflicting inline region and entities/ records"),
            "{conflicting_field}: {error}"
        );
        assert!(!native.exists(), "conflicting sources must not publish");
        assert_eq!(hashes(&source).unwrap(), original);
    }
}

#[test]
fn region_and_entities_dir_identical_bodies_are_admitted_once() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    let entities = legacy_entities();
    let mut chunk = legacy_chunk();
    if let Tag::Compound(root) = &mut chunk {
        root.push(("entities".into(), field(&entities, "Entities").clone()));
    } else {
        unreachable!()
    }
    write_legacy_region(&source, "region", &chunk);
    write_legacy_region(&source, "entities", &entities);
    let original = hashes(&source).unwrap();
    let config = CoreConfig::new(native.clone(), 42);
    let receipt =
        import_anvil(&config, &source, &native, &temp.path().join("receipt.json")).unwrap();
    assert_eq!(receipt.status, "PASS");
    assert_eq!(receipt.counts["entities"], 1);
    let store = NativeStore::open(
        &native.join("native/state.redb"),
        bridge::native_identity(&config),
    )
    .unwrap();
    assert_eq!(store.read_checkpoint().unwrap().entities.len(), 1);
    assert_eq!(hashes(&source).unwrap(), original);
}

fn uuid_tag(value: u128) -> Tag {
    Tag::IntArray(vec![
        (value >> 96) as i32,
        (value >> 64) as i32,
        (value >> 32) as i32,
        value as i32,
    ])
}

fn uuid_value(tag: &Tag) -> u128 {
    let Tag::IntArray(words) = tag else {
        panic!("expected UUID int array: {tag:?}")
    };
    words
        .iter()
        .fold(0_u128, |bits, word| (bits << 32) | u128::from(*word as u32))
}

/// Source-form (DataVersion 2860) item entity with caller-chosen identity,
/// position and body marker; every other field is the `legacy_entities` item.
fn duplicate_fixture_entity(uuid: u128, pos: [f64; 3], marker: &str, age: i16) -> Tag {
    let mut entity = elements(field(&legacy_entities(), "Entities"))[0].clone();
    let Tag::Compound(fields) = &mut entity else {
        unreachable!()
    };
    for (name, value) in fields.iter_mut() {
        match name.as_str() {
            "UUID" => *value = uuid_tag(uuid),
            "Pos" => {
                *value = list(
                    tag_type::DOUBLE,
                    pos.iter().map(|axis| Tag::Double(*axis)).collect(),
                );
            }
            "Age" => *value = Tag::Short(age),
            "mod:entity" => *value = string(marker),
            _ => {}
        }
    }
    entity
}

/// Write one `r.0.0.mca` entity region; each `(x, bodies)` is chunk `(x, 0)`.
fn write_entity_chunks(root: &Path, directory: &str, chunks: &[(i32, Vec<Tag>)]) {
    fs::create_dir_all(root.join(directory)).unwrap();
    let payloads: Vec<_> = chunks
        .iter()
        .map(|(x, bodies)| {
            let tag = compound(vec![
                ("DataVersion", Tag::Int(2860)),
                ("Position", Tag::IntArray(vec![*x, 0])),
                ("Entities", list(tag_type::COMPOUND, bodies.clone())),
            ]);
            let mut bytes = Vec::new();
            mc_nbt::write_named(&mut bytes, "", &tag).unwrap();
            mc_world::anvil::ChunkPayload {
                local_x: *x as u8,
                local_z: 0,
                timestamp: 0,
                uncompressed_nbt: bytes,
            }
        })
        .collect();
    mc_world::anvil::write_region(root.join(directory).join("r.0.0.mca"), &payloads).unwrap();
}

#[test]
fn duplicate_entity_uuids_keep_first_and_archive_original_bodies() {
    // Deterministic first-wins across inline region entities, an entities/
    // region and a second dimension: every later body sharing a UUID is
    // dropped (byte-identical ones included), the keeper is never reminted
    // or rewritten, and the original source NBT is archived beside the receipt.
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    let keeper_7 = duplicate_fixture_entity(7, [1.0, 65.0, 1.0], "keeper-7", 12);
    let keeper_8 = duplicate_fixture_entity(8, [2.0, 65.0, 1.0], "keeper-8", 1);
    let mut unique_9 = duplicate_fixture_entity(9, [3.0, 65.0, 1.0], "unique-9", 5);
    let Tag::Compound(unique_fields) = &mut unique_9 else {
        unreachable!()
    };
    // References to duplicated UUIDs must keep pointing at the keepers.
    unique_fields.push(("mod:owner".into(), uuid_tag(7)));
    unique_fields.push(("mod:target".into(), uuid_tag(8)));
    let identical_7 = keeper_7.clone();
    let distinct_8 = duplicate_fixture_entity(8, [4.0, 65.0, 1.0], "dropped-8-distinct", 99);
    let cross_chunk_7 = duplicate_fixture_entity(7, [17.0, 65.0, 1.0], "dropped-7-cross-chunk", 77);
    // A later duplicate of unique-9 whose Item id is unsupported: it must be
    // archived and dropped before typed entity decoding, not stop the import.
    let mut malformed_9 = duplicate_fixture_entity(9, [19.0, 65.0, 1.0], "dropped-9-malformed", 3);
    let Tag::Compound(malformed_fields) = &mut malformed_9 else {
        unreachable!()
    };
    malformed_fields
        .iter_mut()
        .find(|(name, _)| name == "Item")
        .unwrap()
        .1 = legacy_item("minecraft:solaris_not_an_item", 1, None);
    let nether_identical_8 = keeper_8.clone();
    let nether_distinct_7 = duplicate_fixture_entity(7, [5.0, 65.0, 5.0], "dropped-7-nether", 55);

    // Overworld chunk (0, 0): inline in the region chunk.
    let mut chunk = legacy_chunk();
    let Tag::Compound(chunk_fields) = &mut chunk else {
        unreachable!()
    };
    chunk_fields.push((
        "entities".into(),
        list(
            tag_type::COMPOUND,
            vec![
                keeper_7.clone(),
                keeper_8.clone(),
                identical_7.clone(),
                unique_9.clone(),
                distinct_8.clone(),
            ],
        ),
    ));
    write_legacy_region(&source, "region", &chunk);
    // Overworld chunk (1, 0): external entities/ region.
    write_entity_chunks(
        &source,
        "entities",
        &[(
            1,
            vec![
                cross_chunk_7.clone(),
                duplicate_fixture_entity(10, [18.0, 65.0, 1.0], "unique-10", 6),
                malformed_9.clone(),
            ],
        )],
    );
    // Nether chunk (0, 0).
    write_entity_chunks(
        &source,
        "DIM-1/entities",
        &[(
            0,
            vec![
                nether_identical_8.clone(),
                nether_distinct_7.clone(),
                duplicate_fixture_entity(11, [6.0, 65.0, 6.0], "unique-11", 7),
            ],
        )],
    );
    let original_hashes = hashes(&source).unwrap();

    // Deterministic drop order: dimension, chunk, original list index.
    let expected_drops = [
        ("region/r.0.0.mca", [0, 0], 2, 7_u128, &identical_7),
        ("region/r.0.0.mca", [0, 0], 4, 8, &distinct_8),
        ("entities/r.0.0.mca", [1, 0], 0, 7, &cross_chunk_7),
        ("entities/r.0.0.mca", [1, 0], 2, 9, &malformed_9),
        (
            "DIM-1/entities/r.0.0.mca",
            [0, 0],
            0,
            8,
            &nether_identical_8,
        ),
        ("DIM-1/entities/r.0.0.mca", [0, 0], 1, 7, &nether_distinct_7),
    ];
    let expected_survivors: std::collections::BTreeSet<(u32, i32, i32, u128, String)> = [
        (0, 0, 0, 7, "keeper-7"),
        (0, 0, 0, 8, "keeper-8"),
        (0, 0, 0, 9, "unique-9"),
        (0, 1, 0, 10, "unique-10"),
        (1, 0, 0, 11, "unique-11"),
    ]
    .into_iter()
    .map(|(dimension, x, z, uuid, marker)| (dimension, x, z, uuid, marker.to_string()))
    .collect();
    let marker = |tag: &Tag| match field(tag, "mod:entity") {
        Tag::String(marker) => marker.clone(),
        other => panic!("mod:entity is not a string: {other:?}"),
    };
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let canonical_temp = fs::canonicalize(temp.path()).unwrap();

    let mut runs = Vec::new();
    for run in ["a", "b"] {
        let native = temp.path().join(format!("native-{run}"));
        let receipt_name = format!("receipt-{run}.json");
        let config = CoreConfig::new(native.clone(), 42);
        let receipt =
            import_anvil(&config, &source, &native, &temp.path().join(&receipt_name)).unwrap();
        assert_eq!(receipt.status, "PASS", "run {run}");
        assert_eq!(receipt.input_hashes, original_hashes, "run {run}");
        assert_eq!(hashes(&source).unwrap(), original_hashes, "run {run}");
        assert_eq!(receipt.counts["duplicate_uuid_dropped"], 6, "run {run}");
        assert_eq!(receipt.counts["entities"], 5, "run {run}");
        // Entity UUIDs are never reminted; `reminted_ids` stays the WireId counter.
        assert!(!receipt.counts.contains_key("reminted_entity_uuids"));

        let archive = canonical_temp.join(format!("{receipt_name}.dropped-entity-nbt"));
        let mut archived_names: Vec<String> = fs::read_dir(&archive)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        archived_names.sort();
        let mut expected_names: Vec<String> = expected_drops
            .iter()
            .enumerate()
            .map(|(ordinal, (_, _, _, uuid, _))| {
                format!("{ordinal}-{}.nbt", uuid::Uuid::from_u128(*uuid))
            })
            .collect();
        expected_names.sort();
        assert_eq!(archived_names, expected_names, "run {run}");
        let losses: Vec<&String> = receipt
            .known_gaps
            .iter()
            .filter(|gap| gap.contains("duplicate UUID body dropped"))
            .collect();
        assert_eq!(losses.len(), 6, "run {run}: {:?}", receipt.known_gaps);
        for (ordinal, (path, chunk, index, uuid, original)) in expected_drops.iter().enumerate() {
            let uuid = uuid::Uuid::from_u128(*uuid);
            let artifact = archive.join(format!("{ordinal}-{uuid}.nbt"));
            assert_eq!(
                &read_nbt(&artifact).unwrap(),
                *original,
                "run {run}: archived body {ordinal} must be the original source NBT"
            );
            let expected_gap = format!(
                "PRESERVATION LOSS: {path}#chunk({}, {}) entity-index {index} UUID {uuid}: duplicate UUID body dropped; deterministic first body kept, references unchanged; original NBT: {}",
                chunk[0],
                chunk[1],
                artifact.display()
            );
            assert!(
                losses.contains(&&expected_gap),
                "run {run}: missing {expected_gap}\nin {losses:#?}"
            );
        }

        let store = NativeStore::open(
            &native.join("native/state.redb"),
            bridge::native_identity(&config),
        )
        .unwrap();
        let checkpoint = store.read_checkpoint().unwrap();
        drop(store);
        let mut survivors = std::collections::BTreeSet::new();
        for snapshot in &checkpoint.entities {
            let payload = snapshot.payload.as_ref().unwrap();
            let uuid = u128::from_be_bytes(snapshot.identity);
            assert_eq!(uuid_value(field(payload, "UUID")), uuid, "no UUID remint");
            let entity =
                entity_from_fields(&registry.items, VanillaEntity::default(), fields(payload))
                    .unwrap();
            assert_eq!(entity.uuid.as_u128(), uuid);
            if marker(payload) == "unique-9" {
                assert_eq!(field(payload, "mod:owner"), &uuid_tag(7));
                assert_eq!(field(payload, "mod:target"), &uuid_tag(8));
            }
            assert!(
                survivors.insert((
                    snapshot.dimension,
                    snapshot.position.x,
                    snapshot.position.z,
                    uuid,
                    marker(payload),
                )),
                "duplicate survivor in native checkpoint"
            );
        }
        assert_eq!(survivors, expected_survivors, "run {run}");
        let unique_uuids: std::collections::BTreeSet<_> =
            survivors.iter().map(|survivor| survivor.3).collect();
        assert_eq!(unique_uuids.len(), survivors.len());

        if run == "a" {
            let output = temp.path().join("exported");
            let export = export_anvil(
                &config,
                &native,
                &output,
                &temp.path().join("export-receipt.json"),
            )
            .unwrap();
            assert_eq!(export.status, "PASS");
            let mut exported = std::collections::BTreeSet::new();
            for (dimension, directory) in [(0_u32, "overworld"), (1, "the_nether")] {
                let path = output
                    .join("dimensions/minecraft")
                    .join(directory)
                    .join("entities/r.0.0.mca");
                for record in mc_world::anvil::read_region(&path).unwrap() {
                    let (_, tag) =
                        mc_nbt::read_named(&mut record.uncompressed_nbt.as_slice()).unwrap();
                    let Tag::IntArray(position) = field(&tag, "Position") else {
                        panic!("Position is not an int array")
                    };
                    for entity in elements(field(&tag, "Entities")) {
                        assert!(exported.insert((
                            dimension,
                            position[0],
                            position[1],
                            uuid_value(field(entity, "UUID")),
                            marker(entity),
                        )));
                    }
                }
            }
            assert_eq!(exported, expected_survivors);
        }
        runs.push((survivors, receipt.counts["duplicate_uuid_dropped"]));
    }
    // The same source keeps the same first bodies on every import.
    assert_eq!(runs[0], runs[1]);
    assert_eq!(hashes(&source).unwrap(), original_hashes);
}

#[test]
fn versionless_player_with_opaque_modern_data_field_still_runs_legacy_chain() {
    // A versionless player carrying an opaque `Data` payload whose own
    // DataVersion is modern must still report from=-1 (root-only read):
    // the opaque field is not the record's declared version.
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let native = temp.path().join("native");
    fs::create_dir(&source).unwrap();
    write_nbt(&source.join("level.dat"), &legacy_level()).unwrap();
    write_legacy_region(&source, "region", &legacy_chunk());
    let mut player = legacy_player();
    let Tag::Compound(fields) = &mut player else {
        unreachable!()
    };
    fields.retain(|(key, _)| key != "DataVersion");
    fields.push((
        "Data".into(),
        compound(vec![("DataVersion", Tag::Int(4790))]),
    ));
    write_nbt(
        &source.join("playerdata/00000000-0000-0000-0000-000000000003.dat"),
        &player,
    )
    .unwrap();
    let receipt = import_anvil(
        &CoreConfig::new(native.clone(), 42),
        &source,
        &native,
        &temp.path().join("receipt.json"),
    )
    .unwrap();
    assert_eq!(receipt.status, "PASS");
    let upgrade = receipt
        .upgrades
        .iter()
        .find(|entry| entry.record == "playerdata/00000000-0000-0000-0000-000000000003.dat")
        .unwrap();
    assert_eq!(upgrade.from, -1);
    assert_eq!(upgrade.to, 4790);
    let config = CoreConfig::new(native.clone(), 42);
    let store = NativeStore::open(
        &native.join("native/state.redb"),
        bridge::native_identity(&config),
    )
    .unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    assert_eq!(checkpoint.players.len(), 1);
    // The opaque `Data` remainder is preserved verbatim through the
    // upgrade and native custody: no fixer consumes it, and the inner
    // version is data, not the record's source.
    let player_tag = bridge::decode_tag(&checkpoint.players[0].record, "player").unwrap();
    assert_eq!(field(&player_tag, "DataVersion"), &Tag::Int(4790));
    assert_eq!(
        field(field(&player_tag, "Data"), "DataVersion"),
        &Tag::Int(4790)
    );
    // Export round-trips the remainder unchanged into players/data/<uuid>.dat.
    drop(checkpoint);
    drop(store);
    let output = temp.path().join("exported");
    export_anvil(&config, &native, &output, &temp.path().join("export.json")).unwrap();
    let exported =
        read_nbt(&output.join("players/data/00000000-0000-0000-0000-000000000003.dat")).unwrap();
    assert_eq!(field(&exported, "DataVersion"), &Tag::Int(4790));
    assert_eq!(
        field(field(&exported, "Data"), "DataVersion"),
        &Tag::Int(4790)
    );
}
