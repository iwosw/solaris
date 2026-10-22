use crate::boundary::{ChunkPos, PlayerInput, TickOutput};
use crate::command_console::ConsoleCommand;
use crate::config::CoreConfig;
use crate::core::Core;
use crate::hot_backup::copy_world;

fn batch(dir: &std::path::Path, lo: u64, hi: u64) {
    std::fs::write(
        dir.join(format!("batch-{lo:020}-{hi:020}.bin")),
        format!("{lo}-{hi}"),
    )
    .unwrap();
}

fn fake_world() -> tempfile::TempDir {
    let world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(world.path().join("native")).unwrap();
    std::fs::create_dir_all(world.path().join("journal")).unwrap();
    std::fs::write(world.path().join("native/state.redb"), b"database").unwrap();
    world
}

#[test]
fn copy_takes_database_and_only_the_wal_between_checkpoint_and_through() {
    let world = fake_world();
    let journal = world.path().join("journal");
    // Pruned/older, covered, covered, and appended after the requested tick.
    batch(&journal, 1, 10);
    batch(&journal, 11, 11);
    batch(&journal, 12, 12);
    batch(&journal, 13, 13);
    std::fs::write(
        journal.join("batch-00000000000000000014-00000000000000000014.bin.tmp"),
        b"x",
    )
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let backup = copy_world(world.path(), out.path(), 10, 12).unwrap();
    assert_eq!((backup.checkpoint_tick, backup.wal_tick), (10, 12));
    let mut names: Vec<_> = backup.files.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "journal/batch-00000000000000000011-00000000000000000011.bin",
            "journal/batch-00000000000000000012-00000000000000000012.bin",
            "native/state.redb",
        ]
    );
    assert_eq!(
        std::fs::read(out.path().join("native/state.redb")).unwrap(),
        b"database"
    );
    use sha2::{Digest, Sha256};
    assert_eq!(
        backup.files["native/state.redb"],
        format!("{:x}", Sha256::digest(b"database"))
    );
}

#[test]
fn copy_refuses_a_wal_hole_and_a_straddling_batch() {
    let world = fake_world();
    let journal = world.path().join("journal");
    batch(&journal, 11, 11);
    batch(&journal, 13, 13);
    let out = tempfile::tempdir().unwrap();
    let error = copy_world(world.path(), out.path(), 10, 13).unwrap_err();
    assert!(error.to_string().contains("expected 12"), "{error}");

    let world = fake_world();
    batch(&world.path().join("journal"), 8, 12);
    let out = tempfile::tempdir().unwrap();
    let error = copy_world(world.path(), out.path(), 10, 12).unwrap_err();
    assert!(error.to_string().contains("expected 11"), "{error}");

    let world = fake_world();
    batch(&world.path().join("journal"), 11, 11);
    let out = tempfile::tempdir().unwrap();
    let error = copy_world(world.path(), out.path(), 10, 12).unwrap_err();
    assert!(error.to_string().contains("journal hole"), "{error}");
}

fn config(dir: &std::path::Path) -> CoreConfig {
    let mut config = CoreConfig::new(dir.to_path_buf(), 42);
    config.view_distance = 1;
    config.simulation_distance = 0;
    config.checkpoint_every_ticks = u64::MAX;
    config
}

fn next_output(rx: &mut tokio::sync::broadcast::Receiver<TickOutput>, at_least: u64) -> TickOutput {
    loop {
        match rx.blocking_recv() {
            Ok(output) if output.tick >= at_least => return output,
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => panic!("tick thread died"),
        }
    }
}

fn next_resident(
    rx: &mut tokio::sync::broadcast::Receiver<TickOutput>,
    pos: ChunkPos,
) -> TickOutput {
    loop {
        let output = next_output(rx, 1);
        if output.snapshot.chunks.iter().any(|chunk| chunk.pos == pos) {
            return output;
        }
    }
}

fn block(output: &TickOutput, [x, y, z]: [i32; 3]) -> u32 {
    let pos = ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
    };
    let chunk = output
        .snapshot
        .chunks
        .iter()
        .find(|chunk| chunk.pos == pos)
        .expect("spawn chunk resident");
    chunk
        .chunk
        .get_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8)
        .expect("block in chunk")
        .0
}

fn set_block(
    core: &Core,
    rx: &mut tokio::sync::broadcast::Receiver<TickOutput>,
    at: [i32; 3],
    name: &str,
) -> TickOutput {
    let (command, completed) = ConsoleCommand::new(format!(
        "setblock {} {} {} minecraft:{name}",
        at[0], at[1], at[2]
    ));
    assert!(core.submit(PlayerInput::ConsoleCommand(command)));
    let tick = completed.blocking_recv().expect("command executed");
    next_output(rx, tick)
}

#[test]
fn backup_of_a_running_core_restores_the_state_at_its_tick() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let copy = root.path().join("copy");
    let core = Core::open(config(&live)).unwrap();
    let mut rx = core.subscribe();
    let [sx, sy, sz] = core.world_spawn();
    let (before, after) = ([sx, sy + 8, sz], [sx + 1, sy + 8, sz]);
    // No player is online: ticket the chunk under test and wait for the
    // loader to publish it.
    let chunk = ChunkPos {
        x: sx.div_euclid(16),
        z: sz.div_euclid(16),
    };
    core.add_ticket(crate::core::RemoteTicket {
        owner: "hot-backup-test".into(),
        chunk,
        radius: 0,
        reason: "hot backup test".into(),
        budget_chunks: 1,
    })
    .unwrap();
    let initial = next_resident(&mut rx, chunk);
    let (before_original, after_original) = (block(&initial, before), block(&initial, after));

    let output = set_block(&core, &mut rx, before, "gold_block");
    let before_changed = block(&output, before);
    assert_ne!(before_changed, before_original);

    std::fs::create_dir(&copy).unwrap();
    let backup = core.backup(&copy).unwrap();
    assert!(backup.wal_tick >= backup.checkpoint_tick);
    assert!(backup.files.contains_key("native/state.redb"));

    // Changes after the backup must not appear in it.
    let output = set_block(&core, &mut rx, after, "diamond_block");
    assert_ne!(block(&output, after), after_original);
    core.shutdown();

    let restored = Core::open(config(&copy)).unwrap();
    let mut rx = restored.subscribe();
    let output = next_resident(&mut rx, chunk);
    assert!(output.tick >= backup.wal_tick);
    assert_eq!(block(&output, before), before_changed);
    assert_eq!(block(&output, after), after_original);
    restored.shutdown();
}
