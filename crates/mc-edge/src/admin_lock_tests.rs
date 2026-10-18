use super::*;

fn fixture() -> (Admin, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (Admin::open(dir.path()).unwrap(), dir)
}

#[test]
fn cached_permissions_follow_all_sessions_and_loaded_levels() {
    let dir = tempfile::tempdir().unwrap();
    let uuid = Uuid::new_v4();
    std::fs::write(
        dir.path().join("ops.json"),
        serde_json::to_vec(&vec![OpEntry {
            uuid,
            name: "Alex".into(),
            level: 2,
            bypasses_player_limit: false,
        }])
        .unwrap(),
    )
    .unwrap();
    let admin = Admin::open(dir.path()).unwrap();
    for id in [ConnectionId(1), ConnectionId(2)] {
        admin.register(id, uuid, "Alex".into(), "127.0.0.1".parse().unwrap());
        assert_eq!(admin.session_level(id), 2);
    }
    admin.set_op(uuid, "Alex", true).unwrap();
    assert_eq!(admin.session_level(ConnectionId(1)), 2);
    admin.set_op(uuid, "Alex", false).unwrap();
    for id in [ConnectionId(1), ConnectionId(2)] {
        assert_eq!(admin.session_level(id), 0);
    }
    admin.set_op(uuid, "Alex", true).unwrap();
    for id in [ConnectionId(1), ConnectionId(2)] {
        assert_eq!(admin.session_level(id), 4);
    }
    admin.remove(ConnectionId(1));
    assert_eq!(admin.session_level(ConnectionId(1)), 0);
}

#[test]
fn failed_persistence_does_not_publish_admin_changes() {
    let (admin, dir) = fixture();
    let uuid = Uuid::new_v4();
    let id = ConnectionId(1);
    let ip = "127.0.0.1".parse().unwrap();
    admin.register(id, uuid, "Alex".into(), ip);
    for filename in [
        "ops.json.tmp",
        "banned-players.json.tmp",
        "banned-ips.json.tmp",
    ] {
        std::fs::create_dir(dir.path().join(filename)).unwrap();
    }
    assert!(admin.set_op(uuid, "Alex", true).is_err());
    assert_eq!(admin.session_level(id), 0);
    assert_eq!(admin.level(uuid), 0);
    assert!(
        admin
            .ban_player(uuid, "Alex", "Console", "test", true)
            .is_err()
    );
    assert!(admin.ban_ip("127.0.0.1", "Console", "test", true).is_err());
    assert!(admin.admission(uuid, Some(ip), false).is_none());
}

#[test]
fn concurrent_persistence_keeps_every_mutation_in_memory_and_on_disk() {
    let (admin, dir) = fixture();
    let ids: Vec<_> = (0..8).map(|_| Uuid::new_v4()).collect();
    let barrier = std::sync::Barrier::new(ids.len());
    std::thread::scope(|scope| {
        for &uuid in &ids {
            let admin = &admin;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                admin.set_op(uuid, "Operator", true).unwrap();
                admin
                    .ban_player(uuid, "Operator", "Console", "test", true)
                    .unwrap();
            });
        }
    });
    let reopened = Admin::open(dir.path()).unwrap();
    for uuid in ids {
        assert_eq!(admin.level(uuid), 4);
        assert_eq!(reopened.level(uuid), 4);
        assert!(admin.admission(uuid, None, false).is_some());
        assert!(reopened.admission(uuid, None, false).is_some());
    }
}

#[test]
fn registration_and_deop_publish_consistent_cached_permission() {
    let (admin, _dir) = fixture();
    let uuid = Uuid::new_v4();
    let id = ConnectionId(1);
    admin.set_op(uuid, "Alex", true).unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            barrier.wait();
            admin.register(id, uuid, "Alex".into(), "127.0.0.1".parse().unwrap());
        });
        scope.spawn(|| {
            barrier.wait();
            admin.set_op(uuid, "Alex", false).unwrap();
        });
    });
    assert_eq!(admin.level(uuid), 0);
    assert_eq!(admin.session_level(id), 0);
}
