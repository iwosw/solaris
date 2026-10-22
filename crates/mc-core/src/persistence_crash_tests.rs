//! Real process death at unpublished WAL and native checkpoint boundaries.
//! The native manifest and checkpoint data share one redb transaction: there is
//! no separate manifest rename to inject between them (ADR 0013).
use super::*;
use crate::dimensions::Dimension;
use crate::native_checkpoint::tests::read_manifest;
use mc_data::{Identifier, ItemStack};
use std::io::{BufRead, Read, Write};
use std::os::unix::process::ExitStatusExt;

const CHILD: &str = "core::persistence_crash_tests::crash_child";
const CELL: (i32, i32, i32) = (2, 100, 1);
const CONNECTION: ConnectionId = ConnectionId(1);
const UUID: uuid::Uuid = uuid::Uuid::from_u128(0xC0AC_0001);

fn config(directory: &std::path::Path) -> CoreConfig {
    let mut config = CoreConfig::new(directory.to_path_buf(), 42);
    config.view_distance = 0;
    config.simulation_distance = 0;
    config
}

fn recovered_world(config: &CoreConfig) -> TickState {
    let mut state = recover_dimension_state(config, Dimension::Overworld).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        state.other_dimensions.insert(
            dimension,
            Box::new(recover_dimension_state(config, dimension).unwrap()),
        );
    }
    // Tick admission normally loads asynchronously. Mutate only a resident
    // chunk, and await that exact condition rather than racing generation.
    state.world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    state.world.fold_overlay();
    state
}

fn join(state: &mut TickState) {
    crate::tick::run_tick(
        state,
        &[PlayerInput::Join {
            connection: CONNECTION,
            uuid: UUID,
            name: "CrashCustody".into(),
            permission_level: 0,
        }],
    );
}

fn block(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
        .0
}

fn item(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&Identifier::parse(name).unwrap())
        .unwrap()
}

fn block_name(tick: u64) -> &'static str {
    match tick {
        1 => "minecraft:stone",
        2 => "minecraft:dirt",
        3 => "minecraft:gold_block",
        _ => panic!("unexpected fixture tick {tick}"),
    }
}

// Every version changes all three owners in the same tick. A mixed checkpoint
// or a partly replayed WAL cannot satisfy this oracle.
fn mutate(state: &mut TickState) -> Vec<JournalEntry> {
    let tick = state.tick;
    let actor = state.connections.entity_of(CONNECTION).unwrap();
    let mut inventory = vec![ItemStack::EMPTY; 41];
    inventory[0] = ItemStack::new(item(state, "minecraft:apple"), tick as i32);
    state.players.get_mut(&actor).unwrap().inventory.slots = inventory;
    let mut slots = vec![ItemStack::EMPTY; 27];
    slots[0] = ItemStack::new(item(state, "minecraft:gold_ingot"), 2 * tick as i32);
    state.block_entities.insert(
        CELL,
        crate::tick::BlockEntityState {
            slots: slots.clone(),
            furnace: None,
        },
    );
    let changed = block(state, block_name(tick));
    let chest = block(state, "minecraft:chest");
    state.world.set_block(1, 100, 1, changed);
    state.world.set_block(CELL.0, CELL.1, CELL.2, chest);
    state.world.fold_overlay();
    let player = player_record_for(state, actor, tick).unwrap();
    vec![
        JournalEntry::Block {
            pos: [1, 100, 1],
            state: changed,
        },
        JournalEntry::Block {
            pos: [CELL.0, CELL.1, CELL.2],
            state: chest,
        },
        player_journal_entry(UUID.to_string(), &player, None, tick),
        JournalEntry::BlockEntity {
            pos: [CELL.0, CELL.1, CELL.2],
            slots,
            furnace: None,
            tick,
        },
    ]
}

fn checkpoint(state: &mut TickState) -> Box<CheckpointJob> {
    let (tx, rx) = crate::core::journal_work_channel();
    let (done, _) = std::sync::mpsc::channel();
    assert!(start_checkpoint(state, &tx, &done, state.tick));
    let JournalWork::Checkpoint(job) = rx.recv().unwrap() else {
        panic!("expected checkpoint");
    };
    job
}

