use super::*;

fn field<'a>(tag: &'a Tag, key: &str) -> Result<Option<&'a Tag>, CoreError> {
    match tag {
        Tag::Compound(fields) => Ok(mc_data::item_nbt::field(fields, key)),
        _ => Err(fail("expected compound test fixture")),
    }
}

#[test]
fn native_26_3_source_versions_are_accepted_without_accepting_snapshots() {
    for version in [4790, 5023] {
        let tag = Tag::Compound(vec![("DataVersion".into(), Tag::Int(version))]);
        assert_eq!(
            source_version(&tag, Path::new("level.dat")).unwrap(),
            version
        );
        assert_eq!(
            player_source_version(&tag, Path::new("player.dat")).unwrap(),
            version
        );
        assert_eq!(
            saved_data_source_version(&tag, Path::new("scoreboard.dat")).unwrap(),
            version
        );
        check_version(&tag, Path::new("scoreboard.dat")).unwrap();
    }
    for version in [4791, 5006, 5024] {
        let tag = Tag::Compound(vec![("DataVersion".into(), Tag::Int(version))]);
        assert!(source_version(&tag, Path::new("level.dat")).is_err());
    }
}

#[test]
#[ignore = "requires a clean-stopped local 26.3 vanilla world; SOLARIS_26_3_ANVIL_WORLD and SOLARIS_26_3_IMPORT_OUTPUT"]
fn imports_real_native_26_3_vanilla_world_with_pass_receipt() {
    let source = PathBuf::from(std::env::var_os("SOLARIS_26_3_ANVIL_WORLD").expect("source world"));
    let output =
        PathBuf::from(std::env::var_os("SOLARIS_26_3_IMPORT_OUTPUT").expect("artifact directory"));
    fs::create_dir_all(&output).unwrap();
    let destination = output.join("native");
    let mut config = CoreConfig::new(destination.clone(), 263);
    if let Some(cache) = std::env::var_os("SOLARIS_26_3_CONTENT") {
        config.vanilla_data_dir = Some(cache.into());
    }
    let level = read_nbt(&source.join("level.dat")).unwrap();
    assert_eq!(
        declared_version(&level, &source.join("level.dat")).unwrap(),
        5023
    );
    let receipt =
        import_anvil(&config, &source, &destination, &output.join("import.json")).unwrap();
    assert_eq!(receipt.status, "PASS");
    assert!(receipt.counts["chunks"] > 0);
    let exported = output.join("exported");
    let receipt = export_anvil(
        &config,
        &destination,
        &exported,
        &output.join("export.json"),
    )
    .unwrap();
    assert_eq!(receipt.status, "PASS");
    assert_eq!(receipt.data_version, 5023);
    let level = read_nbt(&exported.join("level.dat")).unwrap();
    let data = field(&level, "Data").unwrap().unwrap();
    assert_eq!(field(data, "DataVersion").unwrap(), Some(&Tag::Int(5023)));
    let version = field(data, "Version").unwrap().unwrap();
    assert_eq!(
        field(version, "Name").unwrap(),
        Some(&Tag::String("26.3".into()))
    );
    assert_eq!(field(version, "Id").unwrap(), Some(&Tag::Int(5023)));
}
