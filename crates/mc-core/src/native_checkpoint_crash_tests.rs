use super::*;
use crate::dimensions::Dimension;
use crate::journal::read_complete_batches_after;
use crate::native_checkpoint::tests::read_manifest;

fn recovered_world(config: &CoreConfig) -> TickState {
    let mut state = recover_dimension_state(config, Dimension::Overworld).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        state.other_dimensions.insert(
            dimension,
            Box::new(recover_dimension_state(config, dimension).unwrap()),
        );
    }
    state
}

fn captured_world(
    config: &CoreConfig,
    tick: u64,
) -> (
    TickState,
    Box<CheckpointJob>,
    std::sync::mpsc::Receiver<CheckpointAck>,
) {
    let mut state = recovered_world(config);
    let (work_tx, work_rx) = crate::core::journal_work_channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    assert!(start_checkpoint(&mut state, &work_tx, &done_tx, tick));
    let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
        panic!("expected checkpoint capture");
    };
    (state, job, done_rx)
}

#[test]
fn failed_native_transaction_keeps_previous_world_and_wal_without_dirty_ack() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let (state, job, done_rx) = captured_world(&config, 2);
    let mut manifest = read_manifest(&config).unwrap();
    let previous = manifest.clone();
    let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
    writer.append(manifest.tick + 1, 2, &[]).unwrap();
    *FAIL_CHECKPOINT_AFTER_REGIONS
        .lock()
        .expect("precommit failure hook") = Some(directory.path().to_path_buf());
    let error = write_checkpoint(
        directory.path(),
        &job,
        &state.plugins,
        &mut writer,
        &mut manifest,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "storage: injected checkpoint failure");
    assert_eq!(manifest.tick, previous.tick);
    assert_eq!(read_manifest(&config).unwrap().tick, previous.tick);
    assert!(matches!(
        done_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        read_complete_batches_after(&directory.path().join("journal"), 0)
            .unwrap()
            .len(),
        1
    );
    drop(job);
    drop(state);
    drop(writer);
    let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
    assert_eq!(recovered.manifest.tick, previous.tick);
    assert!(
        recovered
            .dimensions
            .values()
            .all(|dimension| dimension.tick == 2)
    );
}

#[test]
fn crash_after_durable_native_commit_recovers_complete_world_without_ack_or_pruning() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let (state, job, done_rx) = captured_world(&config, 2);
    let mut manifest = read_manifest(&config).unwrap();
    let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
    writer.append(manifest.tick + 1, 2, &[]).unwrap();
    *FAIL_CHECKPOINT_AFTER_NATIVE_COMMIT
        .lock()
        .expect("postcommit crash hook") = Some(directory.path().to_path_buf());
    let error = write_checkpoint(
        directory.path(),
        &job,
        &state.plugins,
        &mut writer,
        &mut manifest,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "storage: injected crash after native commit before acknowledgement"
    );
    let committed = read_manifest(&config).unwrap();
    assert_eq!(committed.tick, 2);
    assert!(
        committed
            .dimensions
            .values()
            .all(|dimension| dimension.tick == 2)
    );
    assert!(matches!(
        done_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        read_complete_batches_after(&directory.path().join("journal"), 0)
            .unwrap()
            .len(),
        1
    );
    drop(job);
    drop(state);
    drop(writer);
    let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
    assert_eq!(recovered.manifest.tick, 2);
    assert!(
        recovered
            .dimensions
            .values()
            .all(|dimension| dimension.tick == 2)
    );
}

