use super::*;

#[test]
fn destination_exists_refuses_and_writes_failure_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let destination = dir.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("owner-data"), b"unchanged").unwrap();
    let receipt = dir.path().join("receipt.json");
    let result = import_anvil(
        &CoreConfig::new(source.clone(), 0),
        &source,
        &destination,
        &receipt,
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("destination already exists")
    );
    let receipt: ConversionReceipt = serde_json::from_slice(&fs::read(receipt).unwrap()).unwrap();
    assert_eq!(receipt.status, "FAIL");
    assert!(receipt.checkpoint_tick.is_none());
    assert_eq!(
        fs::read(destination.join("owner-data")).unwrap(),
        b"unchanged"
    );
}

#[test]
fn receipt_inside_source_is_refused_even_when_destination_exists() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let destination = dir.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    let receipt = source.join("receipt.json");
    assert!(
        import_anvil(
            &CoreConfig::new(source.clone(), 0),
            &source,
            &destination,
            &receipt
        )
        .is_err()
    );
    assert!(
        !receipt.exists(),
        "a failure receipt must never modify the source"
    );
}

#[test]
fn supported_record_versions_and_current_typed_reader_versions_are_distinct() {
    let record = |version| Tag::Compound(vec![("DataVersion".into(), Tag::Int(version))]);
    let level = |data| Tag::Compound(vec![("Data".into(), data)]);
    for version in [2230, 2586, 2860, 3700, 4790] {
        assert_eq!(
            source_version(&record(version), Path::new("region/r.0.0.mca")).unwrap(),
            version
        );
        assert_eq!(
            source_version(&level(record(version)), Path::new("level.dat")).unwrap(),
            version
        );
        assert_eq!(
            source_version(
                &Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(version)),
                    ("Data".into(), record(version)),
                ]),
                Path::new("level.dat"),
            )
            .unwrap(),
            version
        );
    }
    let invalid = [
        record(2229),
        record(4791),
        Tag::Compound(vec![]),
        Tag::Compound(vec![("DataVersion".into(), Tag::Long(2860))]),
        Tag::Compound(vec![("DataVersion".into(), Tag::String("2860".into()))]),
        Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(2860)),
            ("DataVersion".into(), Tag::Int(2860)),
        ]),
        level(Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(2860)),
            ("DataVersion".into(), Tag::Int(4790)),
        ])),
        Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(2860)),
            ("Data".into(), record(4790)),
        ]),
        Tag::Compound(vec![
            ("Data".into(), record(2860)),
            ("Data".into(), record(2860)),
        ]),
        level(Tag::Int(2860)),
        Tag::Int(2860),
    ];
    for tag in invalid {
        let error = source_version(&tag, Path::new("level.dat")).unwrap_err();
        assert!(error.to_string().contains("level.dat"), "{tag:?}: {error}");
    }
    // A chunk's opaque Data payload is not a level.dat version wrapper.
    assert_eq!(
        source_version(
            &Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(2860)),
                ("Data".into(), record(4790)),
            ]),
            Path::new("region/r.0.0.mca"),
        )
        .unwrap(),
        2860
    );
    for path in [
        Path::new("data/minecraft/scoreboard.dat"),
        Path::new("data/minecraft/world_gen_settings.dat"),
    ] {
        for version in [2860, 3700, 4772, 2585, 4791] {
            let error = check_version(&record(version), path).unwrap_err();
            assert!(error.to_string().contains(&path.display().to_string()));
            assert!(!error.to_string().contains("upgrade not yet available"));
        }
        assert!(check_version(&record(4790), path).is_ok());
        assert!(check_version(&Tag::Compound(vec![]), path).is_err());
    }
}

