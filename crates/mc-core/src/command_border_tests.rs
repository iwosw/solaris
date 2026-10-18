use super::*;

#[test]
fn center_relative_set_add_get_and_timed_rejection_match_old() {
    let mut border = Border::default();
    execute(
        &mut border,
        parse(&["center", "~2", "~-3"]).unwrap(),
        [10.0, 64.0, 20.0],
    );
    assert_eq!((border.center_x, border.center_z), (12.0, 17.0));
    execute(&mut border, parse(&["set", "100", "0"]).unwrap(), [0.0; 3]);
    execute(&mut border, parse(&["add", "-200"]).unwrap(), [0.0; 3]);
    assert_eq!(border.size, 1.0);
    assert!(execute(&mut border, BorderCommand::Get, [0.0; 3]).contains("1 blocks"));
    assert!(parse(&["set", "20", "1"]).is_err());
    assert!(parse(&["set", "NaN"]).is_err());
    assert!(parse(&["center", "^", "^"]).is_err());
}

#[test]
fn checkpoint_and_fsynced_tail_keep_exact_border_and_reject_invalid_state() {
    use crate::journal::*;
    let dir = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
    let base = Border {
        center_x: 1.25,
        center_z: -4.5,
        size: 128.0,
    };
    crate::native_checkpoint::tests::write_manifest(
        &config,
        &CheckpointManifest {
            tick: 1,
            border: base,
            ..Default::default()
        },
    )
    .unwrap();
    let next = Border { size: 64.0, ..base };
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    writer
        .append(2, 2, &[JournalEntry::Border { value: next }])
        .unwrap();
    writer.flush().unwrap();
    let manifest = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
    let replay = replay_batches(
        Replay {
            border: manifest.border,
            tick: manifest.tick,
            ..Default::default()
        },
        &read_complete_batches_after(&dir.path().join("journal"), 1).unwrap(),
    );
    assert_eq!(replay.border, next);
    assert!(
        Border { size: 0.0, ..base }
            .validate()
            .unwrap_err()
            .to_string()
            .contains("worldborder.size")
    );
}