#[test]
fn durable_checkpoint_prunes_covered_batches_and_crash_replays_only_tail() {
    for lose_ack in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let config = CoreConfig::new(directory.path().to_path_buf(), 42);
        let (state, job, done_rx) = captured_world(&config, 2);
        let mut manifest = read_manifest(&config).unwrap();
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        writer.append(1, 1, &[]).unwrap();
        writer.append(2, 2, &[]).unwrap();
        writer.gate(manifest.tick, job.tick).unwrap();
        // Later appends may already exist while the native transaction commits.
        // An unchanged tick still has a complete frame, not a recovery hole.
        writer.append(3, 3, &[]).unwrap();
        writer
            .append(
                4,
                4,
                &[JournalEntry::Difficulty {
                    value: crate::survival::Difficulty::Hard,
                }],
            )
            .unwrap();
        let done_rx = (!lose_ack).then_some(done_rx);
        let outcome = run_native_work(
            NativeWork::Checkpoint {
                job,
                plugins: Ok(Box::new(state.plugins.clone())),
            },
            directory.path(),
            &std::sync::atomic::AtomicBool::new(false),
            &std::sync::atomic::AtomicU64::new(manifest.tick),
            &mut manifest,
            &mut Default::default(),
        );
        assert!(
            outcome.is_continue(),
            "a checkpoint does not stop the writer"
        );
        if let Some(done_rx) = done_rx {
            assert!(done_rx.recv().unwrap().is_some());
        }
        let journal = directory.path().join("journal");
        for tick in 1..=2 {
            assert!(
                !journal
                    .join(format!("batch-{tick:020}-{tick:020}.bin"))
                    .exists(),
                "batch {tick} is covered by the durable checkpoint (lost ACK: {lose_ack})",
            );
        }
        let batches = read_complete_batches_after(&journal, manifest.tick).unwrap();
        assert_eq!(
            batches
                .iter()
                .map(|(lo, hi, _)| (*lo, *hi))
                .collect::<Vec<_>>(),
            vec![(3, 3), (4, 4)],
            "only the tail above the durable checkpoint may remain (lost ACK: {lose_ack})",
        );
        assert!(batches[0].2.is_empty(), "unchanged tick remains replayable");
        drop(state);
        drop(writer);
        // Abrupt reopen: no graceful final checkpoint can hide a missing tail.
        let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
        assert_eq!(recovered.manifest.tick, 2);
        for replay in recovered.dimensions.values() {
            assert_eq!(replay.tick, 4);
            assert_eq!(replay.difficulty, crate::survival::Difficulty::Hard);
        }
    }
}

