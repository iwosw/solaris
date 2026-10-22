use super::*;
use crate::dimensions::Dimension;
use crate::native_checkpoint::tests::read_manifest;

fn recover(config: &CoreConfig) -> TickState {
    let mut state = recover_dimension_state(config, Dimension::Overworld).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        state.other_dimensions.insert(
            dimension,
            Box::new(recover_dimension_state(config, dimension).unwrap()),
        );
    }
    state
}

fn process_io() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/io").unwrap();
    let counter = |key| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    (counter("write_bytes: "), counter("wchar: "))
}

fn save(
    config: &CoreConfig,
    state: &mut TickState,
    writer: &mut JournalWriter,
    manifest: &mut CheckpointManifest,
    previous_logical_bytes: Option<u64>,
) -> serde_json::Value {
    let path = config.world_dir.join("native/state.redb");
    let before_size = std::fs::metadata(&path).unwrap().len();
    let before_written = process_io();
    let (work_tx, work_rx) = crate::core::journal_work_channel();
    let (done_tx, _) = std::sync::mpsc::channel();
    let tick = state.tick;
    assert!(start_checkpoint(state, &work_tx, &done_tx, tick));
    let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
        panic!("expected checkpoint");
    };
    let store = job.regions.store().unwrap();
    let logical_before =
        previous_logical_bytes.unwrap_or_else(|| store.logical_data_bytes().unwrap());
    let (written, children, io) =
        write_checkpoint(&config.world_dir, &job, &state.plugins, writer, manifest).unwrap();
    let captured_chunks = written.len() + children.values().map(Vec::len).sum::<usize>();
    state.world.mark_flushed(&written);
    state.block_ticks.acknowledge_flushed(&job.block_ticks);
    state.fluid_ticks.acknowledge_flushed(&job.fluid_ticks);
    for (dimension, written) in children {
        let child = state.other_dimensions.get_mut(&dimension).unwrap();
        child.world.mark_flushed(&written);
        let capture = &job.dimensions[&dimension];
        child.block_ticks.acknowledge_flushed(&capture.block_ticks);
        child.fluid_ticks.acknowledge_flushed(&capture.fluid_ticks);
    }
    let after_size = std::fs::metadata(&path).unwrap().len();
    let after_written = process_io();
    let logical_after = store.logical_data_bytes().unwrap();
    serde_json::json!({
        "tick": state.tick,
        "captured_chunks": captured_chunks,
        "rewritten_chunks": io.rewritten_chunks,
        "chunk_payload_bytes": io.chunk_payload_bytes,
        "rewritten_entities": io.rewritten_entities,
        "entity_payload_bytes": io.entity_payload_bytes,
        "payload_byte_scope": "actual inserted encoded chunk baseline/metadata/overlay and entity row bytes; excludes keys, indexes, dictionaries, globals and redb copy-on-write overhead",
        "native_bytes_before": before_size,
        "native_bytes_after": after_size,
        "logical_data_bytes_before": logical_before,
        "logical_data_bytes_after": logical_after,
        "process_storage_write_bytes": after_written.0 - before_written.0,
        "process_syscall_write_bytes": after_written.1 - before_written.1,
        "process_io_scope": "/proc/self/io deltas, not logical payload bytes; mmap/kernel writeback can be charged outside these counters",
    })
}

