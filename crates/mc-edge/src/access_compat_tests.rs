use super::*;

#[test]
fn serverops_access_operator_bypasses_whitelist_but_not_bans() {
    let root = tempfile::tempdir().unwrap();
    let uuid = Uuid::new_v4();
    std::fs::write(root.path().join("ops.json"), serde_json::to_vec(&serde_json::json!([
        {"uuid": uuid, "name":"Operator", "level": 4, "bypassesPlayerLimit":true}
    ])).unwrap()).unwrap();
    let admin = Admin::open(root.path()).unwrap();
    admin.set_whitelist(true).unwrap();
    assert!(admin.admission(uuid, None, true).is_none());
    admin.ban_player(uuid, "Operator", "Console", "Banned", true).unwrap();
    assert_eq!(Admin::open(root.path()).unwrap().admission(uuid, None, false).unwrap()["translate"], "multiplayer.disconnect.banned.reason");
}

#[test]
fn serverops_access_corrupt_enforcement_toggle_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("whitelist.enabled"), "tru").unwrap();
    assert!(Admin::open(root.path()).is_err());
}

#[test]
fn serverops_access_imports_vanilla_bans_whitelist_and_preserves_files() {
    let root = tempfile::tempdir().unwrap();
    let listed = Uuid::new_v4();
    let banned = Uuid::new_v4();
    std::fs::write(root.path().join("whitelist.json"), serde_json::to_vec(&serde_json::json!([{ "uuid":listed,"name":"Listed"}])).unwrap()).unwrap();
    std::fs::write(root.path().join("banned-players.json"), serde_json::to_vec(&serde_json::json!([{ "uuid":banned,"name":"Banned","created":"2026-01-01 00:00:00 +0000","source":"Server","expires":"forever","reason":"No"}])).unwrap()).unwrap();
    std::fs::write(root.path().join("banned-ips.json"), r#"[{"ip":"192.0.2.1","created":"2026-01-01 00:00:00 +0000","source":"Server","expires":"forever","reason":"No"}]"#).unwrap();
    let admin = Admin::open(root.path()).unwrap();
    admin.set_whitelist(true).unwrap();
    assert!(admin.admission(listed, None, false).is_none());
    assert!(admin.admission(banned, None, false).is_some());
    assert!(admin.admission(listed, Some("192.0.2.1".parse().unwrap()), false).is_some());
    assert_eq!(Admin::open(root.path()).unwrap().whitelist_names(), ["Listed"]);
}
