use super::*;
use crate::core::Core;
use mc_world::native::NativeReadOnlyStore;
use std::path::PathBuf;

const CACHE: usize = 16 * 1024 * 1024;

fn config(dir: &std::path::Path, seed: i64) -> CoreConfig {
    let mut config = CoreConfig::new(dir.to_path_buf(), seed);
    config.checkpoint_every_ticks = u64::MAX;
    config
}

/// A real world, rewritten to record `revision` exactly as an older build left it.
fn world_at(revision: u32) -> (PathBuf, CoreConfig) {
    let dir =
        std::env::temp_dir().join(format!("mc-core-worldgen-upgrade-{}", uuid::Uuid::new_v4()));
    let config = config(&dir, 42);
    Core::open(config.clone()).expect("fresh world").shutdown();
    NativeStore::replace_generation_contract(
        &dir.join("native/state.redb"),
        &contract(&config, mc_worldgen::WORLDGEN_REVISION).unwrap(),
        &contract(&config, revision).unwrap(),
    )
    .unwrap();
    (dir, config)
}

fn stored_identity(dir: &std::path::Path) -> [u8; 32] {
    NativeStore::read_identity(&dir.join("native/state.redb")).unwrap()
}

#[test]
fn older_revision_world_is_refused_then_upgraded_and_opens() {
    let old = mc_worldgen::WORLDGEN_REVISION - 1;
    let (dir, config) = world_at(old);
    let path = dir.join("native/state.redb");
    let old_identity = native_identity_at(&config, old);
    assert_eq!(stored_identity(&dir), old_identity);

    let refusal = Core::open(config.clone())
        .expect_err("default open fails closed")
        .to_string();
    assert!(refusal.starts_with("storage: invalid native record: identity mismatch"));
    assert!(refusal.contains("world upgrade-worldgen"), "{refusal}");

    let before = NativeStore::inspect_read_only(&path, old_identity, CACHE).unwrap();
    let report = upgrade_worldgen_revision(&config).unwrap();
    assert_eq!(
        report,
        WorldgenUpgrade {
            from_revision: old,
            to_revision: mc_worldgen::WORLDGEN_REVISION,
            rewritten: true
        }
    );
    let identity = native_identity_at(&config, mc_worldgen::WORLDGEN_REVISION);
    assert_eq!(stored_identity(&dir), identity);
    // Stored chunks, entities and gameplay rows are exactly what they were.
    let after = NativeStore::inspect_read_only(&path, identity, CACHE).unwrap();
    assert_eq!(format!("{before:?}"), format!("{after:?}"));

    Core::open(config.clone())
        .expect("upgraded world opens")
        .shutdown();
    let store = NativeReadOnlyStore::open_with_cache(&path, identity, CACHE).unwrap();
    let image = store
        .checkpoint_reader()
        .unwrap()
        .read_checkpoint()
        .unwrap();
    let expected = crate::native_checkpoint::generation_records(&config).unwrap();
    for key in ["generation", "dimensions"] {
        assert_eq!(image.globals[key].payload, expected[key].payload);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn different_seed_is_refused_and_leaves_the_world_untouched() {
    let old = mc_worldgen::WORLDGEN_REVISION - 1;
    let (dir, config) = world_at(old);
    let other = self::config(&dir, config.seed + 1);
    let error = upgrade_worldgen_revision(&other).unwrap_err().to_string();
    assert!(
        error.contains("not only in the worldgen revision"),
        "{error}"
    );
    assert_eq!(stored_identity(&dir), native_identity_at(&config, old));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn newer_revision_is_refused_and_leaves_the_world_untouched() {
    let newer = mc_worldgen::WORLDGEN_REVISION + 1;
    let (dir, config) = world_at(newer);
    let error = upgrade_worldgen_revision(&config).unwrap_err().to_string();
    assert!(error.contains("refusing to downgrade"), "{error}");
    assert_eq!(stored_identity(&dir), native_identity_at(&config, newer));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn upgrade_is_idempotent() {
    let (dir, config) = world_at(mc_worldgen::WORLDGEN_REVISION - 1);
    assert!(upgrade_worldgen_revision(&config).unwrap().rewritten);
    let again = upgrade_worldgen_revision(&config).unwrap();
    assert_eq!(again.from_revision, mc_worldgen::WORLDGEN_REVISION);
    assert!(!again.rewritten);
    Core::open(config).expect("still opens").shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn running_world_is_refused() {
    let dir =
        std::env::temp_dir().join(format!("mc-core-worldgen-upgrade-{}", uuid::Uuid::new_v4()));
    let config = config(&dir, 42);
    let core = Core::open(config.clone()).expect("fresh world");
    let error = upgrade_worldgen_revision(&config).unwrap_err().to_string();
    assert!(error.contains("already leased"), "{error}");
    core.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

/// A world as v0.0.8 left it: its checkpoint has no `bossbars` global domain.
fn drop_bossbars(path: &std::path::Path) {
    let database = redb::Database::open(path).unwrap();
    let tx = database.begin_write().unwrap();
    {
        let mut global = tx
            .open_table(redb::TableDefinition::<&str, &[u8]>::new("global"))
            .unwrap();
        assert!(global.remove("bossbars").unwrap().is_some());
    }
    tx.commit().unwrap();
}

#[test]
fn older_storage_format_is_refused_with_export_import_guidance_and_not_written() {
    let (dir, config) = world_at(mc_worldgen::WORLDGEN_REVISION - 1);
    let path = dir.join("native/state.redb");
    drop_bossbars(&path);

    let guided = |error: &str| {
        assert!(error.contains("older Solaris storage format"), "{error}");
        assert!(error.contains("world export-anvil"), "{error}");
        assert!(error.contains("world import-anvil"), "{error}");
        assert!(!error.contains("upgrade-worldgen"), "{error}");
        assert!(!error.contains("missing required domain"), "{error}");
    };
    // The startup hint must not point at the upgrade that would only lead to a dead end.
    guided(&Core::open(config.clone()).unwrap_err().to_string());

    let before = std::fs::read(&path).unwrap();
    guided(&upgrade_worldgen_revision(&config).unwrap_err().to_string());
    assert_eq!(std::fs::read(&path).unwrap(), before, "refusal wrote bytes");
    assert_eq!(
        stored_identity(&dir),
        native_identity_at(&config, mc_worldgen::WORLDGEN_REVISION - 1)
    );

    // A world already rewritten to the current revision gets the same startup guidance.
    let (current_dir, current) = world_at(mc_worldgen::WORLDGEN_REVISION);
    drop_bossbars(&current_dir.join("native/state.redb"));
    guided(&Core::open(current).unwrap_err().to_string());
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(current_dir).unwrap();
}
