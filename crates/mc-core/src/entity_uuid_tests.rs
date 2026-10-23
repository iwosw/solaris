use crate::boundary::EntityId;
use crate::config::CoreConfig;
use crate::core::Core;
use crate::entity_uuid::{SALT_LEN, derive};

#[test]
fn derived_uuids_are_stable_v4_and_distinct() {
    let salt = [7u8; SALT_LEN];
    let base = derive(&salt, 7, EntityId(9));
    assert_eq!(base, derive(&salt, 7, EntityId(9)));
    assert_eq!(base.get_version_num(), 4);
    assert_eq!(base.get_variant(), uuid::Variant::RFC4122);
    let mut seen = std::collections::HashSet::new();
    for salt in [[1u8; SALT_LEN], [2u8; SALT_LEN]] {
        for tick in 0..50 {
            for id in 1..50 {
                assert!(seen.insert(derive(&salt, tick, EntityId(id))));
            }
        }
    }
}

/// The world seed is not an input and a different salt changes every id, so
/// (id, tick) pairs a client sees reveal nothing reproducible from the seed.
#[test]
fn uuid_depends_on_salt_alone() {
    let zero = derive(&[0; SALT_LEN], 100, EntityId(5));
    let other = derive(&[1; SALT_LEN], 100, EntityId(5));
    assert_ne!(zero, other);
    // Same salt, same inputs: same uuid (what replay relies on).
    assert_eq!(other, derive(&[1; SALT_LEN], 100, EntityId(5)));
}

/// New world mints a salt, persists it, and a reopen (and a world-dir copy)
/// installs the same one.
#[test]
fn salt_is_minted_once_and_survives_restart() {
    let dir = std::env::temp_dir().join(format!("mc-core-entity-salt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = CoreConfig::new(dir.clone(), 42);
    let salt_of = |config: &CoreConfig| {
        let state = crate::input_log_tests::open_world_state(config);
        let salt = state.entity_salt;
        for child in state.other_dimensions.values() {
            assert_eq!(child.entity_salt, salt, "every dimension shares the salt");
        }
        salt
    };
    let core = Core::open(config.clone()).expect("create");
    core.shutdown();
    drop(core);
    let first = salt_of(&config);
    assert_ne!(first, [0; SALT_LEN]);
    let core = Core::open(config.clone()).expect("reopen");
    core.shutdown();
    drop(core);
    assert_eq!(salt_of(&config), first);
    let copy =
        std::env::temp_dir().join(format!("mc-core-entity-salt-copy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&copy);
    crate::input_log_tests::copy_dir(&dir, &copy);
    assert_eq!(salt_of(&CoreConfig::new(copy.clone(), 42)), first);
    // A second world gets its own salt.
    let other = std::env::temp_dir().join(format!("mc-core-entity-salt-b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&other);
    let core = Core::open(CoreConfig::new(other.clone(), 42)).expect("create other");
    core.shutdown();
    drop(core);
    assert_ne!(salt_of(&CoreConfig::new(other.clone(), 42)), first);
    for path in [dir, copy, other] {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// A stored salt that does not parse fails closed instead of being silently
/// replaced (which would change the identity of later spawns).
#[test]
fn corrupt_stored_salt_fails_closed() {
    let dir = std::env::temp_dir().join(format!("mc-core-entity-salt-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = CoreConfig::new(dir.clone(), 42);
    let core = Core::open(config.clone()).expect("create");
    core.shutdown();
    drop(core);
    let mut state = crate::input_log_tests::open_world_state(&config);
    crate::terrain::stage_data_field(
        &mut state.native_globals,
        crate::entity_uuid::SALT_FIELD,
        "not-a-uuid",
    )
    .expect("stage corrupt salt");
    let error = crate::entity_uuid::install_salt(&mut state).expect_err("corrupt salt");
    assert!(matches!(error, crate::CoreError::Storage(_)), "{error:?}");
    let _ = std::fs::remove_dir_all(dir);
}
