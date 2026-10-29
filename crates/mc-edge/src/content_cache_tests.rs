use std::path::{Path, PathBuf};

use super::{
    ContentSearch, IMPORT_COMMAND, REQUIRED_FILES, REQUIRED_REGISTRY_PAYLOAD_DIR,
    discover_content_cache, validate_content_cache,
};

fn write_json(root: &Path, relative: &str, value: &serde_json::Value) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

/// Synthetic cache exercising all production validators. Block loot sources come
/// from the checkout's local vanilla data (explicitly allowed for tests); the
/// rest stays synthetic so no Mojang artifact, Java process, or network is needed.
pub(crate) fn write_valid_cache(root: &Path) {
    use serde_json::json;

    std::fs::create_dir_all(root).unwrap();
    for relative in REQUIRED_FILES {
        if !relative.ends_with(".json") {
            std::fs::create_dir_all(root.join(relative)).unwrap();
        } else {
            write_json(root, relative, &json!({}));
        }
    }
    write_json(
        root,
        "version.json",
        &json!({
            "id": mc_protocol::TARGET_RELEASE,
            "world_version": mc_protocol::WORLD_VERSION,
            "protocol_version": mc_protocol::PROTOCOL_VERSION,
        }),
    );
    write_json(
        root,
        crate::content_import::GAMEPLAY_VERSION_FILE,
        &json!({
            "id": crate::content_import::GAMEPLAY_DATAPACK_RELEASE,
            "world_version": 4790,
            "protocol_version": 775,
        }),
    );
    for (registry, directory) in mc_data::KNOWN_REGISTRIES {
        write_json(
            root,
            &format!("data/minecraft/{directory}/sample.json"),
            &json!({}),
        );
        let registry = mc_data::Identifier::parse(format!("minecraft:{registry}")).unwrap();
        let entry = mc_data::Identifier::parse("minecraft:sample").unwrap();
        let path = mc_data::network_registry_payload_path(root, &registry, &entry).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &mc_nbt::Tag::Compound(Vec::new())).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    write_json(
        root,
        "reports/registries.json",
        &json!({
            "minecraft:block": {"entries": {"minecraft:stone": {"protocol_id": 1}}},
            "minecraft:item": {"entries": {"minecraft:stick": {"protocol_id": 1}}},
            "minecraft:entity_type": {"entries": {"minecraft:pig": {"protocol_id": 1}}},
        }),
    );
    for (registry, entry) in [
        ("block", "stone"),
        ("item", "stick"),
        ("entity_type", "pig"),
    ] {
        write_json(
            root,
            &format!("data/minecraft/tags/{registry}/sample.json"),
            &json!({"values": [format!("minecraft:{entry}")]}),
        );
    }
    let state_count = mc_data::blocks::solaris_required_blocks_report()
        .iter()
        .flat_map(|block| block.states.iter().map(|state| state.id as usize + 1))
        .max()
        .unwrap();
    write_json(
        root,
        "reports/block_map_colors.json",
        &json!(vec![0; state_count]),
    );
    write_json(
        root,
        "reports/block_light.json",
        &json!({
            "version": mc_protocol::TARGET_RELEASE,
            "max_state_id": state_count - 1,
            "entries": vec![[0, 0, 1, 0]; state_count],
        }),
    );
    write_json(
        root,
        "reports/block_mining.json",
        &json!({
            "version": mc_protocol::TARGET_RELEASE,
            "max_state_id": state_count - 1,
            "entries": vec![(1.0, 0); state_count],
        }),
    );
    write_json(
        root,
        "reports/block_explosion.json",
        &json!({
            "version": mc_protocol::TARGET_RELEASE,
            "max_state_id": state_count - 1,
            "entries": vec![0.0; state_count],
        }),
    );
    write_json(
        root,
        "reports/minecraft/components/item/stick.json",
        &json!({
            "components": {"minecraft:max_stack_size": 64},
        }),
    );
    write_json(
        root,
        "data/minecraft/recipe/stick.json",
        &json!({
            "type": "minecraft:crafting_shapeless",
            "ingredients": [{"item": "minecraft:oak_planks"}],
            "result": {"id": "minecraft:stick", "count": 4},
        }),
    );
    let checkout_blocks = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/data/minecraft/loot_table/blocks");
    std::fs::create_dir_all(root.join("data/minecraft/loot_table/blocks")).unwrap();
    for entry in std::fs::read_dir(&checkout_blocks).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let name = path.file_name().unwrap();
        std::fs::copy(
            &path,
            root.join("data/minecraft/loot_table/blocks").join(name),
        )
        .unwrap();
    }
    write_json(
        root,
        "data/minecraft/loot_table/blocks/stone.json",
        &json!({
            "pools": [{"entries": [{"type": "minecraft:item", "name": "minecraft:stone"}]}],
        }),
    );
    write_json(
        root,
        "data/minecraft/advancement/sample.json",
        &json!({"criteria": {"sample": {"trigger": "minecraft:impossible"}}}),
    );
}

