use super::*;
use crate::dimensions::Dimension;
use crate::journal::read_complete_batches_after;
use crate::native_checkpoint::tests::read_manifest;

fn config(root: &std::path::Path) -> CoreConfig {
    let mut config = CoreConfig::new(root.to_owned(), 42);
    config.checkpoint_every_ticks = u64::MAX;
    config.journal_fsync_every_batches = 1;
    config.view_distance = 1;
    config.simulation_distance = 0;
    config
}

#[test]
fn native_manifest_rejects_missing_domain_versions() {
    assert!(serde_json::from_str::<CheckpointManifest>(r#"{"tick":9}"#).is_err());
}

#[test]
fn durable_api_ticket_survives_checkpoint_empty_tail_restart_without_portal_ticket() {
    let directory = tempfile::tempdir().unwrap();
    let config = config(directory.path());
    let core = Core::open(config.clone()).unwrap();
    let mut receiver = core.subscribe();
    for (owner, chunk) in [
        ("example:forced", crate::boundary::ChunkPos { x: 3, z: 4 }),
        (
            "solaris:portal:test",
            crate::boundary::ChunkPos { x: 9, z: 10 },
        ),
    ] {
        core.add_ticket(RemoteTicket {
            owner: owner.into(),
            chunk,
            radius: 0,
            reason: "checkpoint regression".into(),
            budget_chunks: 1,
        })
        .unwrap();
    }
    // Admission occurs before broadcast on the tick thread.
    receiver.blocking_recv().unwrap();
    receiver.blocking_recv().unwrap();
    assert!(core.checkpoint());
    core.shutdown();
    let manifest = read_manifest(&config).unwrap();
    assert_eq!(manifest.tickets.len(), 1);
    assert_eq!(manifest.tickets["example:forced"].0, [3, 4]);
    assert_eq!(manifest.tickets["example:forced"].1, (manifest.tick, 0));
    prune_batches_through(&directory.path().join("journal"), manifest.tick);
    assert!(
        read_complete_batches_after(&directory.path().join("journal"), 0)
            .unwrap()
            .is_empty()
    );
    let restored = Core::open(config.clone()).unwrap();
    let mut receiver = restored.subscribe();
    let mut forced_loaded = false;
    for _ in 0..100 {
        let output = receiver.blocking_recv().unwrap();
        forced_loaded |= output
            .snapshot
            .chunks
            .iter()
            .any(|chunk| chunk.pos == mc_world::ChunkPos { x: 3, z: 4 });
        if forced_loaded {
            break;
        }
    }
    assert!(
        forced_loaded,
        "durable ticket forces loading without any players"
    );
    assert!(restored.checkpoint());
    let recovered = read_manifest(&config).unwrap();
    assert_eq!(recovered.tickets.len(), 1);
    assert_eq!(recovered.tickets["example:forced"].0, [3, 4]);
    assert!(!recovered.tickets.contains_key("solaris:portal:test"));
    restored.shutdown();
}

#[test]
fn ticket_capture_is_frozen_and_child_isolated_with_wal_override() {
    let directory = tempfile::tempdir().unwrap();
    let config = config(directory.path());
    let core = Core::open(config.clone()).unwrap();
    core.shutdown();
    let mut manifest = read_manifest(&config).unwrap();
    let load =
        |dimension, _: &CheckpointManifest| recover_dimension_state(&config, dimension).unwrap();
    let mut state = load(Dimension::Overworld, &manifest);
    for (dimension, centre) in [
        (Dimension::Overworld, [3, 4]),
        (Dimension::Nether, [5, 6]),
        (Dimension::End, [7, 8]),
    ] {
        let world = if dimension == Dimension::Overworld {
            &mut state
        } else {
            state
                .other_dimensions
                .insert(dimension, Box::new(load(dimension, &manifest)));
            state.other_dimensions.get_mut(&dimension).unwrap()
        };
        world.tickets.insert(
            "example:shared-owner".into(),
            (
                crate::boundary::ChunkPos {
                    x: centre[0],
                    z: centre[1],
                },
                1,
            ),
        );
        world.tickets.insert(
            "solaris:portal:transient".into(),
            (crate::boundary::ChunkPos { x: 20, z: 20 }, 0),
        );
    }
    let tick = manifest.tick + 1;
    let (work_tx, work_rx) = journal_work_channel();
    let (done_tx, _done_rx) = std::sync::mpsc::channel();
    assert!(start_checkpoint(&mut state, &work_tx, &done_tx, tick));
    // Mutations after capture must not leak into the journal-thread write.
    state.tickets.clear();
    for child in state.other_dimensions.values_mut() {
        child.tickets.clear();
    }
    let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
        panic!("expected frozen checkpoint job");
    };
    let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
    writer.append(tick, tick, &[]).unwrap();
    write_checkpoint(
        directory.path(),
        &job,
        &state.plugins,
        &mut writer,
        &mut manifest,
    )
    .unwrap();
    prune_batches_through(&directory.path().join("journal"), tick);
    assert!(
        read_complete_batches_after(&directory.path().join("journal"), 0)
            .unwrap()
            .is_empty()
    );
    for (dimension, centre) in [
        (Dimension::Overworld, [3, 4]),
        (Dimension::Nether, [5, 6]),
        (Dimension::End, [7, 8]),
    ] {
        let restored = load(dimension, &manifest);
        assert_eq!(restored.tickets.len(), 1);
        assert_eq!(
            restored.tickets["example:shared-owner"],
            (
                crate::boundary::ChunkPos {
                    x: centre[0],
                    z: centre[1]
                },
                1
            ),
        );
    }
    writer
        .append(
            tick + 1,
            tick + 1,
            &[JournalEntry::Dimension {
                dimension: Dimension::Nether,
                entries: vec![JournalEntry::Ticket {
                    owner: "example:shared-owner".into(),
                    chunk: [11, 12],
                    tick: tick + 1,
                    radius: 2,
                }],
            }],
        )
        .unwrap();
    writer.flush().unwrap();
    assert_eq!(
        load(Dimension::Nether, &manifest).tickets["example:shared-owner"],
        (crate::boundary::ChunkPos { x: 11, z: 12 }, 2),
    );
    assert_eq!(
        load(Dimension::Overworld, &manifest).tickets["example:shared-owner"],
        (crate::boundary::ChunkPos { x: 3, z: 4 }, 1),
    );
    assert_eq!(
        load(Dimension::End, &manifest).tickets["example:shared-owner"],
        (crate::boundary::ChunkPos { x: 7, z: 8 }, 1),
    );
}
