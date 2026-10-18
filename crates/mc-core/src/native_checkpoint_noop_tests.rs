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
        second["native_bytes_after"], second["native_bytes_before"],
        "redb must reuse free pages rather than grow on repeated no-op saves"
    );
}
