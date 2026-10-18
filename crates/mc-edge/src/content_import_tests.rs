use std::io::Write;
use std::path::Path;

use super::{
    ContentSource, ImportReport, ImportRequest, finish_import, import_content,
    install_version_json, manifest_version_entry, parse_version_metadata, publish,
    require_target_version, stage_local_jar, staging_dir, wanted_data_entry,
};
use crate::content_cache::tests::write_valid_cache;

fn write_bundle(path: &Path, version: &str) {
    let file = std::fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("version.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(
        archive,
        "{{\"id\":\"{version}\",\"world_version\":{},\"protocol_version\":{}}}",
        mc_protocol::WORLD_VERSION,
        mc_protocol::PROTOCOL_VERSION
    )
    .unwrap();
    archive.finish().unwrap();
}

#[test]
fn manifest_selection_uses_the_pinned_version_not_latest() {
    let manifest = r#"{
        "latest": {"release": "26.2"},
        "versions": [
            {"id":"26.2","url":"https://example.invalid/newer","sha1":"newer"},
            {"id":"26.1.2","url":"https://example.invalid/pinned","sha1":"pinned"}
        ]
    }"#;
    let entry = manifest_version_entry(manifest, mc_protocol::TARGET_RELEASE).unwrap();
    assert_eq!(entry.url, "https://example.invalid/pinned");
    assert_eq!(entry.sha1, "pinned");
    assert!(manifest_version_entry(manifest, "missing").is_err());
    let metadata = parse_version_metadata(
        r#"{
        "id":"26.1.2",
        "downloads":{"server":{"url":"https://example.invalid/server.jar","sha1":"abc","size":42}}
    }"#,
    )
    .unwrap();
    assert_eq!(metadata.id, mc_protocol::TARGET_RELEASE);
    assert_eq!(metadata.server.unwrap().size, 42);
    assert!(require_target_version("26.2").is_err());
}

#[tokio::test]
async fn rejected_version_does_not_create_staging_or_change_existing_cache() {
    let parent = tempfile::tempdir().unwrap();
    let cache = parent.path().join("cache");
    write_valid_cache(&cache);
    let before = std::fs::read(cache.join("version.json")).unwrap();
    let error = import_content(&ImportRequest {
        version: "26.2".into(),
        source: ContentSource::Download,
        cache: cache.clone(),
    })
    .await
    .unwrap_err();
    assert!(error.to_string().contains("cannot derive content"));
    assert_eq!(std::fs::read(cache.join("version.json")).unwrap(), before);
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn actual_local_import_failure_preserves_complete_cache_and_discards_staging() {
    let parent = tempfile::tempdir().unwrap();
    let cache = parent.path().join("cache");
    write_valid_cache(&cache);
    std::fs::write(cache.join("operator-marker"), "keep").unwrap();
    let before = std::fs::read(cache.join("version.json")).unwrap();
    let error = import_content(&ImportRequest {
        version: mc_protocol::TARGET_RELEASE.into(),
        source: ContentSource::LocalJar(parent.path().join("missing.jar")),
        cache: cache.clone(),
    })
    .await
    .unwrap_err();
    assert!(error.to_string().contains("supplied jar"), "{error:#}");
    assert_eq!(std::fs::read(cache.join("version.json")).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(cache.join("operator-marker")).unwrap(),
        "keep"
    );
    crate::content_cache::validate_content_cache(&cache).unwrap();
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn foreign_local_bundle_fails_before_java_or_network_and_preserves_cache() {
    let parent = tempfile::tempdir().unwrap();
    let cache = parent.path().join("cache");
    write_valid_cache(&cache);
    let before = std::fs::read(cache.join("version.json")).unwrap();
    let jar = parent.path().join("licensed.jar");
    write_bundle(&jar, "26.2");
    let error = import_content(&ImportRequest {
        version: mc_protocol::TARGET_RELEASE.into(),
        source: ContentSource::LocalJar(jar),
        cache: cache.clone(),
    })
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("supplied jar is 26.2"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(cache.join("version.json")).unwrap(), before);
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 2);
}

#[test]
fn local_jar_staging_and_version_check_are_offline() {
    let parent = tempfile::tempdir().unwrap();
    let jar = parent.path().join("licensed.jar");
    write_bundle(&jar, mc_protocol::TARGET_RELEASE);
    let staging = staging_dir(&parent.path().join("cache")).unwrap();
    let staged = stage_local_jar(&jar, staging.path()).unwrap();
    install_version_json(&staged, staging.path()).unwrap();
    crate::startup_data::validate_vanilla_sidecar_version(staging.path()).unwrap();
    assert_eq!(std::fs::read(staged).unwrap(), std::fs::read(jar).unwrap());
}

#[test]
fn failed_validation_funnels_through_without_touching_live_cache() {
    let parent = tempfile::tempdir().unwrap();
    let cache = parent.path().join("cache");
    write_valid_cache(&cache);
    let before = std::fs::read(cache.join("version.json")).unwrap();
    let staging = staging_dir(&cache).unwrap();
    std::fs::write(staging.path().join("partial"), "half-derived").unwrap();
    let derived = super::validate_staged_cache(staging.path()).map(|()| ImportReport {
        cache: staging.path().to_path_buf(),
        registries: 0,
        entries: 0,
    });
    let error = finish_import(&cache, staging.path(), derived).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("derived vanilla content cache failed validation")
    );
    assert!(!staging.path().exists());
    assert_eq!(std::fs::read(cache.join("version.json")).unwrap(), before);
    crate::content_cache::validate_content_cache(&cache).unwrap();
}

