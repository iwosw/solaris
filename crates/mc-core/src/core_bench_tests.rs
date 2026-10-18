use super::wal_metrics;

#[test]
fn wal_metrics_counts_only_closed_batches_inside_the_measured_interval() {
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    std::fs::create_dir(&journal).unwrap();
    for (lo, hi, bytes) in [(9, 10, 50), (11, 12, 100), (13, 15, 200), (16, 16, 75)] {
        std::fs::write(
            journal.join(format!("batch-{lo:020}-{hi:020}.bin")),
            vec![0; bytes],
        )
        .unwrap();
    }
    std::fs::write(journal.join("pending.tmp"), [0; 99]).unwrap();
    let metrics = wal_metrics(root.path(), 10, 15);
    assert_eq!(metrics["bytes"], 300);
    assert_eq!(metrics["ticks"], 5);
    assert_eq!(metrics["bytes_per_tick"], 60.0);
    assert_eq!(metrics["batches"], 2);
    assert_eq!(metrics["first_tick"], 11);
    assert_eq!(metrics["last_tick"], 15);
}

#[test]
#[should_panic(expected = "no complete WAL batches inside measured ticks")]
fn wal_metrics_refuses_an_empty_measurement() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("journal")).unwrap();
    wal_metrics(root.path(), 1, 10);
}

#[test]
fn moving_fixture_acknowledges_join_before_rotation_commits() {
    use mc_core::boundary::{ConnectionId, PlayerInput};
    use mc_core::{Core, CoreConfig};

    let root = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(root.path().to_owned(), 42);
    config.view_distance = 1;
    config.simulation_distance = 1;
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    let core = Core::open(config).unwrap();
    let mut receiver = core.subscribe();
    let connection = ConnectionId(1);
    let uuid = uuid::Uuid::from_u128(1);
    core.submit(PlayerInput::Join {
        connection,
        uuid,
        name: "moving-fixture".into(),
        permission_level: 0,
    });
    let mut rotated = false;
    for _ in 0..16 {
        let output = receiver.blocking_recv().unwrap();
        if output
            .snapshot
            .entities
            .iter()
            .any(|view| view.uuid == uuid && view.yaw == 90.0)
        {
            rotated = true;
            break;
        }
        super::acknowledge_teleports(&core);
        core.submit(PlayerInput::MoveRotation {
            connection,
            yaw: 90.0,
            pitch: 0.0,
            on_ground: true,
        });
    }
    core.shutdown();
    assert!(
        rotated,
        "the real join receipt must release the movement fence"
    );
}