#[test]
fn dfu_neutral_auxiliary_files_accept_their_stored_4772_stamp() {
    // Genuine vanilla 26.1.2 `--forceUpgrade` output keeps these files at the
    // DataVersion they were written with: DFU runs on load but no fixer
    // between 4772 and 4790 rewrites their payloads, and vanilla does not
    // rewrite unmodified files on save. Evidence:
    // .analysis/codex-logs/constantinople/aux-dataversion-javap.txt and the
    // byte-identical hop input/output in .analysis/maps/falcons-rock/.
    let clocks = |version| {
        Tag::Compound(vec![
            (
                "data".into(),
                Tag::Compound(vec![(
                    "minecraft:overworld".into(),
                    Tag::Compound(vec![
                        ("total_ticks".into(), Tag::Long(8088)),
                        ("paused".into(), Tag::Byte(0)),
                    ]),
                )]),
            ),
            ("DataVersion".into(), Tag::Int(version)),
        ])
    };
    let record = |version| Tag::Compound(vec![("DataVersion".into(), Tag::Int(version))]);
    for file in [
        "world_clocks.dat",
        "wandering_trader.dat",
        "scheduled_events.dat",
        "custom_boss_events.dat",
    ] {
        let path = PathBuf::from(format!("data/minecraft/{file}"));
        let tag = if file == "world_clocks.dat" {
            clocks(4772)
        } else {
            record(4772)
        };
        assert!(
            check_version(&tag, &path).is_ok(),
            "{file} at stored 4772 must import exactly as vanilla decodes it"
        );
        assert!(check_version(&record(4790), &path).is_ok());
        assert!(check_version(&record(4791), &path).is_err());
        // A payload fixer actually rewrites (e.g. game rules at 4658) must
        // stay fail-closed at an older stamp.
        if file != "world_clocks.dat" {
            assert!(check_version(&record(3700), &path).is_err());
        }
    }
    // Files whose payload a fixer rewrote after 4772 stay exact-match.
    for file in [
        "game_rules.dat",
        "weather.dat",
        "world_gen_settings.dat",
        "scoreboard.dat",
    ] {
        let path = PathBuf::from(format!("data/minecraft/{file}"));
        assert!(check_version(&record(4772), &path).is_err(), "{file}");
        assert!(check_version(&record(4790), &path).is_ok());
    }
}
#[test]
fn raids_2586_pascalcase_upgrades_to_codec_fields() {
    // Genuine 1.16.5 output (Falcons Rock, byte-identical through the
    // official 26.1.2 hop): `{data:{Raids:[],NextAvailableID:1,Tick:...},
    // DataVersion:2586}`. Vanilla `RaidRenamesDataFix4309` renames the
    // PascalCase keys; `SavedDataUUIDFix2514` is vacuous on an empty list.
    use super::import::upgrade_raids_payload_for_tests as upgrade;
    let mut data = vec![
        (
            "Raids".into(),
            Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![
                    ("CX".into(), Tag::Int(10)),
                    ("CY".into(), Tag::Int(64)),
                    ("CZ".into(), Tag::Int(-30)),
                    ("Id".into(), Tag::Int(1)),
                    ("TicksActive".into(), Tag::Long(99)),
                    ("TotalHealth".into(), Tag::Float(100.0)),
                    ("Status".into(), Tag::String("ongoing".into())),
                    (
                        "HeroesOfTheVillage".into(),
                        Tag::List(mc_nbt::ListTag {
                            element_type: mc_nbt::tag_type::INT_ARRAY,
                            elements: Vec::new(),
                        }),
                    ),
                ])],
            }),
        ),
        ("NextAvailableID".into(), Tag::Int(2)),
        ("Tick".into(), Tag::Int(1088304475)),
    ];
    let mut steps = Vec::new();
    upgrade(&mut data, 2586, &mut steps);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].version, 4309);
    let tag = Tag::Compound(vec![
        ("data".into(), Tag::Compound(data)),
        ("DataVersion".into(), Tag::Int(2586)),
    ]);
    // Full entry still fails the typed reader (missing required raid
    // fields), proving the helper renames without fabricating data.
    assert!(crate::raids::from_nbt(&tag).is_err());
    // Empty raid list decodes once renamed.
    let mut empty = vec![
        (
            "Raids".into(),
            Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: Vec::new(),
            }),
        ),
        ("NextAvailableID".into(), Tag::Int(1)),
        ("Tick".into(), Tag::Int(1088304475)),
    ];
    let mut steps = Vec::new();
    upgrade(&mut empty, 2586, &mut steps);
    let tag = Tag::Compound(vec![
        ("data".into(), Tag::Compound(empty)),
        ("DataVersion".into(), Tag::Int(2586)),
    ]);
    let raids = crate::raids::from_nbt(&tag).expect("upgraded raids must decode");
    assert_eq!(raids.next_id, 1);
    assert!(raids.raids.is_empty());
    // Already-current payloads are untouched.
    let mut current = vec![("raids".into(), Tag::Int(0))];
    let mut steps = Vec::new();
    upgrade(&mut current, 4309, &mut steps);
    assert!(steps.is_empty());
}

