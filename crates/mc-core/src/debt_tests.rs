//! Per-chunk active clock, bounded debt and durable checkpoint fences (001-f).
//! Residency is not activity; inactive time is materialized on activation,
//! and explicit whole-chunk consumption owns the remaining-debt zero image.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::journal::{JournalEntry, JournalWriter, SimDebtImage};
use crate::world::ChunkWorld;
use mc_world::ChunkPos;

fn wants(positions: &[ChunkPos]) -> HashMap<ChunkPos, i32> {
    positions.iter().map(|pos| (*pos, 0)).collect()
}

fn active(positions: &[ChunkPos]) -> HashSet<ChunkPos> {
    positions.iter().copied().collect()
}

fn test_world(world_dir: Option<std::path::PathBuf>) -> ChunkWorld {
    crate::world::world_tests::test_world(world_dir)
}

fn sim_debt_images(entries: &[JournalEntry]) -> Vec<SimDebtImage> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            JournalEntry::SimDebt { image } => Some(*image),
            _ => None,
        })
        .collect()
}

#[test]
fn newly_resident_chunks_are_never_automatically_active() {
    let mut world = test_world(None);
    let pos = ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    assert!(world.has_chunk(pos.x, pos.z));
    assert!(
        !world.is_active(pos.x, pos.z),
        "strict empty default: residency is not activity"
    );
    assert_eq!(world.active_positions().count(), 0);
    // Explicit opt-in activates.
    world.set_active_chunks(active(&[pos]), 1);
    assert!(world.is_active(pos.x, pos.z));
}

#[test]
fn active_demand_before_admission_consumes_on_the_same_tick() {
    let directory = tempfile::tempdir().unwrap();
    let mut world = test_world(Some(directory.path().to_path_buf()));
    let pos = ChunkPos { x: 4, z: -3 };
    // Parent order: membership first, admission lands later the same tick.
    world.set_active_chunks(active(&[pos]), 7);
    assert!(
        !world.is_active(pos.x, pos.z),
        "demand without residency stays inactive"
    );
    world.ensure_blocking(&wants(&[pos]));
    world.refresh_active_clock(7);
    assert!(world.is_active(pos.x, pos.z));
    let mut entries = Vec::new();
    world.journal_debt(&mut entries);
    let images = sim_debt_images(&entries);
    assert_eq!(images[0].chunk, [pos.x, pos.z]);
    assert_eq!(images[0].last_sim, 7);
    assert_eq!(images[0].remaining, 0);
    assert!(images[0].active, "admission activation is an open boundary");
    // Persist the new clock before eviction: dirty debt intentionally pins
    // residency until the matching checkpoint acknowledgement.
    let written = world.capture_dirty(&Default::default()).write().unwrap();
    world.mark_flushed(&written);
    assert!(
        world
            .evict_outside(&wants(&[ChunkPos { x: 100, z: 100 }]))
            .contains(&pos)
    );
    world.refresh_active_clock(8);
    assert!(!world.is_active(pos.x, pos.z));
}

#[test]
fn resident_inactive_reactivation_records_elapsed_debt() {
    let mut world = test_world(None);
    let pos = ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    world.set_active_chunks(active(&[pos]), 10);
    assert!(world.is_active(pos.x, pos.z));
    // Go inactive: deactivation emits a frozen inactive boundary image.
    world.set_active_chunks(HashSet::default(), 11);
    assert!(!world.is_active(pos.x, pos.z));
    world.refresh_active_clock(50);
    let mut entries = Vec::new();
    world.journal_debt(&mut entries);
    // Latest-per-chunk wins: the frozen inactive boundary at sim_tick 50.
    let images = sim_debt_images(&entries);
    assert_eq!(images.len(), 1);
    assert!(!images[0].active, "deactivation is a WAL boundary image");
    assert_eq!((images[0].last_sim, images[0].tick), (10, 50));
    // Reactivation records ticks 11..=59; tick 60 itself runs live.
    world.set_active_chunks(active(&[pos]), 60);
    let mut entries = Vec::new();
    world.journal_debt(&mut entries);
    let images = sim_debt_images(&entries);
    assert_eq!(images[0].chunk, [pos.x, pos.z]);
    assert_eq!(images[0].last_sim, 60);
    assert_eq!(images[0].remaining, 49);
    assert_eq!(images[0].tick, 60);
    assert!(images[0].active, "activation boundary is open");
    // Continuing active advances the clock with no journal/dirty.
    world.refresh_active_clock(61);
    let mut entries = Vec::new();
    world.journal_debt(&mut entries);
    assert!(entries.is_empty(), "live accrual is silent");
}