#[tokio::test]
async fn queued_save_requests_keep_one_capture_in_flight_and_both_receive_durable_ack() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let mut state = recover_dimension_state(&config, Dimension::Overworld).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        state.other_dimensions.insert(
            dimension,
            Box::new(recover_dimension_state(&config, dimension).unwrap()),
        );
    }
    let manifest = read_manifest(&config).unwrap();
    let (_input_tx, input_rx) = mpsc_channel(16);
    let (_spawn_tx, spawn_rx) = mpsc_channel(16);
    let (_ticket_tx, ticket_rx) = mpsc_channel(16);
    let (_moderation_selector_tx, moderation_selector_rx) = mpsc_channel(16);
    let (tick_broadcast, mut ticks) = broadcast::channel(16);
    let (timing_broadcast, _) = broadcast::channel(16);
    let (work_tx, work_rx) = crate::core::journal_work_channel();
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let (checkpoint_tx, checkpoint_rx) = std::sync::mpsc::channel();
    let (outbox_tx, _outbox_rx) = std::sync::mpsc::channel();
    let (first_tx, first_rx) = std::sync::mpsc::channel();
    let (second_tx, second_rx) = std::sync::mpsc::channel();
    checkpoint_tx.send(first_tx).unwrap();
    checkpoint_tx.send(second_tx).unwrap();
    let generated_busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let journal_busy = Arc::clone(&generated_busy);
    let journal_plugins = state.plugins.clone();
    let journal_budget = Arc::new(crate::journal_budget::JournalBudget::new(
        crate::journal_budget::JOURNAL_QUEUE_BYTES,
    ));
    let tick_budget = Arc::clone(&journal_budget);
    let terrain_sources = |state: &crate::tick::TickState| {
        [
            Arc::new(crate::terrain::TerrainSource::new(
                state.world.chunk_source(),
                String::new(),
            )),
            Arc::new(crate::terrain::TerrainSource::new(
                state
                    .other_dimensions
                    .get(&Dimension::Nether)
                    .expect("nether")
                    .world
                    .chunk_source(),
                String::new(),
            )),
            Arc::new(crate::terrain::TerrainSource::new(
                state
                    .other_dimensions
                    .get(&Dimension::End)
                    .expect("end")
                    .world
                    .chunk_source(),
                String::new(),
            )),
        ]
    };
    let terrain = terrain_sources(&state);
    let tick = std::thread::spawn(move || {
        tick_loop(
            state,
            TickChannels {
                checkpoint_every: u64::MAX,
                input_rx,
                spawn_rx,
                ticket_rx,
                moderation_selector_rx,
                outbox: outbox_tx,
                tick_broadcast,
                timing_broadcast,
                work_tx,
                journal_budget: tick_budget,
                generated_busy,
                unlocked_sessions: Arc::new(parking_lot::RwLock::new(Default::default())),
                shutdown_rx,
                checkpoint_rx,
                backup_rx: std::sync::mpsc::channel().1,
                pause_when_empty_seconds: 0,
                input_log: None,
                terrain,
            },
        );
    });
    let timeout = std::time::Duration::from_secs(10);
    let mut queued = Vec::new();
    let first_tick = loop {
        let work = work_rx.recv_timeout(timeout).unwrap();
        let captured = match &work {
            JournalWork::Checkpoint(job) => Some(job.tick),
            _ => None,
        };
        queued.push(work);
        if let Some(tick) = captured {
            break tick;
        }
    };
    // Hold the journal before the first ACK while the real tick owner runs.
    // A second request must remain in checkpoint_rx, not freeze another image.
    loop {
        let output = tokio::time::timeout(timeout, ticks.recv())
            .await
            .unwrap()
            .unwrap();
        if output.tick > first_tick + 1 {
            break;
        }
    }
    queued.extend(work_rx.try_iter());
    assert_eq!(
        queued
            .iter()
            .filter(|work| matches!(work, JournalWork::Checkpoint(_)))
            .count(),
        1
    );
    assert!(matches!(
        first_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        second_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));

    // Release every work item in its original order to the real durable writer.
    // The journal thread is live before re-injection, so the bounded
    // production channel applies real backpressure while draining.
    let (journal_tx, journal_rx) = crate::core::journal_work_channel();
    let root = config.world_dir.clone();
    let journal = std::thread::spawn(move || {
        journal_loop(
            &root,
            journal_rx,
            JournalWriter::open(&root, 1).unwrap(),
            &journal_busy,
            &journal_budget,
            manifest,
            journal_plugins,
        );
    });
    for work in queued {
        journal_tx.send(work).unwrap();
    }
    let forward = std::thread::spawn(move || {
        for work in work_rx {
            journal_tx.send(work).unwrap();
        }
    });
    assert!(first_rx.recv_timeout(timeout).unwrap());
    assert!(second_rx.recv_timeout(timeout).unwrap());
    assert!(
        crate::native_checkpoint::tests::fixture_store(&config)
            .unwrap()
            .checkpoint_reader()
            .unwrap()
            .read_checkpoint()
            .unwrap()
            .tick
            > first_tick,
        "second save must commit a later capture after the first ACK",
    );
    shutdown_tx.send(()).unwrap();
    tick.join().unwrap();
    forward.join().unwrap();
    journal.join().unwrap();
}