#[test]
fn malformed_source_publishes_no_destination() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let destination = dir.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("level.dat"), b"not gzip NBT").unwrap();
    let before = hashes(&source).unwrap();
    let receipt = dir.path().join("receipt.json");
    assert!(
        import_anvil(
            &CoreConfig::new(source.clone(), 0),
            &source,
            &destination,
            &receipt
        )
        .is_err()
    );
    assert!(!destination.exists());
    assert_eq!(before, hashes(&source).unwrap());
    let receipt: ConversionReceipt = serde_json::from_slice(&fs::read(receipt).unwrap()).unwrap();
    assert_eq!(receipt.status, "FAIL");
    assert!(receipt.error.is_some());
    assert!(fs::read_dir(dir.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".solaris-conversion-")
    }));
}

#[test]
fn safe_extras_never_admit_runtime_or_executable_paths() {
    for path in [
        "../escape",
        "/etc/passwd",
        "native/state.redb",
        "journal/batch.bin",
        "session.lock",
        "plugins/mod.jar",
    ] {
        assert!(!safe_extra_path(Path::new(path)), "{path}");
    }
    for path in [
        "icon.png",
        "stats/player.json",
        "advancements/player.json",
        "players/advancements/player.json",
        "dimensions/minecraft/overworld/poi/r.0.0.mca",
    ] {
        assert!(safe_extra_path(Path::new(path)), "{path}");
    }
}

#[test]
fn advancement_auxiliary_paths_use_vanilla_player_directory() {
    assert_eq!(
        auxiliary_destination(Path::new("advancements/player.json")).unwrap(),
        Path::new("players/advancements/player.json")
    );
    assert_eq!(
        auxiliary_destination(Path::new("players/advancements/player.json")).unwrap(),
        Path::new("players/advancements/player.json")
    );
}

#[test]
fn nbt_file_adapter_rejects_trailing_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.dat");
    let mut encoder =
        flate2::write::GzEncoder::new(File::create(&path).unwrap(), flate2::Compression::default());
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &Tag::Compound(Vec::new())).unwrap();
    encoder.write_all(&bytes).unwrap();
    encoder.write_all(b"trailing").unwrap();
    encoder.finish().unwrap();
    assert!(
        read_nbt(&path)
            .unwrap_err()
            .to_string()
            .contains("trailing NBT")
    );
}