fn baseline(directory: &std::path::Path) {
    let config = config(directory);
    let mut state = recovered_world(&config);
    join(&mut state);
    assert_eq!(state.tick, 1);
    let entries = mutate(&mut state);
    let mut writer = JournalWriter::open(directory, 1).unwrap();
    writer.append(1, 1, &entries).unwrap();
    let job = checkpoint(&mut state);
    let mut manifest = read_manifest(&config).unwrap();
    write_checkpoint(directory, &job, &state.plugins, &mut writer, &mut manifest).unwrap();
    assert_eq!(manifest.tick, 1);
}

fn at_boundary() -> ! {
    println!("CRASH_BOUNDARY_READY");
    std::io::stdout().flush().unwrap();
    // The parent owns stdin and kills us only after the exact readiness event.
    // EOF or an unexpected byte is a harness failure, never a graceful exit.
    let mut byte = [0];
    std::io::stdin()
        .read_exact(&mut byte)
        .expect("parent must SIGKILL at boundary");
    panic!("parent sent input instead of SIGKILL");
}

#[test]
#[ignore = "re-executed only by SIGKILL boundary parents"]
fn crash_child() {
    let directory =
        std::path::PathBuf::from(std::env::var_os("SOLARIS_PERSIST_CRASH_DIR").unwrap());
    let boundary = std::env::var("SOLARIS_PERSIST_CRASH_BOUNDARY").unwrap();
    let config = config(&directory);
    let mut state = recovered_world(&config);
    join(&mut state);
    let entries = mutate(&mut state);
    let tick = state.tick;
    let mut writer = JournalWriter::open(&directory, 1).unwrap();
    if boundary == "wal_partial" || boundary == "wal_before_fsync" {
        let partial = boundary == "wal_partial";
        *crate::journal::WAL_WRITE_OBSERVER.lock() = Some((
            directory.join("journal"),
            Box::new(move |file, frame| {
                let end = if partial {
                    frame.len() / 2
                } else {
                    frame.len()
                };
                file.write_all(&frame[..end]).unwrap();
                at_boundary();
            }),
        ));
    }
    writer.append(tick, tick, &entries).unwrap();
    if boundary == "wal_before_ack" {
        at_boundary();
    }
    let job = checkpoint(&mut state);
    let mut manifest = read_manifest(&config).unwrap();
    let expected = match boundary.as_str() {
        "checkpoint_regions" => {
            *FAIL_CHECKPOINT_AFTER_REGIONS.lock().unwrap() = Some(directory.clone());
            "storage: injected checkpoint failure"
        }
        "checkpoint_committed" => {
            *FAIL_CHECKPOINT_AFTER_NATIVE_COMMIT.lock().unwrap() = Some(directory.clone());
            "storage: injected crash after native commit before acknowledgement"
        }
        _ => panic!("WAL observer did not stop child: {boundary}"),
    };
    let error =
        write_checkpoint(&directory, &job, &state.plugins, &mut writer, &mut manifest).unwrap_err();
    assert_eq!(error.to_string(), expected);
    // Keep every owner/store alive: the parent kills the actual writer process,
    // not a process that already closed or gracefully checkpointed its world.
    at_boundary();
}

struct CrashChild(std::process::Child);
impl Drop for CrashChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn kill_at(directory: &std::path::Path, boundary: &str) {
    let mut child = CrashChild(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("SOLARIS_PERSIST_CRASH_DIR", directory)
            .env("SOLARIS_PERSIST_CRASH_BOUNDARY", boundary)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut output = std::io::BufReader::new(child.0.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "child exited before {boundary}"
        );
        if line.trim() == "CRASH_BOUNDARY_READY" {
            break;
        }
    }
    child.0.kill().unwrap();
    let status = child.0.wait().unwrap();
    assert_eq!(
        status.signal(),
        Some(9),
        "must be real SIGKILL at {boundary}"
    );
}

