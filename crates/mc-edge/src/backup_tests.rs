use super::*;
use mc_world::native::{NativeReadOnlyStore, NativeStore};

fn seed_world(root: &Path) {
    std::fs::create_dir_all(root.join("native")).unwrap();
    drop(NativeStore::create(&root.join(NATIVE_DATABASE), [7; 32]).unwrap());
    let mut journal = mc_core::journal::JournalWriter::open(root, 1).unwrap();
    journal.append(1, 1, &[]).unwrap();
    journal.flush().unwrap();
    std::fs::create_dir_all(root.join("operator")).unwrap();
    std::fs::write(root.join("operator/world.json"), b"world metadata").unwrap();
}

fn fixture_backup(root: &Path) -> (PathBuf, PathBuf) {
    let world = root.join("world");
    seed_world(&world);
    let backup = root.join("backup");
    run_backup(&world, &backup).unwrap();
    (world, backup)
}

fn change_manifest(backup: &Path, change: impl FnOnce(&mut serde_json::Value)) {
    let path = backup.join(BACKUP_MANIFEST);
    let mut manifest = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    change(&mut manifest);
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

#[test]
fn backup_roundtrip_preserves_real_core_store_wal_and_metadata() {
    let root = tempfile::tempdir().unwrap();
    let world = root.path().join("world");
    let mut config = mc_core::CoreConfig::new(world.clone(), 42);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    config.view_distance = 1;
    config.simulation_distance = 0;
    let core = mc_core::Core::open(config).unwrap();
    core.shutdown();
    drop(core);

    let mut identity = [0; 32];
    identity[..16].copy_from_slice(b"SolarisV2Native\0");
    identity[16..24].copy_from_slice(&42_i64.to_be_bytes());
    identity[24..].copy_from_slice(&(mc_worldgen::WORLDGEN_REVISION as u64).to_be_bytes());
    let store =
        NativeReadOnlyStore::open_with_cache(&world.join(NATIVE_DATABASE), identity, 1024 * 1024)
            .unwrap();
    let image = store
        .checkpoint_reader()
        .unwrap()
        .read_checkpoint()
        .unwrap();
    drop(store);
    let mut journal = mc_core::journal::JournalWriter::open(&world, 1).unwrap();
    journal.append(image.tick + 1, image.tick + 1, &[]).unwrap();
    journal.flush().unwrap();
    drop(journal);
    std::fs::write(world.join("world-metadata.json"), b"operator metadata").unwrap();
    for name in [".solaris-world.lock", "session.lock"] {
        std::fs::write(world.join(name), b"stale lease, not state").unwrap();
    }

    let backup = root.path().join("backup");
    run_backup(&world, &backup).unwrap();
    let manifest = read_manifest(&backup).unwrap();
    assert_eq!(manifest.format, BACKUP_FORMAT);
    assert!(manifest.files.contains_key(NATIVE_DATABASE));
    assert!(
        manifest
            .files
            .keys()
            .any(|name| name.starts_with("journal/batch-"))
    );
    assert!(manifest.files.contains_key("world-metadata.json"));
    assert!(!manifest.files.contains_key(".solaris-world.lock"));
    assert!(!manifest.files.contains_key("session.lock"));
    for (name, hash) in &manifest.files {
        assert_eq!(&digest_file(&backup.join(name)).unwrap(), hash);
    }

    let restored = root.path().join("restored");
    run_restore(&backup, &restored).unwrap();
    for name in manifest.files.keys() {
        assert_eq!(
            std::fs::read(world.join(name)).unwrap(),
            std::fs::read(restored.join(name)).unwrap(),
            "{name} differs after restore",
        );
    }
    assert!(!restored.join(BACKUP_MANIFEST).exists());
    assert!(!restored.join(".solaris-world.lock").exists());
    let recovered = mc_core::Core::open({
        let mut config = mc_core::CoreConfig::new(restored, 42);
        config.vanilla_data_dir =
            Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
        config
    })
    .unwrap();
    recovered.shutdown();
}

#[test]
fn live_core_lease_blocks_backup_and_restore() {
    let root = tempfile::tempdir().unwrap();
    let (_, backup) = fixture_backup(root.path());
    let live = root.path().join("live");
    let core = mc_core::Core::open({
        let mut config = mc_core::CoreConfig::new(live.clone(), 42);
        config.vanilla_data_dir =
            Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
        config
    })
    .unwrap();
    let backup_error = run_backup(&live, &root.path().join("live-backup")).unwrap_err();
    let restore_error = run_restore(&backup, &live).unwrap_err();
    core.shutdown();
    assert!(
        backup_error.to_string().contains("leased"),
        "{backup_error:#}"
    );
    assert!(
        restore_error.to_string().contains("leased"),
        "{restore_error:#}"
    );
    assert!(!root.path().join("live-backup").exists());
}

#[test]
fn empty_target_directory_lease_blocks_restore() {
    let root = tempfile::tempdir().unwrap();
    let (_, backup) = fixture_backup(root.path());
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    // Exactly the directory-inode lease held by Core::open, no v1 lock file.
    let lease = File::open(&target).unwrap();
    lease.try_lock().unwrap();
    let error = run_restore(&backup, &target).unwrap_err();
    assert!(error.to_string().contains("leased"), "{error:#}");
    assert!(std::fs::read_dir(&target).unwrap().next().is_none());
    drop(lease);
    run_restore(&backup, &target).unwrap();
}

#[test]
fn native_redb_writer_blocks_backup_without_a_directory_lease() {
    let root = tempfile::tempdir().unwrap();
    let world = root.path().join("world");
    seed_world(&world);
    let store = NativeStore::open(&world.join(NATIVE_DATABASE), [7; 32]).unwrap();
    let out = root.path().join("backup");
    let error = run_backup(&world, &out).unwrap_err();
    assert!(error.to_string().contains("leased"), "{error:#}");
    assert!(!out.exists());
    drop(store);
    run_backup(&world, &out).unwrap();
}

#[test]
fn backup_holds_the_native_redb_lock_against_new_writers() {
    let root = tempfile::tempdir().unwrap();
    let world = root.path().join("world");
    seed_world(&world);
    let _lease = ExclusiveLease::acquire(&world.join(NATIVE_DATABASE)).unwrap();
    let error = NativeStore::open(&world.join(NATIVE_DATABASE), [7; 32])
        .err()
        .unwrap();
    assert!(error.to_string().contains("already open"), "{error}");
}

#[test]
fn restore_refuses_a_backup_opened_by_a_native_writer() {
    let root = tempfile::tempdir().unwrap();
    let (_, backup) = fixture_backup(root.path());
    let _store = NativeStore::open(&backup.join(NATIVE_DATABASE), [7; 32]).unwrap();
    let target = root.path().join("target");
    let error = run_restore(&backup, &target).unwrap_err();
    assert!(error.to_string().contains("leased"), "{error:#}");
    assert!(!target.exists());
}

#[test]
fn restore_refuses_nonempty_target_without_changing_data() {
    let root = tempfile::tempdir().unwrap();
    let (_, backup) = fixture_backup(root.path());
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("existing"), b"live data").unwrap();
    let error = run_restore(&backup, &target).unwrap_err();
    assert!(error.to_string().contains("not empty"), "{error:#}");
    assert_eq!(
        std::fs::read(target.join("existing")).unwrap(),
        b"live data"
    );
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
}