fn search(standard: Vec<PathBuf>) -> ContentSearch {
    ContentSearch {
        explicit: None,
        environment: None,
        standard,
    }
}

#[test]
fn missing_cache_names_import_instruction_and_all_locations() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("missing-a");
    let b = temp.path().join("missing-b");
    let error = format!(
        "{:#}",
        discover_content_cache(&search(vec![a.clone(), b.clone()])).unwrap_err()
    );
    assert!(error.contains(IMPORT_COMMAND), "{error}");
    assert!(error.contains("JDK 25"), "{error}");
    assert!(error.contains(&a.display().to_string()), "{error}");
    assert!(error.contains(&b.display().to_string()), "{error}");
    assert!(!a.exists());
    assert!(!b.exists());
}

#[test]
fn cache_rejects_missing_artifacts_and_empty_network_payload_directory() {
    let temp = tempfile::tempdir().unwrap();
    write_valid_cache(temp.path());
    validate_content_cache(temp.path()).unwrap();
    std::fs::remove_dir_all(temp.path().join(REQUIRED_REGISTRY_PAYLOAD_DIR)).unwrap();
    let error = format!("{:#}", validate_content_cache(temp.path()).unwrap_err());
    assert!(error.contains(REQUIRED_REGISTRY_PAYLOAD_DIR), "{error}");
    // Keep the current payload directory layout, but no payload files.
    for (registry, _) in mc_data::KNOWN_REGISTRIES {
        let registry = mc_data::Identifier::parse(format!("minecraft:{registry}")).unwrap();
        let entry = mc_data::Identifier::parse("minecraft:sample").unwrap();
        let path = mc_data::network_registry_payload_path(temp.path(), &registry, &entry).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    let error = format!("{:#}", validate_content_cache(temp.path()).unwrap_err());
    assert!(error.contains("captured RegistryData payloads"), "{error}");
    assert!(error.contains("minecraft:sample"), "{error}");
    assert!(error.contains("have no such entry"), "{error}");
    std::fs::remove_file(temp.path().join("reports/block_mining.json")).unwrap();
    let error = format!("{:#}", validate_content_cache(temp.path()).unwrap_err());
    assert!(
        error.contains("missing reports/block_mining.json"),
        "{error}"
    );
}

#[test]
fn cache_rejects_foreign_release_world_and_protocol_versions() {
    let temp = tempfile::tempdir().unwrap();
    for (field, wrong, expected) in [
        ("id", serde_json::json!("not-the-target"), "release id"),
        ("world_version", serde_json::json!(1), "world_version"),
        ("protocol_version", serde_json::json!(1), "protocol_version"),
    ] {
        let mut version = serde_json::json!({
            "id": mc_protocol::TARGET_RELEASE,
            "world_version": mc_protocol::WORLD_VERSION,
            "protocol_version": mc_protocol::PROTOCOL_VERSION,
        });
        version[field] = wrong;
        write_json(temp.path(), "version.json", &version);
        let error = format!("{:#}", validate_content_cache(temp.path()).unwrap_err());
        assert!(error.contains(expected), "{error}");
        assert!(error.contains("does not match"), "{error}");
    }
}

#[test]
fn cache_rejects_corrupt_payloads_and_gameplay_data() {
    let temp = tempfile::tempdir().unwrap();
    write_valid_cache(temp.path());
    let registry = mc_data::Identifier::parse("minecraft:damage_type").unwrap();
    let entry = mc_data::Identifier::parse("minecraft:sample").unwrap();
    let payload = mc_data::network_registry_payload_path(temp.path(), &registry, &entry).unwrap();
    let before = std::fs::read(&payload).unwrap();
    std::fs::write(&payload, b"invalid nbt").unwrap();
    assert!(validate_content_cache(temp.path()).is_err());
    std::fs::write(payload, before).unwrap();
    write_json(
        temp.path(),
        "reports/block_mining.json",
        &serde_json::json!({
            "version": mc_protocol::TARGET_RELEASE,
            "max_state_id": 0,
            "entries": [[1.0, 0]],
        }),
    );
    let error = format!("{:#}", validate_content_cache(temp.path()).unwrap_err());
    assert!(error.contains("requires state id"), "{error}");
}

#[test]
fn authoritative_overrides_never_fall_through_to_a_valid_standard_cache() {
    let temp = tempfile::tempdir().unwrap();
    let valid = temp.path().join("valid");
    write_valid_cache(&valid);
    let missing = temp.path().join("missing");
    let explicit = ContentSearch {
        explicit: Some(missing.clone()),
        environment: Some(valid.clone()),
        standard: vec![valid.clone()],
    };
    let environment = ContentSearch {
        explicit: None,
        environment: Some(missing.clone()),
        standard: vec![valid.clone()],
    };
    for search in [explicit, environment] {
        let error = format!("{:#}", discover_content_cache(&search).unwrap_err());
        assert!(
            error.contains("configured vanilla content cache"),
            "{error}"
        );
        assert!(error.contains(&missing.display().to_string()), "{error}");
        assert!(error.contains(IMPORT_COMMAND), "{error}");
    }
    let precedence = ContentSearch {
        explicit: Some(valid.clone()),
        environment: Some(missing),
        standard: Vec::new(),
    };
    assert_eq!(discover_content_cache(&precedence).unwrap().root(), valid);
}

#[test]
fn standards_skip_incomplete_roots_and_prefer_the_first_complete_cache() {
    let temp = tempfile::tempdir().unwrap();
    let incomplete = temp.path().join("partial");
    std::fs::create_dir_all(&incomplete).unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    write_valid_cache(&first);
    write_valid_cache(&second);
    assert_eq!(
        discover_content_cache(&search(vec![incomplete, first.clone(), second]))
            .unwrap()
            .root(),
        first
    );
}

#[test]
fn search_supports_xdg_without_home_and_home_fallback_and_checkout_ancestors() {
    let home = Path::new("/operator/home");
    let xdg = Path::new("/operator/data");
    let cwd = Path::new("/checkout/crates/mc-edge");
    let with_xdg = ContentSearch::from_locations(None, None, Some(xdg), None, cwd);
    assert_eq!(
        with_xdg.import_target(),
        xdg.join("solaris/content")
            .join(mc_protocol::TARGET_RELEASE)
    );
    assert!(
        with_xdg
            .standard
            .contains(&PathBuf::from("/checkout/data/vanilla"))
    );
    let with_home = ContentSearch::from_locations(None, None, None, Some(home), cwd);
    assert_eq!(
        with_home.import_target(),
        home.join(".local/share/solaris/content")
            .join(mc_protocol::TARGET_RELEASE)
    );
    let relative_xdg =
        ContentSearch::from_locations(None, None, Some(Path::new("relative")), Some(home), cwd);
    assert_eq!(relative_xdg.import_target(), with_home.import_target());
    let explicit = ContentSearch::from_locations(
        Some(Path::new("configured")),
        Some("environment".into()),
        Some(xdg),
        Some(home),
        cwd,
    );
    assert_eq!(explicit.import_target(), Path::new("configured"));
    let environment =
        ContentSearch::from_locations(None, Some("environment".into()), Some(xdg), Some(home), cwd);
    assert_eq!(environment.import_target(), Path::new("environment"));
}

#[test]
fn startup_and_verify_share_discovery_and_pass_the_resolved_root_to_core() {
    let temp = tempfile::tempdir().unwrap();
    let data_home = temp.path().join("fresh-install-xdg");
    let search = ContentSearch::from_locations(
        None,
        None,
        Some(&data_home),
        None,
        &temp.path().join("unrelated-cwd"),
    );
    let expected = search.import_target();
    write_valid_cache(&expected);
    let verified = discover_content_cache(&search).unwrap();
    let mut cfg = crate::config::EdgeConfig::default();
    assert!(cfg.data.vanilla_data_dir.as_os_str().is_empty());
    crate::server::resolve_content_config_from_search(&mut cfg, &search).unwrap();
    assert_eq!(cfg.data.vanilla_data_dir, verified.root());
    assert_eq!(
        crate::server::core_config(&cfg).vanilla_data_dir.as_deref(),
        Some(expected.as_path())
    );
    // The real loader sees the same explicit resolved root, independent of the
    // test process's environment or repository cache.
    let loaded = crate::server::load_vanilla_data(&cfg).unwrap();
    assert_eq!(loaded.root(), verified.root());
    assert!(loaded.has_full_registry_payloads());
    crate::server::load_tags(&cfg, &loaded).unwrap();
    crate::server::resolve_content_config(&mut cfg).unwrap();
    assert_eq!(cfg.data.vanilla_data_dir, expected);
}

#[test]
fn missing_startup_cache_fails_without_attempting_import_or_mutating_config() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("cache");
    let mut cfg = crate::config::EdgeConfig::default();
    cfg.data.vanilla_data_dir = missing.clone();
    let error = format!(
        "{:#}",
        crate::server::resolve_content_config(&mut cfg).unwrap_err()
    );
    assert!(error.contains(IMPORT_COMMAND), "{error}");
    assert_eq!(cfg.data.vanilla_data_dir, missing);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn resolved_protocol_and_tag_loaders_do_not_repeat_full_gameplay_validation() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    write_valid_cache(&cache);
    let mut cfg = crate::config::EdgeConfig::default();
    cfg.data.vanilla_data_dir = cache.clone();
    crate::server::resolve_content_config(&mut cfg).unwrap();
    // Once startup has validated the complete cache, the protocol/tag loaders
    // only load their own data rather than parsing every gameplay report again.
    std::fs::remove_file(cache.join("reports/block_mining.json")).unwrap();
    let data = crate::server::load_vanilla_data(&cfg).unwrap();
    assert_eq!(data.root(), cache);
    crate::server::load_tags(&cfg, &data).unwrap();
    // The unresolved-library path also keeps the root chosen by its data
    // loader, instead of consulting the environment or checkout a second time.
    cfg.data.vanilla_data_dir = PathBuf::new();
    crate::server::load_tags(&cfg, &data).unwrap();
}

#[test]
fn cache_requires_parseable_nonempty_advancement_data() {
    let temp = tempfile::tempdir().unwrap();
    write_valid_cache(temp.path());
    let advancement = temp.path().join("data/minecraft/advancement");
    std::fs::remove_dir_all(&advancement).unwrap();
    let error = format!("{:#}", validate_content_cache(temp.path()).unwrap_err());
    assert!(
        error.contains("missing data/minecraft/advancement"),
        "{error}"
    );
    std::fs::create_dir(&advancement).unwrap();
    let error = format!("{:#}", validate_content_cache(temp.path()).unwrap_err());
    assert!(error.contains("no advancement data"), "{error}");
    std::fs::write(advancement.join("bad.json"), "{}").unwrap();
    let error = format!("{:#}", validate_content_cache(temp.path()).unwrap_err());
    assert!(error.contains("advancement"), "{error}");
}
