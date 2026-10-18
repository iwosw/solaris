use super::*;
use std::fs;
use tempfile::TempDir;

fn make_minimal_layout() -> TempDir {
    let dir = TempDir::new().unwrap();
    let mc = dir.path().join("data").join("minecraft");
    for (registry_path, _) in KNOWN_REGISTRIES {
        let registry_dir = mc.join(registry_path);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::write(registry_dir.join("alpha.json"), "{}").unwrap();
        fs::write(registry_dir.join("beta.json"), "{}").unwrap();
    }
    dir
}

fn write_captured_payloads(dir: &Path) {
    for (registry_path, _) in KNOWN_REGISTRIES {
        let registry = Identifier::parse(format!("minecraft:{registry_path}")).unwrap();
        for entry_path in ["alpha", "beta"] {
            let entry = Identifier::parse(format!("minecraft:{entry_path}")).unwrap();
            let tag = if *registry_path == "dimension_type" && entry_path == "alpha" {
                mc_nbt::Tag::Compound(vec![
                    ("height".into(), mc_nbt::Tag::Int(384)),
                    ("source".into(), mc_nbt::Tag::String("captured".into())),
                ])
            } else {
                mc_nbt::Tag::Compound(vec![(
                    "source".into(),
                    mc_nbt::Tag::String("captured".into()),
                )])
            };
            let mut payload = Vec::new();
            mc_nbt::write_network(&mut payload, &tag).unwrap();
            let path = network_registry_payload_path(dir, &registry, &entry).unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, payload).unwrap();
        }
    }
}

#[test]
fn load_indexes_every_known_registry() {
    let dir = make_minimal_layout();
    let data = load(dir.path()).unwrap();
    assert_eq!(data.sidecar_root(), Some(dir.path()));
    assert_eq!(data.registry_count(), KNOWN_REGISTRIES.len());
    assert!(!data.has_full_registry_payloads());
    for (registry_path, _) in KNOWN_REGISTRIES {
        let reg = data
            .registry(registry_path)
            .unwrap_or_else(|| panic!("missing registry {registry_path}"));
        assert_eq!(reg.entries.len(), 2);
        assert_eq!(
            reg.entries[0].as_str(),
            "minecraft:alpha",
            "entries should be lexicographically sorted"
        );
    }
}

#[test]
fn load_indexes_every_captured_network_nbt_payload() {
    let dir = make_minimal_layout();
    write_captured_payloads(dir.path());

    let data = load(dir.path()).unwrap();
    assert!(data.has_full_registry_payloads());
    let registry = data.registry("dimension_type").unwrap();
    let entry = registry
        .entries
        .iter()
        .find(|entry| entry.as_str() == "minecraft:alpha")
        .unwrap();
    let payload = data
        .registry_entry_payload(&registry.id, entry)
        .expect("loaded sidecar entry has a payload");
    let mut payload = payload.as_ref();
    let tag = mc_nbt::read_network(&mut payload).unwrap();
    assert!(payload.is_empty());
    assert_eq!(
        tag,
        mc_nbt::Tag::Compound(vec![
            ("height".into(), mc_nbt::Tag::Int(384)),
            ("source".into(), mc_nbt::Tag::String("captured".into())),
        ])
    );
}

#[cfg(unix)]
#[test]
fn load_rejects_symlinked_registry_entry() {
    use std::os::unix::fs::symlink;

    let dir = make_minimal_layout();
    let link = dir.path().join("data/minecraft/dimension_type/alpha.json");
    fs::remove_file(&link).unwrap();
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("alpha.json");
    fs::write(&target, "{}").unwrap();
    symlink(&target, &link).unwrap();

    let error = load(dir.path()).expect_err("registry walk must not follow symlinks");
    assert!(matches!(
        error,
        DataError::Io { path, source }
            if path == link && source.kind() == std::io::ErrorKind::InvalidData
    ));
}

#[cfg(unix)]
#[test]
fn load_rejects_symlinked_registry_ancestor() {
    use std::os::unix::fs::symlink;

    let dir = make_minimal_layout();
    let worldgen = dir.path().join("data/minecraft/worldgen");
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("worldgen");
    fs::rename(&worldgen, &target).unwrap();
    symlink(&target, &worldgen).unwrap();

    let error = load(dir.path()).expect_err("registry roots must not cross symlink ancestors");
    assert!(matches!(
        error,
        DataError::Io { path, source }
            if path == worldgen && source.kind() == std::io::ErrorKind::InvalidData
    ));
}

#[cfg(unix)]
#[test]
fn captured_registry_payload_rejects_symlink_escape() {
    use std::os::unix::fs::symlink;

    let dir = make_minimal_layout();
    write_captured_payloads(dir.path());
    let registry = Identifier::parse("minecraft:dimension_type").unwrap();
    let entry = Identifier::parse("minecraft:alpha").unwrap();
    let path = network_registry_payload_path(dir.path(), &registry, &entry).unwrap();
    fs::remove_file(&path).unwrap();

    let outside = TempDir::new().unwrap();
    let outside_path = outside.path().join("payload.nbt");
    let mut payload = Vec::new();
    mc_nbt::write_network(&mut payload, &mc_nbt::Tag::Compound(Vec::new())).unwrap();
    fs::write(&outside_path, payload).unwrap();
    symlink(&outside_path, &path).unwrap();

    let error = load(dir.path()).expect_err("payload symlink must stay below data root");
    assert!(matches!(
        error,
        DataError::ResourcePath(ResourcePathError::EscapesRoot { .. })
    ));
}