#[test]
fn plugin_checkpoint_serialization_uses_fifo_mirror_not_later_tick_state() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let mut state = recover_dimension_state(&config, Dimension::Overworld).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        state.other_dimensions.insert(
            dimension,
            Box::new(recover_dimension_state(&config, dimension).unwrap()),
        );
    }
    let manifest = read_manifest(&config).unwrap();
    let mirror = state.plugins.clone();
    let tick = manifest.tick + 1;
    state.tick = tick;
    state.plugins.set_domain(
        "checkpoint-test",
        "row",
        serde_json::json!({"value": "before"}),
    );
    let mut before = Vec::new();
    crate::plugin::journal(&mut state, &mut before);
    let (work_tx, work_rx) = crate::core::journal_work_channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    work_tx
        .send(JournalWork::Append {
            tick_lo: tick,
            tick_hi: tick,
            entries: before,
            plugin_ack: None,
            reserved: 0,
        })
        .unwrap();
    assert!(start_checkpoint(&mut state, &work_tx, &done_tx, tick));
    // Bounded queue: drain each accepted batch into `queued` in send order
    // (the journal thread is not running yet; nothing is lost).
    let mut queued: Vec<_> = work_rx.try_iter().collect();
    assert_eq!(queued.len(), 2);
    state.tick = tick + 1;
    state.plugins.set_domain(
        "checkpoint-test",
        "row",
        serde_json::json!({"value": "after"}),
    );
    let mut after = Vec::new();
    crate::plugin::journal(&mut state, &mut after);
    work_tx
        .send(JournalWork::Append {
            tick_lo: tick + 1,
            tick_hi: tick + 1,
            entries: after,
            plugin_ack: None,
            reserved: 0,
        })
        .unwrap();
    work_tx.send(JournalWork::Shutdown).unwrap();
    queued.extend(work_rx.try_iter());
    // Capturing a checkpoint carries no serialized or cloned plugin image.
    assert!(queued.iter().any(|work| matches!(work,
        JournalWork::Checkpoint(job) if !job.globals.contains_key("plugins"))));
    // Bounded relay: the backlog captured above can exceed JOURNAL_WORK_QUEUE,
    // so the journal thread is live before re-injection — it drains while
    // this thread sends, and FIFO order is preserved end to end.
    let (journal_tx, journal_rx) = crate::core::journal_work_channel();
    let root = config.world_dir.clone();
    let journal = std::thread::spawn(move || {
        journal_loop(
            &root,
            journal_rx,
            JournalWriter::open(&root, 1).unwrap(),
            &std::sync::atomic::AtomicBool::new(false),
            &crate::journal_budget::JournalBudget::new(crate::journal_budget::JOURNAL_QUEUE_BYTES),
            manifest,
            mirror,
        );
    });
    for work in queued {
        journal_tx.send(work).unwrap();
    }
    journal.join().unwrap();
    assert!(done_rx.recv().unwrap().is_some());
    let checkpoint = crate::native_checkpoint::tests::fixture_store(&config)
        .unwrap()
        .checkpoint_reader()
        .unwrap()
        .read_checkpoint()
        .unwrap();
    assert_eq!(checkpoint.tick, tick);
    let captured: crate::plugin::PluginState =
        serde_json::from_slice(&checkpoint.globals["plugins"].payload).unwrap();
    assert_eq!(
        captured.get_domain("checkpoint-test", "row").unwrap()["value"],
        "before"
    );
    let replayed = recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(
        replayed
            .plugins
            .get_domain("checkpoint-test", "row")
            .unwrap()["value"],
        "after"
    );
}