#[test]
fn destination_created_during_conversion_is_never_overwritten() {
    fn competing_destination(
        config: &CoreConfig,
        _: &Path,
        stage: &Path,
    ) -> Result<ConversionStats, CoreError> {
        fs::write(stage.join("converted"), b"new").map_err(fail)?;
        fs::create_dir(&config.world_dir).map_err(fail)?;
        fs::write(config.world_dir.join("owner-data"), b"original").map_err(fail)?;
        Ok(ConversionStats::default())
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let destination = dir.path().join("destination");
    fs::create_dir(&source).unwrap();
    let receipt = dir.path().join("receipt.json");
    assert!(
        convert(
            "export-anvil",
            &CoreConfig::new(destination.clone(), 0),
            &source,
            &destination,
            &receipt,
            competing_destination
        )
        .is_err()
    );
    assert_eq!(
        fs::read(destination.join("owner-data")).unwrap(),
        b"original"
    );
    assert!(!destination.join("converted").exists());
    let receipt: ConversionReceipt = serde_json::from_slice(&fs::read(receipt).unwrap()).unwrap();
    assert_eq!(receipt.status, "FAIL");
}

#[test]
fn complete_but_unpublished_native_staging_never_admits_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let staging = dir.path().join(".solaris-conversion-interrupted");
    let config = CoreConfig::new(staging.clone(), 42);
    drop(crate::native_checkpoint::tests::fixture_store(&config).unwrap());
    let before = hashes(&staging).unwrap();
    let database = staging.join("native/state.redb");
    let modified = fs::metadata(&database).unwrap().modified().unwrap();
    let error = match crate::Core::open(config) {
        Ok(core) => {
            core.shutdown();
            panic!("unpublished converter staging admitted");
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("staging"), "{error}");
    assert_eq!(before, hashes(&staging).unwrap());
    assert_eq!(
        modified,
        fs::metadata(&database).unwrap().modified().unwrap()
    );
    #[cfg(unix)]
    {
        let alias = dir.path().join("ordinary-world-name");
        std::os::unix::fs::symlink(&staging, &alias).unwrap();
        let error = match crate::Core::open(CoreConfig::new(alias, 42)) {
            Ok(core) => {
                core.shutdown();
                panic!("staging alias admitted");
            }
            Err(error) => error,
        };
        assert!(error.to_string().contains("staging"), "{error}");
        assert_eq!(before, hashes(&staging).unwrap());
        assert_eq!(
            modified,
            fs::metadata(&database).unwrap().modified().unwrap()
        );
    }
}

#[test]
fn auxiliary_paths_follow_vanilla_storage_moves_and_poi_filename_patterns() {
    for (source, destination) in [
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
        (
            "poi/r.-1.2.mca",
            "dimensions/minecraft/overworld/poi/r.-1.2.mca",
        ),
        (
            "DIM-1/poi/c.-32.0.mcc",
            "dimensions/minecraft/the_nether/poi/c.-32.0.mcc",
        ),
        ("data/idcounts.dat", "data/minecraft/maps/last_id.dat"),
        ("data/map_0007.dat", "data/minecraft/maps/0007.dat"),
        ("data/minecraft/map_7.dat", "data/minecraft/maps/7.dat"),
        (
            "data/command_storage_mc_test-1.2.dat",
            "data/mc_test-1.2/command_storage.dat",
        ),
        ("data/scoreboard.dat", "data/minecraft/scoreboard.dat"),
        (
            "data/random_sequences.dat",
            "data/minecraft/random_sequences.dat",
        ),
        ("data/stopwatches.dat", "data/minecraft/stopwatches.dat"),
    ] {
        assert_eq!(
            auxiliary_destination(Path::new(source)).unwrap(),
            Path::new(destination),
            "{source}"
        );
    }
    for path in [
        "region/r.0.0.mca",
        "session.lock",
        "data/minecraft/maps/7.dat",
        "dimensions/minecraft/the_end/poi/r.0.0.mca",
    ] {
        assert_eq!(
            auxiliary_destination(Path::new(path)).unwrap(),
            Path::new(path)
        );
    }
    for path in [
        "poi/r.0.0.mcc",
        "poi/c.0.0.mca",
        "poi/r.+1.0.mca",
        "poi/r.00.0.mca",
        "poi/r.-0.0.mca",
        "poi/r.2147483648.0.mca",
        "poi/r.0.0.mca/extra",
        "../poi/r.0.0.mca",
        "data/command_storage_...dat",
    ] {
        assert!(auxiliary_destination(Path::new(path)).is_err(), "{path}");
    }
}

#[test]
fn player_backup_relocation_requires_canonical_uuid_names() {
    let filename = "00000000-0000-0000-0000-000000000003.dat_old";
    let destination = PathBuf::from(format!("players/data/{filename}"));
    assert_eq!(
        auxiliary_destination(Path::new(&format!("playerdata/{filename}"))).unwrap(),
        destination
    );
    assert!(safe_extra_path(&destination));
    for filename in [
        "not-a-uuid.dat_old",
        "00000000000000000000000000000003.dat_old",
        "00000000-0000-0000-0000-000000000000.dat_old",
        "nested/00000000-0000-0000-0000-000000000003.dat_old",
    ] {
        assert!(auxiliary_destination(Path::new(&format!("playerdata/{filename}"))).is_err());
    }
}