fn assert_reopens(directory: &std::path::Path, committed_tick: u64, checkpoint_tick: u64) {
    let config = config(directory);
    for _ in 0..2 {
        // Open a writable native store first: redb repairs an uncleanly killed
        // writer here. Offline-only readers deliberately cannot perform repair.
        let mut state = recovered_world(&config);
        assert_eq!(state.tick, committed_tick);
        assert!(
            state
                .other_dimensions
                .values()
                .all(|child| child.tick == committed_tick)
        );
        assert_eq!(read_manifest(&config).unwrap().tick, checkpoint_tick);
        join(&mut state); // Normal player/container hydration, no persistence.
        let actor = state.connections.entity_of(CONNECTION).unwrap();
        let mut inventory = vec![ItemStack::EMPTY; 41];
        inventory[0] = ItemStack::new(item(&state, "minecraft:apple"), committed_tick as i32);
        assert_eq!(state.players[&actor].inventory.slots, inventory);
        let mut slots = vec![ItemStack::EMPTY; 27];
        slots[0] = ItemStack::new(
            item(&state, "minecraft:gold_ingot"),
            2 * committed_tick as i32,
        );
        assert_eq!(state.block_entities[&CELL].slots, slots);
        assert_eq!(
            state.world.block_at(1, 100, 1),
            Some(block(&state, block_name(committed_tick)))
        );
        assert_eq!(
            state.world.block_at(CELL.0, CELL.1, CELL.2),
            Some(block(&state, "minecraft:chest"))
        );
    }
}

#[test]
fn sigkill_during_wal_write_ignores_only_unpublished_tail() {
    for boundary in ["wal_partial", "wal_before_fsync"] {
        let directory = tempfile::tempdir().unwrap();
        baseline(directory.path());
        kill_at(directory.path(), boundary);
        let tmp = directory
            .path()
            .join("journal/batch-00000000000000000002-00000000000000000002.bin.tmp");
        let bytes = std::fs::read(tmp).unwrap();
        let payload_len = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
        assert!(bytes.len() > 32);
        if boundary == "wal_partial" {
            assert!(bytes.len() < 32 + payload_len, "the frame really is torn");
        } else {
            assert_eq!(bytes.len(), 32 + payload_len, "complete but unpublished");
        }
        assert_reopens(directory.path(), 1, 1);
    }
}

#[test]
fn sigkill_after_durable_wal_before_ack_keeps_entire_tick() {
    let directory = tempfile::tempdir().unwrap();
    baseline(directory.path());
    kill_at(directory.path(), "wal_before_ack");
    assert_reopens(directory.path(), 2, 1);
}

#[test]
fn sigkill_checkpoint_before_commit_and_before_prune_keeps_entire_tick() {
    for (boundary, checkpoint_tick) in [("checkpoint_regions", 1), ("checkpoint_committed", 2)] {
        let directory = tempfile::tempdir().unwrap();
        baseline(directory.path());
        kill_at(directory.path(), boundary);
        assert!(
            directory
                .path()
                .join("journal/batch-00000000000000000002-00000000000000000002.bin")
                .exists()
        );
        assert_reopens(directory.path(), 2, checkpoint_tick);
    }
}

#[test]
fn sigkill_twice_replays_previous_tail_then_commits_next_tick() {
    let directory = tempfile::tempdir().unwrap();
    baseline(directory.path());
    kill_at(directory.path(), "checkpoint_regions");
    assert_reopens(directory.path(), 2, 1);
    kill_at(directory.path(), "checkpoint_committed");
    assert_reopens(directory.path(), 3, 3);
}

#[test]
fn published_wal_damage_fails_closed_on_every_reopen() {
    for truncate in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        baseline(directory.path());
        kill_at(directory.path(), "wal_before_ack");
        let path = directory
            .path()
            .join("journal/batch-00000000000000000002-00000000000000000002.bin");
        if truncate {
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_len(40).unwrap();
        } else {
            let mut bytes = std::fs::read(&path).unwrap();
            *bytes.last_mut().unwrap() ^= 1;
            std::fs::write(&path, bytes).unwrap();
        }
        for _ in 0..2 {
            let error =
                match recover_dimension_state(&config(directory.path()), Dimension::Overworld) {
                    Ok(_) => panic!("corrupt published WAL must refuse recovery"),
                    Err(error) => error.to_string(),
                };
            assert!(error.contains("batch-00000000000000000002"), "{error}");
            assert!(
                error.contains(if truncate {
                    "incomplete payload"
                } else {
                    "CRC mismatch"
                }),
                "{error}"
            );
        }
    }
}