/// A multi-second native commit must not park WAL appends: tick N+3's
/// `Append` used to block behind the two-deep queue while the journal thread sat
/// inside the checkpoint commit.
#[test]
fn slow_native_commit_does_not_block_wal_appends() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let mut state = recover_dimension_state(&config, Dimension::Overworld).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        state.other_dimensions.insert(
            dimension,
            Box::new(recover_dimension_state(&config, dimension).unwrap()),
        );
    }
    let manifest = read_manifest(&config).unwrap();
    let mirror = state.plugins.clone();
    let tick = manifest.tick + 1;
    state.tick = tick;
    let append = |tick: u64| JournalWork::Append {
        tick_lo: tick,
        tick_hi: tick,
        entries: Vec::new(),
        plugin_ack: None,
        reserved: 0,
    };
    let (capture_tx, capture_rx) = crate::core::journal_work_channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    capture_tx.send(append(tick)).unwrap();
    assert!(start_checkpoint(&mut state, &capture_tx, &done_tx, tick));
    let queued: Vec<_> = capture_rx.try_iter().collect();
    assert_eq!(queued.len(), 2);
    let delay = std::time::Duration::from_millis(1500);
    *crate::core::DELAY_CHECKPOINT_COMMIT.lock() = Some((config.world_dir.clone(), delay));
    let (journal_tx, journal_rx) = crate::core::journal_work_channel();
    let root = config.world_dir.clone();
    let journal = std::thread::spawn(move || {
        journal_loop(
            &root,
            journal_rx,
            JournalWriter::open(&root, 1).unwrap(),
            &std::sync::atomic::AtomicBool::new(false),
            &crate::journal_budget::JournalBudget::new(crate::journal_budget::JOURNAL_QUEUE_BYTES),
            manifest,
            mirror,
        );
    });
    for work in queued {
        journal_tx.send(work).unwrap();
    }
    let started = std::time::Instant::now();
    for offset in 1..=8 {
        journal_tx.send(append(tick + offset)).unwrap();
    }
    let sent_in = started.elapsed();
    journal_tx.send(JournalWork::Shutdown).unwrap();
    journal.join().unwrap();
    *crate::core::DELAY_CHECKPOINT_COMMIT.lock() = None;
    assert!(
        sent_in < delay / 3,
        "appends parked behind the checkpoint commit for {sent_in:?}"
    );
    assert!(done_rx.recv().unwrap().is_some());
    assert_eq!(
        crate::native_checkpoint::tests::fixture_store(&config)
            .unwrap()
            .checkpoint_reader()
            .unwrap()
            .read_checkpoint()
            .unwrap()
            .tick,
        tick
    );
    // Every append published after the checkpoint is retained for replay.
    let after = read_complete_batches_after(&config.world_dir.join("journal"), tick).unwrap();
    assert_eq!(after.len(), 8);
}

struct LiveJournal {
    tick: u64,
    tx: std::sync::mpsc::SyncSender<JournalWork>,
    done_rx: std::sync::mpsc::Receiver<CheckpointAck>,
    journal: std::thread::JoinHandle<()>,
}

fn append_work(tick: u64) -> JournalWork {
    JournalWork::Append {
        tick_lo: tick,
        tick_hi: tick,
        entries: Vec::new(),
        plugin_ack: None,
        reserved: 0,
    }
}

/// A live journal thread that has been handed Append(tick) then a Checkpoint(tick).
/// Hooks keyed by the world dir must be armed by the caller before this call.
fn live_journal_with_checkpoint(config: &CoreConfig) -> LiveJournal {
    let mut state = recover_dimension_state(config, Dimension::Overworld).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        state.other_dimensions.insert(
            dimension,
            Box::new(recover_dimension_state(config, dimension).unwrap()),
        );
    }
    let manifest = read_manifest(config).unwrap();
    let mirror = state.plugins.clone();
    let tick = manifest.tick + 1;
    state.tick = tick;
    let (capture_tx, capture_rx) = crate::core::journal_work_channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    capture_tx.send(append_work(tick)).unwrap();
    assert!(start_checkpoint(&mut state, &capture_tx, &done_tx, tick));
    let queued: Vec<_> = capture_rx.try_iter().collect();
    let (tx, journal_rx) = crate::core::journal_work_channel();
    let root = config.world_dir.clone();
    let journal = std::thread::spawn(move || {
        journal_loop(
            &root,
            journal_rx,
            JournalWriter::open(&root, 1).unwrap(),
            &std::sync::atomic::AtomicBool::new(false),
            &crate::journal_budget::JournalBudget::new(crate::journal_budget::JOURNAL_QUEUE_BYTES),
            manifest,
            mirror,
        );
    });
    for work in queued {
        tx.send(work).unwrap();
    }
    LiveJournal {
        tick,
        tx,
        done_rx,
        journal,
    }
}

