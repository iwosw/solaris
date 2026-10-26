use super::*;

#[test]
fn center_relative_set_add_get_match_old() {
    let mut border = Border::default();
    execute(
        &mut border,
        parse(&["center", "~2", "~-3"]).unwrap(),
        [10.0, 64.0, 20.0],
    );
    assert_eq!((border.center_x, border.center_z), (12.0, 17.0));
    execute(&mut border, parse(&["set", "100"]).unwrap(), [0.0; 3]);
    execute(&mut border, parse(&["add", "-200"]).unwrap(), [0.0; 3]);
    assert_eq!(border.size, 1.0);
    assert!(execute(&mut border, BorderCommand::Get, [0.0; 3]).contains("1 blocks"));
    assert!(parse(&["set", "NaN"]).is_err());
    assert!(parse(&["center", "^", "^"]).is_err());
}

#[test]
fn timed_set_interpolates_linearly_and_settles_exact() {
    // BORDER-LERP: `worldborder set 50 200t` from 100 reaches ~75 after
    // 100 ticks and exactly 50 at 200 (vanilla MovingBorderExtent linear
    // progress over game time).
    let mut border = Border::default();
    execute(&mut border, parse(&["set", "100"]).unwrap(), [0.0; 3]);
    execute(&mut border, parse(&["set", "50", "200t"]).unwrap(), [0.0; 3]);
    assert_eq!((border.size, border.lerp_target, border.lerp_remaining), (100.0, 50.0, 200));
    for _ in 0..100 {
        border.advance_tick();
    }
    assert!((border.size - 75.0).abs() < 1e-6, "mid-lerp size {}", border.size);
    assert_eq!(border.lerp_remaining, 100);
    for _ in 0..100 {
        border.advance_tick();
    }
    assert_eq!((border.size, border.lerp_target, border.lerp_remaining), (50.0, 50.0, 0));
    // Time units: bare and t are ticks, s is 20x, d is 24000x.
    execute(&mut border, parse(&["set", "60", "10s"]).unwrap(), [0.0; 3]);
    assert_eq!(border.lerp_remaining, 200);
    execute(&mut border, parse(&["set", "60", "1d"]).unwrap(), [0.0; 3]);
    assert_eq!(border.lerp_remaining, 24_000);
    // `add` targets current size + delta and stacks remaining time.
    execute(&mut border, parse(&["set", "100"]).unwrap(), [0.0; 3]);
    execute(&mut border, parse(&["add", "-50", "100t"]).unwrap(), [0.0; 3]);
    assert_eq!((border.lerp_target, border.lerp_remaining), (50.0, 100));
}

#[test]
fn damage_and_warning_options_mutate_named_fields() {
    // BORDER-OPTIONS: the four option subcommands mutate their named
    // fields (vanilla setDamageBuffer/setDamagePerBlock/setWarningTime/
    // setWarningBlocks), not size or center.
    let mut border = Border::default();
    execute(&mut border, parse(&["damage", "buffer", "0"]).unwrap(), [0.0; 3]);
    execute(&mut border, parse(&["damage", "amount", "1"]).unwrap(), [0.0; 3]);
    execute(&mut border, parse(&["warning", "distance", "7"]).unwrap(), [0.0; 3]);
    execute(&mut border, parse(&["warning", "time", "3"]).unwrap(), [0.0; 3]);
    assert_eq!(
        (border.safe_zone, border.damage_per_block, border.warning_blocks, border.warning_time),
        (0.0, 1.0, 7, 3)
    );
    assert_eq!((border.size, border.center_x, border.center_z), (MAX_SIZE, 0.0, 0.0));
    assert!(parse(&["damage", "amount", "6"]).is_err());
    assert!(parse(&["damage", "buffer", "-1"]).is_err());
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
        lerp_target: 64.0,
        lerp_remaining: 100,
        ..Border::default()
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
    let next = Border { size: 64.0, lerp_target: 64.0, lerp_remaining: 0, ..base };
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