#[test]
fn native_noop_save_twice_keeps_chunks_and_size() {
    let temporary = tempfile::tempdir().unwrap();
    let existing = std::env::var_os("SOLARIS_NOOP_ROOT").map(std::path::PathBuf::from);
    let seed = std::env::var("SOLARIS_NOOP_SEED")
        .map(|seed| seed.parse::<i64>().unwrap())
        .unwrap_or(42);
    let config = CoreConfig::new(
        existing
            .clone()
            .unwrap_or_else(|| temporary.path().to_owned()),
        seed,
    );
    if existing.is_none() {
        let mut initial = recover(&config);
        let pos = mc_world::ChunkPos { x: 0, z: 0 };
        initial
            .world
            .ensure_blocking(&[(pos, 0)].into_iter().collect());
        initial.world.tick_inhabited_time(pos, 1);
        initial.world.finish_inhabited_tick(1);
        initial.world.finish_inhabited_tick(2);
        initial.tick = 2;
        initial
            .world
            .restore_sim_debt(
                &Replay {
                    tick: 2,
                    sim_debt: [(
                        [pos.x, pos.z],
                        crate::journal::SimDebtImage {
                            chunk: [pos.x, pos.z],
                            last_sim: 2,
                            remaining: 7,
                            active: false,
                            tick: 2,
                        },
                    )]
                    .into_iter()
                    .collect(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        initial.block_ticks = crate::block_ticks::BlockTicks::from_image(
            crate::block_ticks::BlockTicksImage {
                next_sequence: 0,
                chunks: vec![crate::block_ticks::BlockTickChunkImage {
                    chunk: [pos.x, pos.z],
                    ticks: Vec::new(),
                }],
            },
            None,
        );
        initial.fluid_ticks = crate::fluids::FluidTicks::from_image(
            crate::fluids::FluidTicksImage {
                due: Vec::new(),
                covered: vec![[pos.x, pos.z]],
                next_sequence: 0,
            },
            None,
        );
        let mut manifest = read_manifest(&config).unwrap();
        let mut writer =
            JournalWriter::open_recovered(&config.world_dir, 1, manifest.tick, manifest.tick)
                .unwrap();
        writer.append(1, 2, &[]).unwrap();
        save(&config, &mut initial, &mut writer, &mut manifest, None);
    }
    let mut state = recover(&config);
    let entity_count = state.entities.len()
        + state
            .other_dimensions
            .values()
            .map(|child| child.entities.len())
            .sum::<usize>();
    let mut manifest = read_manifest(&config).unwrap();
    let initial_checkpoint_tick = manifest.tick;
    let mut writer =
        JournalWriter::open_recovered(&config.world_dir, 1, manifest.tick, state.tick).unwrap();
    let first = save(&config, &mut state, &mut writer, &mut manifest, None);
    let second = save(
        &config,
        &mut state,
        &mut writer,
        &mut manifest,
        Some(first["logical_data_bytes_after"].as_u64().unwrap()),
    );
    let receipt = serde_json::json!({
        "scope": "open once, save twice at recovered tick without running simulation ticks or changing owners",
        "seed": seed,
        "entities": entity_count,
        "initial_checkpoint_tick": initial_checkpoint_tick,
        "recovered_tick": state.tick,
        "first": first,
        "second": second,
    });
    if existing.is_some() {
        std::fs::write(
            config.world_dir.join("noop-saves.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
    }
    println!("{receipt}");
    if state.tick == initial_checkpoint_tick {
        assert_eq!(
            first["captured_chunks"], 0,
            "durable checkpoint images are clean on open"
        );
    }
    assert_eq!(
        second["captured_chunks"], 0,
        "a no-op save must not recapture clean chunks"
    );
    assert_eq!(second["chunk_payload_bytes"], 0);
    assert_eq!(second["rewritten_chunks"], 0);
    assert_eq!(second["rewritten_entities"], 0);
    assert_eq!(second["entity_payload_bytes"], 0);
    assert_eq!(
        second["logical_data_bytes_after"], second["logical_data_bytes_before"],
        "a no-op save must not rewrite global or player state"
    );
    assert!(
        second["native_bytes_after"].as_u64() <= second["native_bytes_before"].as_u64(),
        "redb must reuse free pages rather than grow on repeated no-op saves"
    );
}

#[test]
fn native_active_clock_checkpoints_write_only_changed_chunks() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_owned(), 42);
    let mut state = recover(&config);
    let positions = [
        mc_world::ChunkPos { x: 0, z: 0 },
        mc_world::ChunkPos { x: 8, z: 0 },
    ];
    state
        .world
        .ensure_blocking(&positions.map(|pos| (pos, 0)).into_iter().collect());
    let mut manifest = read_manifest(&config).unwrap();
    let mut writer =
        JournalWriter::open_recovered(&config.world_dir, 1, manifest.tick, state.tick).unwrap();
    for tick in 1..=4 {
        state.tick = tick;
        state
            .world
            .set_active_chunks(positions.into_iter().collect(), tick);
        for pos in positions {
            state.world.tick_inhabited_time(pos, tick);
        }
        state.world.finish_inhabited_tick(tick);
        if tick == 3 {
            let stone = state
                .registry
                .blocks
                .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
                .unwrap()
                .default
                .0;
            state.world.set_block(1, 200, 1, stone);
        }
        let mut entries = Vec::new();
        state.world.journal_inhabited_time(&mut entries);
        state.world.journal_debt(&mut entries);
        if tick == 3 {
            entries.push(JournalEntry::Block {
                pos: [1, 200, 1],
                state: state.world.block_at(1, 200, 1).unwrap(),
            });
        }
        writer.append(tick, tick, &entries).unwrap();
        let io = save(&config, &mut state, &mut writer, &mut manifest, None);
        let expected = match tick {
            1 => 2, // Initial clock boundaries establish the native chunk image.
            3 => 1, // A real edit writes that chunk exactly once.
            _ => 0, // Live clock accrual is carried by the manifest, not chunks.
        };
        assert_eq!(io["captured_chunks"], expected, "capture at tick {tick}");
        assert_eq!(io["rewritten_chunks"], expected, "writes at tick {tick}");
        if expected == 0 {
            assert_eq!(io["chunk_payload_bytes"], 0);
        }
    }
    writer.flush().unwrap();
    drop(writer);
    prune_batches_through(&config.world_dir.join("journal"), manifest.tick);
    drop(state);
    let mut reopened = recover(&config);
    reopened
        .world
        .ensure_blocking(&positions.map(|pos| (pos, 0)).into_iter().collect());
    for pos in positions {
        assert_eq!(reopened.world.inhabited_time(pos), 4);
    }
    let stone = reopened
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    assert_eq!(reopened.world.block_at(1, 200, 1), Some(stone));
    // Closing a recovered active debt anchor must reach native storage before
    // the manifest drops that now-inactive, zero-debt record.
    let mut manifest = read_manifest(&config).unwrap();
    let mut writer =
        JournalWriter::open_recovered(&config.world_dir, 1, manifest.tick, reopened.tick).unwrap();
    save(&config, &mut reopened, &mut writer, &mut manifest, None);
    drop(writer);
    drop(reopened);
    let mut reopened = recover(&config);
    reopened
        .world
        .ensure_blocking(&positions.map(|pos| (pos, 0)).into_iter().collect());
    reopened
        .world
        .set_active_chunks(positions.into_iter().collect(), 5);
    for pos in positions {
        assert_eq!(
            reopened.world.simulation_debt(pos),
            0,
            "reopen must not accrue already checkpointed live time"
        );
    }
}

#[test]
fn inhabited_interval_closed_after_restart_reaches_native_storage() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_owned(), 42);
    let mut state = recover(&config);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    state
        .world
        .ensure_blocking(&[(pos, 0)].into_iter().collect());
    let mut manifest = read_manifest(&config).unwrap();
    let mut writer =
        JournalWriter::open_recovered(&config.world_dir, 1, manifest.tick, state.tick).unwrap();
    for tick in 1..=3 {
        state.tick = tick;
        // No simulation-debt record: only the inhabited interval is in play.
        state.world.tick_inhabited_time(pos, tick);
        state.world.finish_inhabited_tick(tick);
        let mut entries = Vec::new();
        state.world.journal_inhabited_time(&mut entries);
        writer.append(tick, tick, &entries).unwrap();
        save(&config, &mut state, &mut writer, &mut manifest, None);
    }
    writer.flush().unwrap();
    drop(writer);
    prune_batches_through(&config.world_dir.join("journal"), manifest.tick);
    drop(state);

    // Clean restart: the interval is still open (value 3 lives in the manifest).
    let mut reopened = recover(&config);
    let mut manifest = read_manifest(&config).unwrap();
    let mut writer =
        JournalWriter::open_recovered(&config.world_dir, 1, manifest.tick, reopened.tick).unwrap();
    // One tick without any player: the interval closes and must be persisted.
    let tick = reopened.tick + 1;
    reopened.tick = tick;
    reopened.world.finish_inhabited_tick(tick);
    let mut entries = Vec::new();
    reopened.world.journal_inhabited_time(&mut entries);
    reopened.world.journal_debt(&mut entries);
    writer.append(tick, tick, &entries).unwrap();
    save(&config, &mut reopened, &mut writer, &mut manifest, None);
    writer.flush().unwrap();
    drop(writer);
    prune_batches_through(&config.world_dir.join("journal"), manifest.tick);
    drop(reopened);

    let mut again = recover(&config);
    again
        .world
        .ensure_blocking(&[(pos, 0)].into_iter().collect());
    assert_eq!(again.world.inhabited_time(pos), 3);
}