/// Keep appending until the journal's receiver is gone; returns how long that took.
fn time_to_halt(live: &LiveJournal, limit: std::time::Duration) -> Option<std::time::Duration> {
    let started = std::time::Instant::now();
    for offset in 2.. {
        if live.tx.send(append_work(live.tick + offset)).is_err() {
            return Some(started.elapsed());
        }
        if started.elapsed() > limit {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

/// A failed append must fail the tick's next `send` at once, not after the
/// in-flight native commit that the journal scope still has to join.
#[test]
fn append_failure_halts_tick_without_waiting_for_native_commit() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let delay = std::time::Duration::from_millis(2000);
    *crate::core::DELAY_CHECKPOINT_COMMIT.lock() = Some((config.world_dir.clone(), delay));
    let live = live_journal_with_checkpoint(&config);
    // Let the journal thread gate and hand the checkpoint to the native writer.
    std::thread::sleep(std::time::Duration::from_millis(400));
    *FAIL_JOURNAL_APPEND.lock().unwrap() = Some(config.world_dir.clone());
    live.tx.send(append_work(live.tick + 1)).unwrap();
    let halted = time_to_halt(&live, delay * 2);
    *FAIL_JOURNAL_APPEND.lock().unwrap() = None;
    *crate::core::DELAY_CHECKPOINT_COMMIT.lock() = None;
    let LiveJournal {
        journal, done_rx, ..
    } = live;
    journal.join().unwrap();
    assert!(
        halted.is_some_and(|elapsed| elapsed < delay / 2),
        "tick send kept blocking after the append failure: {halted:?}"
    );
    // The in-flight checkpoint was gated before the failure and still completes.
    assert!(done_rx.recv().unwrap().is_some());
}

/// A panicking native writer must fail closed: the job reports failure and the
/// journal side halts, so the tick cannot run on without checkpoints.
#[test]
fn native_writer_panic_fails_checkpoint_and_halts_journal() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    *crate::core::PANIC_CHECKPOINT_COMMIT.lock() = Some(config.world_dir.clone());
    let live = live_journal_with_checkpoint(&config);
    let ack = live
        .done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("panicked checkpoint must still report on `done`");
    assert!(ack.is_none());
    let halted = time_to_halt(&live, std::time::Duration::from_secs(5));
    *crate::core::PANIC_CHECKPOINT_COMMIT.lock() = None;
    assert!(
        halted.is_some(),
        "journal kept accepting work after the native writer died"
    );
    live.journal.join().unwrap();
}

#[test]
fn imported_bee_hive_release_checkpoints_after_kill_and_unload() {
    fn save(
        config: &CoreConfig,
        state: &mut TickState,
        writer: &mut JournalWriter,
        manifest: &mut CheckpointManifest,
    ) {
        let (work_tx, work_rx) = crate::core::journal_work_channel();
        let (done_tx, _) = std::sync::mpsc::channel();
        assert!(start_checkpoint(state, &work_tx, &done_tx, state.tick));
        let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
            panic!("expected full entity capture");
        };
        writer.append(manifest.tick + 1, state.tick, &[]).unwrap();
        write_checkpoint(&config.world_dir, &job, &state.plugins, writer, manifest).unwrap();
    }

    let directory = tempfile::tempdir().unwrap();
    let source_config = CoreConfig::new(directory.path().join("fixture"), 42);
    let mut source = recovered_world(&source_config);
    source.tick = 1;
    let pos = crate::bees::tests::POS;
    let chunk = crate::boundary::ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    };
    source
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    crate::bees::tests::place(&mut source, pos, "minecraft:beehive");
    let bee = crate::bees::tests::mob(
        &mut source,
        "minecraft:bee",
        [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ],
    );
    let bee_slot = source.entities.slot_of(bee).unwrap();
    source.entities.row_mut(bee_slot).controls.no_ai = true;
    let bee_uuid = source.entities.row(bee_slot).uuid;
    let cow = crate::bees::tests::mob(&mut source, "minecraft:cow", [6.5, 201.0, 6.5]);
    let cow_uuid = source
        .entities
        .row(source.entities.slot_of(cow).unwrap())
        .uuid;
    let mut source_manifest = read_manifest(&source_config).unwrap();
    let mut source_writer = JournalWriter::open(&source_config.world_dir, 1).unwrap();
    save(
        &source_config,
        &mut source,
        &mut source_writer,
        &mut source_manifest,
    );
    drop(source_writer);
    drop(source);

    let anvil = directory.path().join("anvil");
    crate::world_conversion::export_anvil(
        &source_config,
        &source_config.world_dir,
        &anvil,
        &directory.path().join("export.json"),
    )
    .unwrap();
    let config = CoreConfig::new(directory.path().join("imported"), 42);
    let imported = crate::world_conversion::import_anvil(
        &config,
        &anvil,
        &config.world_dir,
        &directory.path().join("import.json"),
    )
    .unwrap();
    assert_eq!(imported.counts["entities"], 2);
    let mut state = recovered_world(&config);
    state.tick = 2;
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    let bee_slot = state.entities.slot_of_uuid(bee_uuid).unwrap();
    let bee_id = state.entities.body(bee_slot).id;
    let bee = state.entities.row_mut(bee_slot).bee.as_mut().unwrap();
    bee.nectar = true;
    bee.hive = Some(pos);
    let cow_slot = state.entities.slot_of_uuid(cow_uuid).unwrap();
    state.entities.remove(cow_slot);
    let summoned = crate::bees::tests::mob(&mut state, "minecraft:villager", [8.5, 201.0, 8.5]);
    let summoned_uuid = state
        .entities
        .row(state.entities.slot_of(summoned).unwrap())
        .uuid;
    let mut changes = crate::boundary::ChangeSet::default();
    crate::bees::commit(&mut state, &mut changes);
    assert_eq!(changes.entities_removed, vec![bee_id]);
    assert!(state.entities.slot_of_uuid_any(bee_uuid).is_none());
    let mut manifest = read_manifest(&config).unwrap();
    let mut writer = JournalWriter::open(&config.world_dir, 1).unwrap();
    save(&config, &mut state, &mut writer, &mut manifest);
    drop(writer);
    drop(state);

    // A new owner loads only the checkpointed hive, not the old dense entity slot.
    let mut state = recovered_world(&config);
    state.tick = 3;
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    crate::bees::break_custody(&mut state, pos, &mut Vec::new()).unwrap();
    let released = *state.hostile.spawned.last().expect("hive releases its bee");
    let released_slot = state.entities.slot_of(released).unwrap();
    let released_uuid = state.entities.row(released_slot).uuid;
    assert!(state.entities.row(released_slot).controls.no_ai);
    let mut writer = JournalWriter::open(&config.world_dir, 1).unwrap();
    save(&config, &mut state, &mut writer, &mut manifest);
    assert_ne!(released_uuid, bee_uuid);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    for uuid in [bee_uuid, cow_uuid] {
        assert!(
            store
                .entity(*uuid.as_bytes())
                .unwrap()
                .unwrap()
                .payload
                .is_none()
        );
    }
    for uuid in [released_uuid, summoned_uuid] {
        assert!(
            store
                .entity(*uuid.as_bytes())
                .unwrap()
                .unwrap()
                .payload
                .is_some()
        );
    }
    drop(store);
    drop(writer);
    drop(state);
    let restored = recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert!(restored.entities.slot_of_uuid(released_uuid).is_some());
    assert!(restored.entities.slot_of_uuid(summoned_uuid).is_some());
    assert!(restored.entities.slot_of_uuid_any(bee_uuid).is_none());
    assert!(restored.entities.slot_of_uuid_any(cow_uuid).is_none());
}