#[test]
fn every_hash_is_verified_before_any_target_data_is_written() {
    let root = tempfile::tempdir().unwrap();
    let (_, backup) = fixture_backup(root.path());
    // This sorts after journal/ and native/: a one-pass verify-and-copy restore
    // would already have written the database before noticing the mismatch.
    std::fs::write(backup.join("operator/world.json"), b"tampered").unwrap();
    let absent = root.path().join("absent/target");
    let error = run_restore(&backup, &absent).unwrap_err();
    assert!(
        error.to_string().contains("failed verification"),
        "{error:#}"
    );
    assert!(!root.path().join("absent").exists());
    let empty = root.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert!(run_restore(&backup, &empty).is_err());
    assert!(std::fs::read_dir(&empty).unwrap().next().is_none());
}

#[test]
fn restore_rejects_unsafe_manifest_paths_before_creating_target() {
    for name in [
        "../escape",
        "/absolute",
        "native/../../escape",
        "",
        ".",
        "native//state.redb",
        "native/./state.redb",
        "native\\state.redb",
        "manifest.json",
        "session.lock",
        ".solaris-world.lock",
        "nul\0file",
    ] {
        let root = tempfile::tempdir().unwrap();
        let (_, backup) = fixture_backup(root.path());
        change_manifest(&backup, |manifest| {
            manifest["files"][name] = serde_json::json!("00");
        });
        let target = root.path().join("target");
        let error = run_restore(&backup, &target).unwrap_err();
        assert!(
            error.to_string().contains("unsafe path"),
            "{name:?}: {error:#}"
        );
        assert!(!target.exists());
        assert!(!root.path().join("escape").exists());
    }
}