#[test]
fn publish_replaces_only_after_derivation_and_restores_on_rename_failure() {
    let parent = tempfile::tempdir().unwrap();
    let cache = parent.path().join("cache");
    std::fs::create_dir(&cache).unwrap();
    std::fs::write(cache.join("marker"), "previous").unwrap();
    let staging = parent.path().join("staging");
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(staging.join("marker"), "replacement").unwrap();
    publish(&staging, &cache).unwrap();
    assert_eq!(
        std::fs::read_to_string(cache.join("marker")).unwrap(),
        "replacement"
    );
    assert!(!staging.exists());
    assert!(!parent.path().join(".cache.previous").exists());
    let error = publish(&parent.path().join("missing-staging"), &cache).unwrap_err();
    assert!(error.to_string().contains("publish"));
    assert_eq!(
        std::fs::read_to_string(cache.join("marker")).unwrap(),
        "replacement"
    );
    assert!(!parent.path().join(".cache.previous").exists());
}

#[test]
fn staging_is_unique_and_failed_import_does_not_consume_another_import() {
    let parent = tempfile::tempdir().unwrap();
    let cache = parent.path().join("cache");
    let first = staging_dir(&cache).unwrap();
    std::fs::write(first.path().join("marker"), "first").unwrap();
    let second = staging_dir(&cache).unwrap();
    assert_ne!(first.path(), second.path());
    let _ = finish_import(
        &cache,
        second.path(),
        Err(anyhow::anyhow!("datagen failed")),
    );
    assert_eq!(
        std::fs::read_to_string(first.path().join("marker")).unwrap(),
        "first"
    );
}

#[test]
fn extraction_selects_consumed_content_and_rejects_path_traversal() {
    for relative in [
        "worldgen/structure_set/villages.json",
        "structure/village/house.nbt",
        "tags/block/mineable/pickaxe.json",
        "recipe/stick.json",
        "loot_table/blocks/stone.json",
        "dimension_type/overworld.json",
        "advancement/story/root.json",
    ] {
        assert_eq!(
            wanted_data_entry(&format!("data/minecraft/{relative}")),
            Some(relative.into())
        );
    }
    assert!(wanted_data_entry("data/minecraft/lang/en_us.json").is_none());
    assert!(wanted_data_entry("data/minecraft/tags/../../escape.json").is_none());
    assert!(wanted_data_entry("../data/minecraft/recipe/stick.json").is_none());
}