#[test]
fn partial_captured_payload_directory_fails_closed() {
    let dir = make_minimal_layout();
    let registry = Identifier::parse("minecraft:dimension_type").unwrap();
    let entry = Identifier::parse("minecraft:alpha").unwrap();
    let path = network_registry_payload_path(dir.path(), &registry, &entry).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut payload = Vec::new();
    mc_nbt::write_network(&mut payload, &mc_nbt::Tag::Compound(Vec::new())).unwrap();
    fs::write(path, payload).unwrap();

    let error = load(dir.path()).unwrap_err();
    assert!(matches!(error, DataError::Io { .. }));
}

#[test]
fn embedded_registry_index_has_no_invented_payloads() {
    let data = solaris_required_data();
    assert!(!data.has_full_registry_payloads());
    let registry = data.registry("dimension_type").unwrap();
    assert_eq!(
        data.registry_entry_payload(&registry.id, &registry.entries[0]),
        None
    );
}

#[test]
fn registry_lookup_accepts_namespaced_id() {
    let dir = make_minimal_layout();
    let data = load(dir.path()).unwrap();
    assert!(data.registry("minecraft:dimension_type").is_some());
    assert!(data.registry("dimension_type").is_some());
    assert!(data.registry("nonexistent").is_none());
}

#[test]
fn embedded_required_registry_index_covers_known_registries() {
    let data = solaris_required_data();

    assert!(data.sidecar_root().is_none());
    assert_eq!(data.registry_count(), KNOWN_REGISTRIES.len());
    assert!(data.entry_count() > 300);
    assert!(
        data.registry("minecraft:worldgen/biome")
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.as_str() == "minecraft:plains")
    );
    assert!(
        data.registry("minecraft:dimension_type")
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.as_str() == "minecraft:overworld")
    );
    assert_eq!(
        data.registry("minecraft:test_environment")
            .unwrap()
            .entries
            .iter()
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["minecraft:default"]
    );
    assert_eq!(
        data.registry("minecraft:test_instance")
            .unwrap()
            .entries
            .iter()
            .map(Identifier::as_str)
            .collect::<Vec<_>>(),
        vec!["minecraft:always_pass"]
    );
    assert!(data.registry("minecraft:enchantment_provider").is_none());
    assert!(data.registry("minecraft:trade_set").is_none());
    assert!(data.registry("minecraft:villager_trade").is_none());
}

#[test]
fn missing_root_is_reported_clearly() {
    let err = load(PathBuf::from("/definitely/does/not/exist")).unwrap_err();
    assert!(matches!(err, DataError::Missing(_)));
}

#[test]
fn empty_registry_is_an_error() {
    let dir = TempDir::new().unwrap();
    let mc = dir.path().join("data").join("minecraft");
    for (registry_path, _) in KNOWN_REGISTRIES {
        let registry_dir = mc.join(registry_path);
        fs::create_dir_all(&registry_dir).unwrap();
        if registry_path != &"dimension_type" {
            fs::write(registry_dir.join("alpha.json"), "{}").unwrap();
        }
    }
    let err = load(dir.path()).unwrap_err();
    assert!(matches!(
        err,
        DataError::EmptyRegistry { ref registry, .. } if registry == "dimension_type"
    ));
}

#[test]
fn nested_entries_use_forward_slashes() {
    let dir = TempDir::new().unwrap();
    let mc = dir.path().join("data").join("minecraft");
    for (registry_path, _) in KNOWN_REGISTRIES {
        let registry_dir = mc.join(registry_path);
        fs::create_dir_all(&registry_dir).unwrap();
        fs::write(registry_dir.join("alpha.json"), "{}").unwrap();
    }
    let biome = mc.join("worldgen").join("biome");
    fs::create_dir_all(biome.join("nether")).unwrap();
    fs::write(biome.join("nether").join("warped_forest.json"), "{}").unwrap();

    let data = load(dir.path()).unwrap();
    let reg = data.registry("worldgen/biome").unwrap();
    let nested = reg
        .entries
        .iter()
        .find(|e| e.path() == "nether/warped_forest")
        .expect("nested entry indexed");
    assert_eq!(nested.as_str(), "minecraft:nether/warped_forest");
}

/// Opt-in sanity check against a real local vanilla extraction. The
/// ignored gate keeps ordinary CI independent of `.analysis/server.jar`.
#[test]
#[ignore = "requires populated local 26.1.2 data/vanilla sidecars"]
fn loads_real_vanilla_sidecar_when_present() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("data")
        .join("vanilla");
    let minecraft_data = path.join("data").join("minecraft");
    assert!(
        minecraft_data.is_dir(),
        "{} not populated; run tools/extract-vanilla-data.sh",
        minecraft_data.display()
    );
    let data = load(&path).expect("real vanilla data should load");
    assert!(data.registry_count() >= KNOWN_REGISTRIES.len());
    let dim = data.registry("dimension_type").unwrap();
    assert!(
        dim.entries
            .iter()
            .any(|id| id.as_str() == "minecraft:overworld"),
        "vanilla dimension_type should always contain overworld; got {:?}",
        dim.entries
    );
    let biomes = data.registry("worldgen/biome").unwrap();
    assert!(biomes.entries.len() >= 30, "expected ≥ 30 vanilla biomes");
    assert!(data.registry("test_environment").is_some());
    assert!(data.registry("test_instance").is_some());
    assert!(data.registry("enchantment_provider").is_none());
    assert!(data.registry("trade_set").is_none());
    assert!(data.registry("villager_trade").is_none());
}