#[test]
fn inactive_chunks_accrue_no_debt_clock() {
    let mut world = test_world(None);
    let pos = ChunkPos { x: 1, z: 1 };
    world.ensure_blocking(&wants(&[pos]));
    // Never activated: refresh ticks must not create records or images.
    world.refresh_active_clock(100);
    world.refresh_active_clock(200);
    assert!(!world.is_active(pos.x, pos.z));
    let mut entries = Vec::new();
    world.journal_debt(&mut entries);
    assert!(entries.is_empty(), "inactive residency accrues nothing");
    assert!(world.debt_checkpoint(200).is_empty());
    // Same-tick repeat activation is a single consumption.
    world.set_active_chunks(active(&[pos]), 200);
    world.refresh_active_clock(200);
    world.set_active_chunks(active(&[pos]), 200);
    let mut entries = Vec::new();
    world.journal_debt(&mut entries);
    assert_eq!(sim_debt_images(&entries).len(), 1);
}

#[test]
fn lazy_entity_projection_uses_the_current_chunk_clock_boundary() {
    let mut world = test_world(None);
    let pos = ChunkPos { x: 0, z: 0 };
    let initial = world.active_revision();
    world.ensure_blocking(&wants(&[pos]));
    assert!(
        world.active_revision() > initial,
        "admission changes membership"
    );
    assert_eq!(world.simulation_interval(pos, 0, 10), None);

    let resident = world.active_revision();
    world.set_active_chunks(active(&[pos]), 10);
    assert!(world.active_revision() > resident);
    assert_eq!(
        world.simulation_interval(pos, 8, 10),
        Some((9, 10)),
        "activation tick is live"
    );
    let mut entries = Vec::new();
    world.journal_debt(&mut entries);
    let active_revision = world.active_revision();
    world.refresh_active_clock(11);
    assert_eq!(
        world.active_revision(),
        active_revision,
        "accrual is not membership"
    );
    assert_eq!(world.simulation_interval(pos, 10, 11), Some((10, 11)));

    world.set_active_chunks(HashSet::default(), 12);
    assert!(world.active_revision() > active_revision);
    assert_eq!(
        world.simulation_interval(pos, 10, 12),
        Some((10, 11)),
        "deactivation tick is frozen"
    );
    entries.clear();
    world.journal_debt(&mut entries);
    let inactive_revision = world.active_revision();
    world.refresh_active_clock(100);
    assert_eq!(world.active_revision(), inactive_revision);
    assert_eq!(world.simulation_interval(pos, 12, 100), None);

    world.set_active_chunks(active(&[pos]), 101);
    assert_eq!(
        world.simulation_interval(pos, 100, 101),
        Some((100, 101)),
        "no inactive gap accrues"
    );
    assert_eq!(world.simulation_interval(pos, 101, 101), None);
}