#[test]
fn restore_rejects_missing_or_unhashed_files() {
    let root = tempfile::tempdir().unwrap();
    let (_, backup) = fixture_backup(root.path());
    let target = root.path().join("target");
    std::fs::write(backup.join("extra"), b"unhashed").unwrap();
    assert!(
        run_restore(&backup, &target)
            .unwrap_err()
            .to_string()
            .contains("match manifest")
    );
    std::fs::remove_file(backup.join("extra")).unwrap();
    std::fs::remove_file(backup.join("operator/world.json")).unwrap();
    assert!(
        run_restore(&backup, &target)
            .unwrap_err()
            .to_string()
            .contains("match manifest")
    );
    assert!(!target.exists());
}

#[test]
fn backup_and_restore_refuse_overlapping_roots_before_writes() {
    let root = tempfile::tempdir().unwrap();
    let (world, backup) = fixture_backup(root.path());
    for target in [
        world.clone(),
        world.join("nested"),
        root.path().to_path_buf(),
    ] {
        assert!(
            run_backup(&world, &target)
                .unwrap_err()
                .to_string()
                .contains("overlaps")
        );
    }
    assert!(!world.join("nested").exists());
    for target in [
        backup.clone(),
        backup.join("nested"),
        root.path().to_path_buf(),
    ] {
        assert!(
            run_restore(&backup, &target)
                .unwrap_err()
                .to_string()
                .contains("overlaps")
        );
    }
    assert!(!backup.join("nested").exists());
}

#[test]
fn backup_refuses_nonempty_output_without_replacing_files() {
    let root = tempfile::tempdir().unwrap();
    let world = root.path().join("world");
    seed_world(&world);
    let out = root.path().join("backup");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("existing"), b"keep").unwrap();
    assert!(
        run_backup(&world, &out)
            .unwrap_err()
            .to_string()
            .contains("not empty")
    );
    assert_eq!(std::fs::read(out.join("existing")).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1);
}

#[test]
fn restore_rejects_v1_or_incompatible_schema_without_creating_target() {
    let root = tempfile::tempdir().unwrap();
    let (_, backup) = fixture_backup(root.path());
    let target = root.path().join("target");
    change_manifest(&backup, |manifest| {
        manifest["format"] = serde_json::json!(1)
    });
    assert!(
        run_restore(&backup, &target)
            .unwrap_err()
            .to_string()
            .contains("format")
    );
    change_manifest(&backup, |manifest| {
        manifest["format"] = serde_json::json!(BACKUP_FORMAT);
        manifest["native_prototype_schema"] = serde_json::json!(u32::MAX);
    });
    assert!(
        run_restore(&backup, &target)
            .unwrap_err()
            .to_string()
            .contains("schema")
    );
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
fn backup_and_restore_reject_file_and_directory_symlinks() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let (world, backup) = fixture_backup(root.path());
    let out = root.path().join("other-backup");
    symlink(world.join("operator/world.json"), world.join("linked-file")).unwrap();
    assert!(
        run_backup(&world, &out)
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
    std::fs::remove_file(world.join("linked-file")).unwrap();
    symlink(world.join("operator"), world.join("linked-dir")).unwrap();
    assert!(
        run_backup(&world, &out)
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
    assert!(!out.exists());

    let target = root.path().join("target");
    std::fs::remove_file(backup.join("operator/world.json")).unwrap();
    symlink(
        world.join("operator/world.json"),
        backup.join("operator/world.json"),
    )
    .unwrap();
    assert!(
        run_restore(&backup, &target)
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
    std::fs::remove_file(backup.join("operator/world.json")).unwrap();
    std::fs::remove_dir(backup.join("operator")).unwrap();
    symlink(world.join("operator"), backup.join("operator")).unwrap();
    assert!(
        run_restore(&backup, &target)
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
fn restore_rejects_symlink_manifest_and_resolves_alias_overlap() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let (world, backup) = fixture_backup(root.path());
    let alias = root.path().join("alias");
    symlink(&backup, &alias).unwrap();
    assert!(
        run_restore(&backup, &alias.join("nested"))
            .unwrap_err()
            .to_string()
            .contains("overlaps")
    );
    assert!(!backup.join("nested").exists());
    let manifest = backup.join(BACKUP_MANIFEST);
    let outside = world.join("manifest-copy.json");
    std::fs::copy(&manifest, &outside).unwrap();
    std::fs::remove_file(&manifest).unwrap();
    symlink(outside, manifest).unwrap();
    let target = root.path().join("target");
    assert!(
        run_restore(&backup, &target)
            .unwrap_err()
            .to_string()
            .contains("regular file")
    );
    assert!(!target.exists());
}