/// Nonzero authority debt through the real journal + native checkpoint:
/// activate consumes exactly once across a crash before ACK.
#[test]
fn nonzero_debt_eviction_before_ack_restart_consumes_exactly_once() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path().to_path_buf();
    let pos = ChunkPos { x: 0, z: 0 };
    // Seed nonzero authority debt via replay (a later slice's producer or a
    // pre-restart journal tail would write this shape).
    let seed = SimDebtImage {
        chunk: [pos.x, pos.z],
        last_sim: 5,
        remaining: 3,
        tick: 5,
        active: false,
    };
    {
        let mut world = test_world(Some(dir.clone()));
        world
            .restore_sim_debt(
                &crate::journal::Replay {
                    tick: 5,
                    sim_debt: [([pos.x, pos.z], seed)].into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        world.ensure_blocking(&wants(&[pos]));
        // Restored nonzero debt pins eviction before any checkpoint.
        assert!(
            !world.evict_outside(&HashMap::default()).contains(&pos),
            "restored debt pins eviction until checkpointed"
        );
        // Write the POSITIVE native capture + checkpoint at 5 (ACK withheld):
        // the stored image must carry remaining=3 before activation.
        let pre = world.capture_dirty(&Default::default());
        assert!(
            pre.written().iter().any(|(written, _)| *written == pos),
            "nonzero debt rides the checkpoint"
        );
        pre.write().unwrap();
        let stored = world
            .chunk(pos)
            .expect("resident after capture")
            .extras
            .iter()
            .filter_map(|(key, tag)| match (key.as_str(), tag) {
                ("SolarisSimDebt", mc_nbt::Tag::Long(value)) => Some(*value),
                _ => None,
            })
            .next();
        assert_eq!(stored, Some(3), "native image carries nonzero debt");
        // Activation preserves debt; the single merge owner explicitly
        // consumes the whole-chunk interval before journaling its effects.
        world.set_active_chunks(active(&[pos]), 10);
        assert_eq!(world.simulation_debt(pos), 7);
        world.consume_simulation_debt(pos, 7);
        let mut entries = Vec::new();
        world.journal_debt(&mut entries);
        let images = sim_debt_images(&entries);
        assert_eq!(images.len(), 1, "activation consumes once");
        assert_eq!((images[0].last_sim, images[0].remaining), (10, 0));
        // Attempted evict before the matching ACK must retain the chunk.
        assert!(
            !world.evict_outside(&HashMap::default()).contains(&pos),
            "consumption fence holds until ACK"
        );
        // native checkpoint but crash BEFORE mark_flushed (no ACK).
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer.append(1, 10, &entries).unwrap();
        writer.flush().unwrap();
        let capture = world.capture_dirty(&Default::default());
        assert!(
            capture.written().iter().any(|(written, _)| *written == pos),
            "consumption rides the checkpoint"
        );
        capture.write().unwrap();
        drop(writer);
        drop(world);
    }
    // Restart: read back the committed WAL tail + native checkpoint image,
    // fold latest-wins, restore, and prove no second consumption. Replay
    // tick is 10 (the zero batch); only that batch is durable.
    {
        let batches = crate::journal::read_complete_batches_after(&dir.join("journal"), 0)
            .expect("journal replays");
        assert_eq!(batches.len(), 1, "committed zero survives the crash");
        assert_eq!((batches[0].0, batches[0].1), (1, 10));
        let replay: crate::journal::Replay =
            crate::journal::replay_batches(Default::default(), &batches);
        assert_eq!(replay.tick, 10);
        let zeros = replay
            .sim_debt
            .values()
            .filter(|image| image.chunk == [pos.x, pos.z] && image.remaining == 0)
            .count();
        assert_eq!(zeros, 1, "exactly one zero-consumption image durable");
        assert!(
            !replay
                .sim_debt
                .values()
                .any(|image| image.chunk == [pos.x, pos.z] && image.remaining > 0),
            "no owed image survives past the committed zero"
        );
        let mut world = test_world(Some(dir.clone()));
        let tick = replay.tick;
        // The reopened native store (not just the live chunk) carries the
        // projected zero: admission reads it back through compose.
        world.restore_sim_debt(&replay, None).unwrap();
        world.ensure_blocking(&wants(&[pos]));
        world.set_active_chunks(active(&[pos]), tick.max(11));
        let mut entries = Vec::new();
        world.journal_debt(&mut entries);
        let images = sim_debt_images(&entries);
        assert_eq!(
            images.len(),
            1,
            "post-restart activation emits its own image"
        );
        assert_eq!(
            images[0].remaining, 0,
            "original nonzero debt is never consumed twice"
        );
        // Flush the zero, then prove it through a FRESH admission from the
        // reopened native store (no replay): composed extras carry remaining 0.
        world.flush_dirty(&Default::default()).unwrap();
        drop(world);
        let mut reopened = test_world(Some(dir.clone()));
        reopened.ensure_blocking(&wants(&[pos]));
        let stored = reopened
            .chunk(pos)
            .expect("admitted from native store")
            .extras
            .iter()
            .filter_map(|(key, tag)| match (key.as_str(), tag) {
                ("SolarisSimDebt", mc_nbt::Tag::Long(value)) => Some(*value),
                _ => None,
            })
            .next();
        assert_eq!(stored, Some(0), "stored zero anchor survives reopen");
        reopened.set_active_chunks(active(&[pos]), 30);
        let mut entries = Vec::new();
        reopened.journal_debt(&mut entries);
        let images = sim_debt_images(&entries);
        assert_eq!(images.len(), 1);
        assert_eq!(
            images[0].remaining, 18,
            "only the new inactive interval accrues"
        );
        assert!(!reopened.evict_outside(&HashMap::default()).contains(&pos));
        reopened.flush_dirty(&Default::default()).unwrap();
        assert!(reopened.evict_outside(&HashMap::default()).contains(&pos));
    }
}

#[test]
fn stale_ack_does_not_clear_newer_debt() {
    let directory = tempfile::tempdir().unwrap();
    let mut world = test_world(Some(directory.path().to_path_buf()));
    let pos = ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    world.set_active_chunks(active(&[pos]), 10);
    let capture = world.capture_dirty(&Default::default());
    let written = capture.write().unwrap();
    let revision = written
        .iter()
        .find(|(written, _)| *written == pos)
        .map(|(_, revision)| *revision)
        .unwrap();
    // New activation after the capture bumps the storage revision.
    world.set_active_chunks(HashSet::default(), 11);
    world.set_active_chunks(active(&[pos]), 12);
    // Stale ACK (older revision) must not clear.
    world.mark_flushed(&[(pos, revision)]);
    assert!(
        !world.evict_outside(&HashMap::default()).contains(&pos),
        "stale ACK keeps the newer debt fence"
    );
    // Fresh capture + ACK clears.
    let latest = world.capture_dirty(&Default::default()).write().unwrap();
    world.mark_flushed(&latest);
    assert!(world.evict_outside(&HashMap::default()).contains(&pos));
}

/// Active boundary images project through the recovered WAL clock;
/// inactive boundaries freeze at their anchor.
#[test]
fn active_image_projects_clock_and_inactive_boundary_freezes() {
    use crate::journal::{Replay, replay_batches};
    let pos = [7, -2];
    // Active image at 10, then empty batches 11..=20: recovered clock is 20.
    let first = vec![(
        10u64,
        10u64,
        vec![JournalEntry::SimDebt {
            image: SimDebtImage {
                chunk: pos,
                last_sim: 10,
                remaining: 0,
                tick: 10,
                active: true,
            },
        }],
    )];
    let replay: Replay = replay_batches(Default::default(), &first);
    assert_eq!(replay.tick, 10);
    let image = replay.sim_debt[&pos];
    assert!(image.active, "boundary flag survives the WAL");
    // Empty batches 11..=20 advance only the batch clock; the projection
    // applies at recovery time.
    assert_eq!(image.last_sim_at(20), 20);
    assert_eq!(image.last_sim_at(10), 10);
    let tail: Vec<(u64, u64, Vec<JournalEntry>)> =
        (11u64..=20).map(|tick| (tick, tick, Vec::new())).collect();
    let replay2: Replay = replay_batches(replay_batches(Default::default(), &first), &tail);
    assert_eq!(replay2.tick, 20);
    let image2 = replay2.sim_debt[&pos];
    assert_eq!(
        image2.last_sim, 20,
        "active image projects to the recovered tick"
    );
    assert_eq!(image2.tick, 20);
    assert!(image2.active);
    assert_eq!(image2.remaining, 0, "projection never moves remaining");
    // Inactive boundary at 15 freezes at its specified anchor (deactivation
    // journals last_sim 14 at tick 15).
    let frozen = SimDebtImage {
        chunk: pos,
        last_sim: 14,
        remaining: 0,
        tick: 15,
        active: false,
    };
    let replay3: Replay = replay_batches(
        Default::default(),
        &[(15u64, 15u64, vec![JournalEntry::SimDebt { image: frozen }])],
    );
    let tail3: Vec<(u64, u64, Vec<JournalEntry>)> =
        (16u64..=30).map(|tick| (tick, tick, Vec::new())).collect();
    let replay3: Replay = replay_batches(replay3, &tail3);
    assert_eq!(replay3.tick, 30);
    let image3 = replay3.sim_debt[&pos];
    assert_eq!(
        (image3.last_sim, image3.tick),
        (14, 30),
        "inactive boundary freezes at its anchor"
    );
    assert!(!image3.active);
}

fn debt_chunk(extras: Vec<(String, mc_nbt::Tag)>) -> mc_world::Chunk {
    let mut chunk = mc_world::Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.extras = extras;
    chunk
}

#[test]
fn native_debt_absent_pair_is_new_and_valid_pair_is_exact() {
    use crate::debt::{LAST_SIM_KEY, REMAINING_KEY, adopt_native_extras};
    let pos = ChunkPos { x: 0, z: 0 };
    let mut records = HashMap::default();
    adopt_native_extras(
        pos,
        &std::sync::Arc::new(debt_chunk(Vec::new())),
        &mut records,
    )
    .unwrap();
    assert!(records.is_empty());
    let chunk = std::sync::Arc::new(debt_chunk(vec![
        (LAST_SIM_KEY.into(), mc_nbt::Tag::Long(17)),
        (REMAINING_KEY.into(), mc_nbt::Tag::Long(3)),
    ]));
    adopt_native_extras(pos, &chunk, &mut records).unwrap();
    assert_eq!((records[&pos].last_sim, records[&pos].remaining), (17, 3));
}

#[test]
fn native_debt_rejects_partial_wrong_type_negative_and_duplicate_authority() {
    use crate::debt::{LAST_SIM_KEY, REMAINING_KEY, SimDebtRecord, adopt_native_extras};
    use mc_nbt::Tag;
    let pos = ChunkPos { x: 0, z: 0 };
    let cases = [
        vec![(LAST_SIM_KEY.into(), Tag::Long(17))],
        vec![(REMAINING_KEY.into(), Tag::Long(3))],
        vec![
            (LAST_SIM_KEY.into(), Tag::Int(17)),
            (REMAINING_KEY.into(), Tag::Long(3)),
        ],
        vec![
            (LAST_SIM_KEY.into(), Tag::Long(17)),
            (REMAINING_KEY.into(), Tag::String("3".into())),
        ],
        vec![
            (LAST_SIM_KEY.into(), Tag::Long(-1)),
            (REMAINING_KEY.into(), Tag::Long(3)),
        ],
        vec![
            (LAST_SIM_KEY.into(), Tag::Long(17)),
            (REMAINING_KEY.into(), Tag::Long(-1)),
        ],
        vec![
            (LAST_SIM_KEY.into(), Tag::Long(17)),
            (LAST_SIM_KEY.into(), Tag::Long(18)),
            (REMAINING_KEY.into(), Tag::Long(3)),
        ],
        vec![
            (LAST_SIM_KEY.into(), Tag::Long(17)),
            (REMAINING_KEY.into(), Tag::Long(3)),
            (REMAINING_KEY.into(), Tag::Long(4)),
        ],
    ];
    for extras in cases {
        let chunk = std::sync::Arc::new(debt_chunk(extras));
        for existing in [false, true] {
            let mut records = HashMap::default();
            if existing {
                records.insert(
                    pos,
                    SimDebtRecord {
                        last_sim: 19,
                        remaining: 2,
                        dirty: true,
                        active: false,
                    },
                );
            }
            let error = adopt_native_extras(pos, &chunk, &mut records).unwrap_err();
            assert!(matches!(error, crate::CoreError::Storage(_)), "{error}");
            assert!(error.to_string().contains("simulation debt"), "{error}");
            assert_eq!(records.len(), usize::from(existing));
            if existing {
                assert_eq!((records[&pos].last_sim, records[&pos].remaining), (19, 2));
            }
        }
    }
}

#[test]
fn native_debt_projection_overflow_refuses_without_mutating_authority() {
    use crate::debt::{LAST_SIM_KEY, REMAINING_KEY};
    let overflow = i64::MAX as u64 + 1;
    for (last_sim, remaining) in [(overflow, 3), (17, overflow)] {
        let mut chunk = debt_chunk(vec![
            (LAST_SIM_KEY.into(), mc_nbt::Tag::Long(17)),
            (REMAINING_KEY.into(), mc_nbt::Tag::Long(3)),
        ]);
        let before = chunk.extras.clone();
        let error = crate::debt::project(&mut chunk, last_sim, remaining).unwrap_err();
        assert!(matches!(error, crate::CoreError::Storage(_)), "{error}");
        assert!(error.to_string().contains("simulation debt"), "{error}");
        assert_eq!(chunk.extras, before);
    }
}

#[test]
fn persisted_malformed_native_debt_refuses_core_open_without_rewriting_it() {
    use crate::debt::{LAST_SIM_KEY, REMAINING_KEY};
    use mc_nbt::Tag;
    use mc_world::native::{NativeCheckpoint, NativeChunkSnapshot};
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let cases = [
        vec![(LAST_SIM_KEY.into(), Tag::Long(17))],
        vec![(REMAINING_KEY.into(), Tag::Long(3))],
        vec![
            (LAST_SIM_KEY.into(), Tag::Int(17)),
            (REMAINING_KEY.into(), Tag::Long(3)),
        ],
        vec![
            (LAST_SIM_KEY.into(), Tag::Long(17)),
            (REMAINING_KEY.into(), Tag::Int(3)),
        ],
        vec![
            (LAST_SIM_KEY.into(), Tag::Long(-1)),
            (REMAINING_KEY.into(), Tag::Long(3)),
        ],
        vec![
            (LAST_SIM_KEY.into(), Tag::Long(17)),
            (REMAINING_KEY.into(), Tag::Long(-1)),
        ],
    ];
    for extras in cases {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::CoreConfig::new(directory.path().to_path_buf(), 42);
        let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
        let manifest = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
        crate::native_checkpoint::tests::write_manifest(&config, &manifest).unwrap();
        let checkpoint = store.read_checkpoint().unwrap();
        let base = debt_chunk(Vec::new());
        let effective = debt_chunk(extras.clone());
        store
            .commit_checkpoint(
                NativeCheckpoint {
                    tick: checkpoint.tick,
                    previous_tick: checkpoint.tick,
                    chunks: &[NativeChunkSnapshot {
                        dimension: 0,
                        base: &base,
                        effective: &effective,
                    }],
                    entities: &[],
                    players: &[],
                    globals: &checkpoint.globals,
                },
                &registry.blocks,
                &registry.items,
            )
            .unwrap();
        drop(store);
        // A replayed block forces startup to admit this native chunk.
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        writer
            .append(
                checkpoint.tick + 1,
                checkpoint.tick + 1,
                &[JournalEntry::Block {
                    pos: [0, 64, 0],
                    state: registry.states.air,
                }],
            )
            .unwrap();
        writer.flush().unwrap();
        drop(writer);
        let error = crate::Core::open(config.clone()).unwrap_err();
        assert!(matches!(error, crate::CoreError::Storage(_)), "{error}");
        assert!(error.to_string().contains("simulation debt"), "{error}");
        let reopened = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
        assert_eq!(reopened.read_checkpoint().unwrap().tick, checkpoint.tick);
        let generator =
            mc_worldgen::TerrainGenerator::new(42, std::sync::Arc::clone(&registry.blocks));
        let stored = reopened
            .read_chunk(
                0,
                ChunkPos { x: 0, z: 0 },
                &registry.blocks,
                &registry.items,
                &registry.chest_loot,
                &generator,
            )
            .unwrap()
            .unwrap();
        for authority in extras {
            assert!(
                stored.extras.contains(&authority),
                "refusal must preserve {authority:?}"
            );
        }
    }
}

#[test]
fn checkpoint_debt_outside_native_range_refuses_core_open() {
    let overflow = i64::MAX as u64 + 1;
    for (last_sim, remaining) in [(overflow, 3), (17, overflow)] {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::CoreConfig::new(directory.path().to_path_buf(), 42);
        let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
        let mut manifest = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
        let image = SimDebtImage {
            chunk: [0, 0],
            last_sim,
            remaining,
            tick: manifest.tick.max(1),
            active: false,
        };
        manifest.debt.push(image);
        crate::native_checkpoint::tests::write_manifest(&config, &manifest).unwrap();
        drop(store);
        let error = crate::Core::open(config.clone()).unwrap_err();
        assert!(matches!(error, crate::CoreError::Storage(_)), "{error}");
        assert!(error.to_string().contains("simulation debt"), "{error}");
        assert_eq!(
            crate::native_checkpoint::tests::read_manifest(&config)
                .unwrap()
                .debt,
            vec![image]
        );
    }
}
