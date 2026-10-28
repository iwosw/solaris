//! Sibling `*_tests.rs` (AGENTS.md rule: no inline `#[test]` in `world.rs`).
//! Dirty-chunk retention: eviction keeps unflushed edits; container marks
//! write through flush (resident and absent chunks alike).

use rustc_hash::FxHashMap as HashMap;
use std::sync::Arc;

use crate::world::{ChunkCapture, ChunkWorld, SectionMatchCache, chunks_in_radius};
use mc_world::native::{NativeCheckpoint, NativeDomainRecord, NativeStore};

/// Standalone native chunk fixtures, not serving/recovery world descriptors.
pub(crate) fn native_store(
    directory: &std::path::Path,
    registry: &crate::registry::CoreRegistry,
) -> Arc<NativeStore> {
    std::fs::create_dir_all(directory.join("native")).unwrap();
    let path = directory.join("native/state.redb");
    if path.exists() {
        return Arc::new(NativeStore::open(&path, [1; 32]).unwrap());
    }
    let store = Arc::new(NativeStore::create(&path, [1; 32]).unwrap());
    let manifest = serde_json::json!({
        "tick":0,"dimensions":{},"end_fight":null,"raids":null,"tickets":{},"players":{},
        "world_spawn":[0,64,0],"world_spawn_rotation":[0,0],"generated_entity_claims":[],
        "climate":{},"border":{},"difficulty":"Normal","gamerules":{},
        "block_ticks":{"next_sequence":0,"chunks":[]},
        "fluid_ticks":{"next_sequence":0,"covered":[],"due":[]},
        "domain_versions":{"climate":1,"gamerules":1,"block_ticks":1,"fluid_ticks":1,
            "claims":1,"tickets":1,"raids":1,"end_fight":1}
    });
    let mut checkpoint = manifest.clone();
    checkpoint["dimensions"] = serde_json::json!({
        "minecraft:the_nether":manifest.clone(),"minecraft:the_end":manifest
    });
    let dimensions = serde_json::json!({
        "0":{"name":"minecraft:overworld","min_y":-64,"height":384,"generator":"test","seed":42,"revision":1},
        "1":{"name":"minecraft:the_nether","min_y":0,"height":256,"generator":"test","seed":42,"revision":1},
        "2":{"name":"minecraft:the_end","min_y":0,"height":256,"generator":"test","seed":42,"revision":1}
    });
    let mut globals = std::collections::BTreeMap::new();
    for (key, value) in [
        ("checkpoint", checkpoint),
        ("dimensions", dimensions),
        (
            "generation",
            serde_json::json!({"identity":vec![1;32],"base":"fixture","generators":{}}),
        ),
    ] {
        globals.insert(
            key.into(),
            NativeDomainRecord {
                version: 1,
                captured_tick: 0,
                payload: serde_json::to_vec(&value).unwrap(),
            },
        );
    }
    let mut nbt = Vec::new();
    mc_nbt::write_named(&mut nbt, "", &mc_nbt::Tag::Compound(Vec::new())).unwrap();
    globals.insert(
        "bossbars".into(),
        crate::native_checkpoint::tag_record(0, &crate::bossbar::BossBars::default().to_nbt())
            .unwrap(),
    );
    for key in ["scoreboard", "level_metadata", "conversion_extras"] {
        globals.insert(
            key.into(),
            NativeDomainRecord {
                version: 1,
                captured_tick: 0,
                payload: nbt.clone(),
            },
        );
    }
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 0,
                previous_tick: 0,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &globals,
            },
            &registry.blocks,
            &registry.items,
        )
        .unwrap();
    store
}

pub(crate) fn write_capture(
    capture: &ChunkCapture,
) -> Result<Vec<(mc_world::ChunkPos, u64)>, crate::config::CoreError> {
    write_capture_io(capture).map(|(written, _)| written)
}

/// [`write_capture`] plus the committed record counters of the native transaction.
pub(crate) fn write_capture_io(
    capture: &ChunkCapture,
) -> Result<
    (
        Vec<(mc_world::ChunkPos, u64)>,
        mc_world::native::NativeCheckpointIo,
    ),
    crate::config::CoreError,
> {
    let written = capture.written();
    // Reject a frozen load error even for an empty capture. Resolve chunks
    // exactly once, inside the atomic streaming commit below.
    if let Some(error) = capture.load_error() {
        return Err(crate::config::CoreError::Storage(error.to_string()));
    }
    if written.is_empty() {
        return Ok((written, Default::default()));
    }
    let Some(store) = capture.store() else {
        return Ok((Vec::new(), Default::default()));
    };
    let storage =
        |error: mc_world::native::NativeError| crate::config::CoreError::Storage(error.to_string());
    let mut image = store.read_checkpoint().map_err(storage)?;
    let tick = image.tick + 1;
    for record in image.globals.values_mut() {
        record.captured_tick = tick;
    }
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest["tick"] = tick.into();
    for child in manifest["dimensions"].as_object_mut().unwrap().values_mut() {
        child["tick"] = tick.into();
    }
    image.globals.get_mut("checkpoint").unwrap().payload = serde_json::to_vec(&manifest).unwrap();
    let entities = image.entities.iter().map(Ok);
    let io = store
        .commit_checkpoint_stream(
            NativeCheckpoint {
                tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &image.globals,
            },
            |write| {
                capture
                    .visit_native(|snapshot| {
                        write(snapshot)
                            .map_err(|error| crate::config::CoreError::Storage(error.to_string()))
                    })
                    .map_err(|error| mc_world::native::NativeError::Source(error.to_string()))
            },
            entities,
            capture.registry(),
            capture.items(),
        )
        .map_err(storage)?;
    Ok((written, io))
}

fn wants(positions: &[mc_world::ChunkPos]) -> HashMap<mc_world::ChunkPos, i32> {
    positions.iter().map(|pos| (*pos, 0)).collect()
}
#[cfg(test)]
fn world_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
}

pub(crate) fn test_world(world_dir: Option<std::path::PathBuf>) -> ChunkWorld {
    let registry =
        crate::registry::CoreRegistry::load(Some(&world_root())).expect("registry loads");
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(
        mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks)),
    );
    match world_dir {
        Some(directory) => ChunkWorld::new_native(
            42,
            native_store(&directory, &registry),
            0,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [0, 64, 0],
            mc_world::OVERWORLD_GEOMETRY,
        ),
        None => ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [0, 64, 0],
            mc_world::OVERWORLD_GEOMETRY,
        ),
    }
}

#[test]
fn inhabited_time_deferred_journal_capture_coalesces_absolute_postimages() {
    let mut world = test_world(None);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    for tick in 1..=5 {
        if tick != 3 {
            world.tick_inhabited_time(pos, tick);
        }
        world.finish_inhabited_tick(tick);
    }
    let mut entries = Vec::new();
    world.journal_inhabited_time(&mut entries);
    assert_eq!(
        entries.len(),
        1,
        "only the latest absolute post-image survives"
    );
    let crate::journal::JournalEntry::InhabitedTime { image } = entries[0] else {
        panic!("inhabited boundary");
    };
    assert_eq!(image.tick, 5, "journal clock, not an earlier setup tick");
    assert_eq!(image.value, 4);
    assert!(image.active);
    let replay = crate::journal::replay_batches(Default::default(), &[(5, 5, entries)]);
    assert_eq!(
        replay.inhabited_time[&[0, 0]].value_at(replay.tick),
        world.inhabited_time(pos)
    );
}

#[test]
fn inhabited_time_checkpoint_preserves_imported_value_and_fences_stale_ack() {
    let directory = tempfile::tempdir().unwrap();
    let mut world = test_world(Some(directory.path().to_path_buf()));
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
    chunk
        .extras
        .push(("InhabitedTime".into(), mc_nbt::Tag::Long(900)));
    world
        .source
        .native
        .as_ref()
        .unwrap()
        .import_chunk(0, chunk, &world.source.registry, &world.source.items)
        .unwrap();
    let held = Arc::clone(&world.chunks[&pos]);
    let wire = world.chunk_revision(pos.x, pos.z);
    let storage = world.storage_revision(pos);
    for tick in 1..=20 {
        world.tick_inhabited_time(pos, tick);
        world.tick_inhabited_time(pos, tick);
        world.finish_inhabited_tick(tick);
    }
    assert_eq!(world.inhabited_time(pos), 920);
    assert!(Arc::ptr_eq(&held, &world.chunks[&pos]), "no per-tick COW");
    assert_eq!(world.storage_revision(pos), storage);
    assert_eq!(world.chunk_revision(pos.x, pos.z), wire);
    assert!(!world.evict_outside(&HashMap::default()).contains(&pos));
    let capture = world.capture_dirty(&Default::default());
    world.tick_inhabited_time(pos, 21);
    world.finish_inhabited_tick(21);
    world.mark_flushed(&capture.write().unwrap());
    assert!(!world.evict_outside(&HashMap::default()).contains(&pos));
    assert_eq!(world.inhabited_time(pos), 921);
    world.finish_inhabited_tick(22);
    world.flush_dirty(&Default::default()).unwrap();
    assert_eq!(world.chunk_revision(pos.x, pos.z), wire);
    assert!(world.evict_outside(&HashMap::default()).contains(&pos));
    drop(capture);
    drop(world);
    let mut reopened = test_world(Some(directory.path().to_path_buf()));
    reopened.ensure_blocking(&wants(&[pos]));
    assert_eq!(reopened.inhabited_time(pos), 921);
}

#[test]
fn absent_inhabited_capture_retries_until_ack_without_loading_or_residency() {
    let directory = tempfile::tempdir().unwrap();
    let mut world = test_world(Some(directory.path().to_path_buf()));
    let pos = mc_world::ChunkPos { x: 3, z: -2 };
    let replay = crate::journal::Replay {
        tick: 20,
        inhabited_time: [(
            [pos.x, pos.z],
            crate::journal::InhabitedTimeImage {
                chunk: [pos.x, pos.z],
                value: 50,
                tick: 10,
                active: true,
            },
        )]
        .into(),
        ..Default::default()
    };
    world.restore_inhabited_time(&replay, None);
    let capture = world.capture_dirty(&Default::default());
    assert!(capture.chunks.is_empty());
    assert_eq!(capture.deferred.len(), 1);
    assert!(!world.has_chunk(pos.x, pos.z));
    assert_eq!(world.chunk_revision(pos.x, pos.z), u64::MAX);
    assert!(world.generated.is_empty());
    world.finish_inhabited_tick(21);
    let retry = world.capture_dirty(&Default::default());
    assert_eq!(retry.written(), capture.written());
    let mut visits = 0;
    retry
        .visit_native(|snapshot| {
            visits += 1;
            assert_eq!(snapshot.effective.inhabited_time(), 60);
            Ok(())
        })
        .unwrap();
    assert_eq!(visits, 1);
    assert!(!world.has_chunk(pos.x, pos.z), "preparation never admits");
    world.mark_flushed(&capture.write().unwrap());
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty(),
        "only a successful revision-matching ACK clears the absent image"
    );
    world.ensure_blocking(&wants(&[pos]));
    assert_eq!(world.chunk(pos).unwrap().inhabited_time(), 60);
    assert_eq!(world.inhabited_time(pos), 60);
    assert!(world.evict_outside(&Default::default()).contains(&pos));
    world.ensure_blocking(&wants(&[pos]));
    assert_eq!(world.chunk(pos).unwrap().inhabited_time(), 60);
}

/// Mine a block, leave its simulation radius before any flush, return:
/// without dirty retention the edit is lost (old block regenerates).
#[test]
fn eviction_retains_dirty_chunks_until_flushed() {
    let mut world = test_world(None);
    let center = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&chunks_in_radius(center, 1)));
    let before = world.block_at(4, 80, 4);
    // Authoritative edit + fold (dirty chunk).
    let air = mc_world::BlockStateId(
        crate::registry::CoreRegistry::load(Some(&world_root()))
            .expect("r")
            .states
            .air,
    );
    let _ = (before, air);
    let registry =
        crate::registry::CoreRegistry::load(Some(&world_root())).expect("registry loads");
    let dirt = registry.states.dirt;
    world.set_block(4, 80, 4, dirt);
    world.fold_overlay();
    assert_eq!(world.block_at(4, 80, 4), Some(dirt));
    // Leave the radius entirely: eviction must retain the dirty chunk.
    let far = mc_world::ChunkPos { x: 100, z: 100 };
    let evicted = world.evict_outside(&wants(&chunks_in_radius(far, 1)));
    assert!(
        !evicted.contains(&mc_world::ChunkPos { x: 0, z: 0 }),
        "dirty chunk survives eviction until flushed"
    );
    assert_eq!(world.block_at(4, 80, 4), Some(dirt), "edit still legible");
}

/// Container storage change marks the chunk dirty even though no block
/// changed: `mark_block_entity_dirty` flips the resident flag.
#[test]
fn container_change_marks_chunk_dirty() {
    let mut world = test_world(None);
    world.ensure_blocking(&wants(&[mc_world::ChunkPos { x: 0, z: 0 }]));
    world.mark_block_entity_dirty(mc_world::BlockPos { x: 4, y: 64, z: 4 });
    let evicted = world.evict_outside(&HashMap::default());
    assert!(
        !evicted.contains(&mc_world::ChunkPos { x: 0, z: 0 }),
        "container-marked chunk survives eviction"
    );
}

/// A container edit after a capture is not cleared by that capture's
/// acknowledgement: the chunk stays dirty and survives eviction until a
/// later checkpoint writes it.
#[test]
fn container_edit_after_capture_survives_stale_ack() {
    let dir = std::env::temp_dir().join(format!("mc-world-stale-ack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut world = test_world(Some(dir.clone()));
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    world.mark_block_entity_dirty(mc_world::BlockPos { x: 4, y: 64, z: 4 });
    let capture = world.capture_dirty(&Default::default());
    world.mark_block_entity_dirty(mc_world::BlockPos { x: 4, y: 64, z: 4 });
    let written = capture.write().expect("native checkpoint");
    world.mark_flushed(&written);
    let evicted = world.evict_outside(&HashMap::default());
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !evicted.contains(&pos),
        "edit after the capture stays dirty"
    );
}

#[test]
fn private_container_storage_keeps_wire_revision_and_fences_checkpoint_ack() {
    let dir = std::env::temp_dir().join(format!("private-storage-{}", uuid::Uuid::new_v4()));
    let mut world = test_world(Some(dir.clone()));
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    let block = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.ensure_blocking(&wants(&[pos]));
    let before = world.snapshot()[0].revision;
    world.mark_block_entity_dirty(block);
    let capture = world.capture_dirty(&Default::default());
    world.mark_block_entity_dirty(block);
    assert_eq!(
        world.snapshot()[0].revision,
        before,
        "private custody is not chunk wire content"
    );
    world.mark_flushed(&capture.write().unwrap());
    assert!(!world.evict_outside(&HashMap::default()).contains(&pos));
    let latest = world.capture_dirty(&Default::default()).write().unwrap();
    world.mark_flushed(&latest);
    assert!(world.evict_outside(&HashMap::default()).contains(&pos));
    drop(world);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A clean chunk evicted moments ago is resident again on the very next
/// `ensure` (no off-tick load), with identical content and a new revision.
#[test]
fn recently_evicted_chunk_returns_without_a_load() {
    let mut world = test_world(None);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    let revision = |world: &mut ChunkWorld| {
        world
            .snapshot()
            .iter()
            .find(|entry| entry.pos == pos)
            .map(|entry| entry.revision)
    };
    let before = world.block_at(4, 60, 4);
    let first = revision(&mut world).expect("resident wire revision");
    assert_eq!(world.evict_outside(&HashMap::default()), vec![pos]);
    assert_eq!(
        world.ensure(&wants(&[pos])),
        vec![pos],
        "back in the same call"
    );
    assert_eq!(world.block_at(4, 60, 4), before);
    let now = revision(&mut world).expect("readmitted wire revision");
    assert!(now > first, "readmission must never reuse a wire revision");
    assert_eq!(world.chunk_revision(pos.x, pos.z), now);
    assert_ne!(
        now,
        u64::MAX,
        "resident content is not the absence sentinel"
    );
}

#[test]
fn quarantined_chunk_has_no_wire_revision() {
    let mut world = test_world(None);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    assert_ne!(world.chunk_revision(pos.x, pos.z), u64::MAX);
    world.reject_resident(pos, "invalid fixture metadata".into());
    assert!(!world.has_chunk(pos.x, pos.z));
    assert_eq!(world.chunk_revision(pos.x, pos.z), u64::MAX);
    assert_eq!(world.storage_revision(pos), u64::MAX);
    assert!(world.snapshot().is_empty());
}

/// Residency gate for the single debt-flag `is_active`: every resident
/// removal leaves no stale activity behind, and re-admission with demand
/// reactivates through the normal clock authority.
#[test]
fn quarantined_and_evicted_residents_are_immediately_inactive() {
    use rustc_hash::FxHashSet as HashSet;
    // Native-backed: the activation checkpoint must ACK before clean eviction.
    let directory = tempfile::tempdir().unwrap();
    let mut world = test_world(Some(directory.path().to_path_buf()));
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    assert!(
        !world.is_active(pos.x, pos.z),
        "strict empty default: residency is not activity"
    );
    // Admitted + activated.
    let mut active = HashSet::default();
    active.insert(pos);
    world.set_active_chunks(active.clone(), 10);
    assert!(world.is_active(pos.x, pos.z));
    // Evicted: authority removed, immediately inactive.
    let capture = world.capture_dirty(&Default::default());
    world.mark_flushed(&capture.write().unwrap());
    assert_eq!(world.evict_outside(&HashMap::default()), vec![pos]);
    assert!(!world.has_chunk(pos.x, pos.z));
    assert!(!world.is_active(pos.x, pos.z));
    // Re-admission without demand stays resident-but-inactive; a repeated
    // demand reactivates through the normal activation authority.
    // (Quarantine would fence `failed` and refuse re-admission, so the
    // reinsertion leg runs on the eviction path.)
    world.set_active_chunks(HashSet::default(), 11);
    world.ensure_blocking(&wants(&[pos]));
    assert!(world.has_chunk(pos.x, pos.z));
    world.refresh_active_clock(12);
    assert!(
        !world.is_active(pos.x, pos.z),
        "re-admission is never implicitly active"
    );
    world.set_active_chunks(active, 13);
    assert!(world.is_active(pos.x, pos.z));
    assert_eq!(world.simulation_interval(pos, 12, 13), Some((12, 13)));
    // Quarantined while active: immediately inactive, with the frozen
    // deactivation boundary preserved for the journal.
    let before = world.active_revision();
    world.reject_resident(pos, "invalid fixture metadata".into());
    assert!(!world.has_chunk(pos.x, pos.z));
    assert!(!world.is_active(pos.x, pos.z));
    assert!(
        world.active_revision() > before,
        "quarantine moves the membership fence"
    );
    let mut entries = Vec::new();
    world.journal_debt(&mut entries);
    let images: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            crate::journal::JournalEntry::SimDebt { image } => Some(*image),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 1, "quarantine emits the frozen boundary");
    assert!(!images[0].active, "deactivation freezes the live clock");
    assert_eq!((images[0].last_sim, images[0].tick), (13, 13));
}

/// Quarantine of a resident-but-inactive chunk keeps the absent-authority
/// `None` fallback and never invents a clock boundary image.
#[test]
fn quarantined_inactive_resident_emits_no_clock_boundary() {
    let mut world = test_world(None);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    assert!(!world.is_active(pos.x, pos.z));
    world.reject_resident(pos, "invalid fixture metadata".into());
    assert!(!world.has_chunk(pos.x, pos.z));
    assert!(!world.is_active(pos.x, pos.z));
    let mut entries = Vec::new();
    world.journal_debt(&mut entries);
    assert!(
        entries.is_empty(),
        "no activation means no deactivation boundary"
    );
}

/// A delayed generated-base save never overwrites checkpointed gameplay.
#[test]
fn generated_chunks_persist_without_overwriting_edits() {
    let dir = std::env::temp_dir().join(format!("mc-world-gen-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    let dirt = crate::registry::CoreRegistry::load(Some(&world_root()))
        .expect("registry")
        .states
        .dirt;
    let mut world = test_world(Some(dir.clone()));
    world.ensure_blocking(&wants(&[pos]));
    let generated = world.take_generated(0).expect("generated chunk queued");
    // Edit and checkpoint the chunk BEFORE the generated batch is written.
    world.set_block(4, 100, 4, dirt);
    world.fold_overlay();
    world.flush_dirty(&Default::default()).expect("checkpoint");
    generated.fill_generated().expect("fill immutable bases");
    drop(world);
    let reopened = test_world(Some(dir.clone()));
    let stored = reopened.source.load_native(pos).expect("readable");
    let stored = stored.expect("chunk in native store");
    assert_eq!(stored.get_block(4, 100, 4).map(|b| b.0), Some(dirt));
    drop(reopened);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A generated chunk alone lands as an immutable native base.
#[test]
fn generated_chunk_fills_its_empty_slot() {
    let dir = std::env::temp_dir().join(format!("mc-world-gen-fill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let pos = mc_world::ChunkPos { x: 3, z: -2 };
    let mut world = test_world(Some(dir.clone()));
    world.ensure_blocking(&wants(&[pos]));
    world
        .take_generated(0)
        .expect("queued")
        .fill_generated()
        .expect("fill");
    let stored = world.source.load_native(pos);
    assert!(
        stored.expect("readable").is_some(),
        "loaded from native store next time"
    );
    assert!(!dir.join("region").exists(), "no runtime Anvil writes");
    drop(world);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Snapshot revisions: a block edit or a reload gives the chunk a new
/// revision, an untouched chunk keeps its own (edge caches key on this).
#[test]
fn chunk_revision_changes_exactly_with_content() {
    let dir = std::env::temp_dir().join(format!("mc-world-revision-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut world = test_world(Some(dir.clone()));
    let (a, b) = (
        mc_world::ChunkPos { x: 0, z: 0 },
        mc_world::ChunkPos { x: 1, z: 0 },
    );
    world.ensure_blocking(&wants(&[a, b]));
    let revision = |world: &mut ChunkWorld, pos| {
        world
            .snapshot()
            .iter()
            .find(|entry| entry.pos == pos)
            .map(|entry| entry.revision)
            .expect("resident")
    };
    let empty: std::collections::BTreeMap<(i32, i32, i32), crate::journal::ReplayBlockEntity> =
        std::collections::BTreeMap::new();
    let (a0, b0) = (revision(&mut world, a), revision(&mut world, b));
    assert_ne!(a0, b0);
    let dirt = crate::registry::CoreRegistry::load(Some(&world_root()))
        .expect("r")
        .states
        .dirt;
    world.set_block(4, 80, 4, dirt);
    world.fold_overlay();
    assert_ne!(revision(&mut world, a), a0, "edit bumps revision");
    assert_eq!(
        revision(&mut world, b),
        b0,
        "untouched chunk keeps revision"
    );
    world.flush_dirty(&empty).expect("flush");
    assert_eq!(
        revision(&mut world, b),
        b0,
        "saving without block entities keeps it"
    );
    let evicted = world.evict_outside(&HashMap::default());
    assert!(evicted.contains(&b), "flushed chunk evicts");
    world.ensure_blocking(&wants(&[a, b]));
    assert_ne!(revision(&mut world, b), b0, "reload gets a new revision");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Absent-chunk container marks flush through load-or-generate to native.
#[test]
fn absent_container_mark_flushes_through_generate() {
    let dir = std::env::temp_dir().join(format!("mc-world-absent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut world = test_world(Some(dir.clone()));
    world.mark_block_entity_dirty(mc_world::BlockPos { x: 4, y: 64, z: 4 });
    let empty: std::collections::BTreeMap<(i32, i32, i32), crate::journal::ReplayBlockEntity> =
        std::collections::BTreeMap::new();
    let flushed = world.flush_dirty(&empty).expect("flush");
    assert_eq!(flushed, 1, "absent marked chunk writes one chunk");
    let stored = world
        .source
        .load_native(mc_world::ChunkPos { x: 0, z: 0 })
        .unwrap();
    assert!(stored.is_some(), "absent custody persisted in native store");
    assert!(!dir.join("region").exists(), "no runtime Anvil writes");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A block write for a chunk that is not resident (journal replay before
/// the chunk loads) survives a checkpoint flush: a reopened world reads it.
#[test]
fn checkpoint_persists_writes_for_nonresident_chunks() {
    let dir = std::env::temp_dir().join(format!("mc-world-homeless-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let registry =
        crate::registry::CoreRegistry::load(Some(&world_root())).expect("registry loads");
    let gold = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:gold_block").expect("id"))
        .expect("gold block")
        .default
        .0;
    let empty: std::collections::BTreeMap<(i32, i32, i32), crate::journal::ReplayBlockEntity> =
        std::collections::BTreeMap::new();
    let mut world = test_world(Some(dir.clone()));
    world.set_block(100, 70, 100, gold);
    world.fold_overlay();
    assert!(
        !world.overlay_is_empty(),
        "chunk absent: write stays homeless"
    );
    world.flush_dirty(&empty).expect("flush");
    assert!(world.overlay_is_empty(), "flush folded the homeless write");
    drop(world);
    let mut reopened = test_world(Some(dir.clone()));
    reopened.ensure_blocking(&wants(&[mc_world::ChunkPos { x: 6, z: 6 }]));
    assert_eq!(reopened.block_at(100, 70, 100), Some(gold));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Generator whose first call blocks until released (the off-tick worker's
/// load); later calls (the checkpoint's synchronous load) pass through.
struct PausedFirstGeneration {
    inner: mc_worldgen::TerrainGenerator,
    calls: std::sync::atomic::AtomicUsize,
    started: std::sync::mpsc::Sender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl mc_world::ChunkGenerator for PausedFirstGeneration {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            self.started.send(()).expect("test alive");
            self.release
                .lock()
                .expect("release lock")
                .recv()
                .expect("released");
        }
        self.inner.generate(pos)
    }
}

/// An off-tick load already running when a checkpoint materializes and
/// saves the same chunk must never land later (after eviction and re-want)
/// with its pre-checkpoint data.
#[test]
fn late_load_never_reverts_a_checkpointed_edit() {
    let dir = std::env::temp_dir().join(format!("mc-world-late-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let registry =
        crate::registry::CoreRegistry::load(Some(&world_root())).expect("registry loads");
    let gold = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:gold_block").expect("id"))
        .expect("gold block")
        .default
        .0;
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(PausedFirstGeneration {
        inner: mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks)),
        calls: std::sync::atomic::AtomicUsize::new(0),
        started: started_tx,
        release: std::sync::Mutex::new(release_rx),
    });
    let mut world = ChunkWorld::new_native(
        42,
        native_store(&dir, &registry),
        0,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let empty: std::collections::BTreeMap<(i32, i32, i32), crate::journal::ReplayBlockEntity> =
        std::collections::BTreeMap::new();
    let want = wants(&[mc_world::ChunkPos { x: 0, z: 0 }]);
    world.ensure(&want);
    started_rx.recv().expect("worker took the load");
    world.set_block(4, 80, 4, gold);
    world.fold_overlay();
    world.flush_dirty(&empty).expect("checkpoint flush");
    world.evict_outside(&HashMap::default());
    release_tx.send(()).expect("release worker");
    world.ensure_blocking(&want);
    assert_eq!(world.block_at(4, 80, 4), Some(gold), "late load discarded");
    world.flush_dirty(&empty).expect("second flush");
    drop(world);
    let mut reopened = test_world(Some(dir.clone()));
    reopened.ensure_blocking(&want);
    assert_eq!(reopened.block_at(4, 80, 4), Some(gold), "edit on disk");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn queue_coverage_requires_unloaded_clean_column() {
    let mut world = test_world(None);
    assert!(world.unloaded_clean(-1, -1));
    world.mark_block_entity_dirty(mc_world::BlockPos {
        x: -1,
        y: 64,
        z: -1,
    });
    assert!(
        !world.unloaded_clean(-1, -1),
        "absent dirty custody is not evictable"
    );
    assert!(world.unloaded_clean(0, 0));
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    assert!(
        !world.unloaded_clean(0, 0),
        "resident column retains queue coverage"
    );
    let registry = crate::registry::CoreRegistry::load(Some(&world_root())).unwrap();
    world.set_block(1, 80, 1, registry.states.dirt);
    assert!(
        !world.unloaded_clean(0, 0),
        "unfolded overlay is not evictable"
    );
    world.fold_overlay();
    world.evict_outside(&HashMap::default());
    assert!(
        !world.unloaded_clean(0, 0),
        "dirty folded content is not evictable"
    );
    world.mark_flushed(&[(pos, world.storage_revision(pos))]);
    world.evict_outside(&HashMap::default());
    assert!(
        !world.unloaded_clean(0, 0),
        "a cached pre-checkpoint copy must retain queue coverage"
    );
    world.recent.take(pos);
    assert!(
        world.unloaded_clean(0, 0),
        "dropping the cache copy releases coverage"
    );
    world.in_flight.insert(pos, world.revision_now() + 1);
    assert!(
        !world.unloaded_clean(0, 0),
        "an accepted old-image worker ticket must retain queue coverage"
    );
    world.in_flight.remove(&pos);
    assert!(
        world.unloaded_clean(0, 0),
        "discarding the ticket releases coverage"
    );
}

#[test]
fn deferred_checkpoint_ack_keeps_coverage_for_old_worker_and_recent_images() {
    for admit_before_commit in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let mut world = test_world(Some(directory.path().to_path_buf()));
        let pos = mc_world::ChunkPos { x: 0, z: 0 };
        let cell = mc_world::BlockPos { x: 1, y: 100, z: 1 };
        let mut stored = mc_world::Chunk::empty(pos, world._air, world._biome.clone());
        stored.load_scheduled_block_ticks(vec![mc_world::ScheduledBlockTick::new(
            cell,
            mc_data::Identifier::parse("minecraft:stone").unwrap(),
            1,
            0,
        )]);
        stored.load_scheduled_fluid_ticks(vec![mc_world::ScheduledFluidTick::new(
            cell,
            mc_data::Identifier::parse("minecraft:water").unwrap(),
            1,
            0,
        )]);
        world
            .source
            .native
            .as_ref()
            .unwrap()
            .import_chunk(0, &stored, &world.source.registry, &world.source.items)
            .unwrap();
        let want = wants(&[pos]);
        world.ensure_blocking(&want);
        world.set_active_chunks([pos].into_iter().collect(), 100);
        // Activation adds a durable debt boundary; acknowledge setup before
        // testing eviction of the independently retained queue postimages.
        world.flush_dirty(&Default::default()).unwrap();
        let mut blocks = crate::block_ticks::BlockTicks::default();
        let mut fluids = crate::fluids::FluidTicks::default();
        blocks.hydrate_chunk(world.chunk(pos).unwrap(), 100);
        fluids.hydrate(world.chunk(pos).unwrap(), 100);
        assert_eq!(blocks.drain_loaded(101, 1, &world).len(), 1);
        assert_eq!(fluids.drain(101, &world).len(), 1);
        let mut entries = Vec::new();
        blocks.journal_entries(&mut entries);
        fluids.journal(&mut entries);
        assert!(world.evict_outside(&Default::default()).contains(&pos));
        let absent_blocks = blocks.checkpoint(&mut world, 102);
        let absent_fluids = fluids.checkpoint(&mut world, 102);
        let mut capture = world.capture_dirty(&Default::default());
        world.capture_scheduled_block_ticks(&mut capture, absent_blocks);
        world.capture_scheduled_fluid_ticks(&mut capture, absent_fluids);
        let block_image = blocks.image();
        let fluid_image = fluids.image();
        assert!(!world.has_chunk(pos.x, pos.z));
        assert!(!world.recent.chunks.contains_key(&pos));

        // Complete a fresh worker read before the empty native checkpoint
        // commits. Keep its actual accepted ticket until admission, so both
        // the resident-before-commit and result-after-ACK races are covered.
        assert!(world.ensure(&want).is_empty());
        let mut old_load = Some(world.loader.recv());
        let (loaded_pos, _, result) = old_load.as_ref().unwrap();
        assert_eq!(*loaded_pos, pos);
        assert_eq!(result.as_ref().unwrap().0.scheduled_block_ticks().len(), 1);
        assert_eq!(result.as_ref().unwrap().0.scheduled_fluid_ticks().len(), 1);
        if admit_before_commit {
            let (loaded_pos, ticket, result) = old_load.take().unwrap();
            assert!(world.accept(loaded_pos, ticket, result, &want));
        }
        world.mark_flushed(&capture.write().unwrap());
        blocks.acknowledge_flushed(&block_image);
        fluids.acknowledge_flushed(&fluid_image);
        blocks.evict_flushed(|x, z| world.unloaded_clean(x, z));
        fluids.evict_flushed(|x, z| world.unloaded_clean(x, z));
        assert_eq!(blocks.image().chunks.len(), 1);
        assert_eq!(fluids.image().covered.len(), 1);
        if !admit_before_commit {
            let (loaded_pos, ticket, result) = old_load.take().unwrap();
            assert!(world.accept(loaded_pos, ticket, result, &want));
        }
        blocks.hydrate_chunk(world.chunk(pos).unwrap(), 103);
        fluids.hydrate(world.chunk(pos).unwrap(), 103);
        assert!(blocks.drain_loaded(104, 10, &world).is_empty());
        assert!(fluids.drain(104, &world).is_empty());
        assert!(world.evict_outside(&Default::default()).contains(&pos));
        blocks.evict_flushed(|x, z| world.unloaded_clean(x, z));
        fluids.evict_flushed(|x, z| world.unloaded_clean(x, z));
        assert_eq!(
            blocks.image().chunks.len(),
            1,
            "recent copy retains block tombstone"
        );
        assert_eq!(
            fluids.image().covered.len(),
            1,
            "recent copy retains fluid tombstone"
        );

        // Revisit the real cached old image, not the now-empty native image.
        world.ensure_blocking(&want);
        let cached = world.chunk(pos).unwrap();
        assert_eq!(cached.scheduled_block_ticks().len(), 1);
        assert_eq!(cached.scheduled_fluid_ticks().len(), 1);
        blocks.hydrate_chunk(cached, 105);
        fluids.hydrate(cached, 105);
        assert!(blocks.drain_loaded(106, 10, &world).is_empty());
        assert!(fluids.drain(106, &world).is_empty());

        // Once the bounded cache drops that last reusable old image, normal
        // coverage retirement resumes; the next native load is really empty.
        world.evict_outside(&Default::default());
        drop(world.recent.take(pos));
        blocks.evict_flushed(|x, z| world.unloaded_clean(x, z));
        fluids.evict_flushed(|x, z| world.unloaded_clean(x, z));
        assert!(blocks.image().chunks.is_empty());
        assert!(fluids.image().covered.is_empty());
        world.ensure_blocking(&want);
        let committed = world.chunk(pos).unwrap();
        assert!(committed.scheduled_block_ticks().is_empty());
        assert!(committed.scheduled_fluid_ticks().is_empty());
        blocks.hydrate_chunk(committed, 107);
        fluids.hydrate(committed, 107);
        assert!(blocks.drain_loaded(108, 10, &world).is_empty());
        assert!(fluids.drain(108, &world).is_empty());
    }
}

#[test]
fn tick_admission_shares_one_nearest_first_budget_with_recent_hits() {
    let mut world = test_world(None);
    let far = mc_world::ChunkPos { x: 2, z: 0 };
    let near = mc_world::ChunkPos { x: 0, z: 0 };
    let middle = mc_world::ChunkPos { x: 1, z: 0 };
    let want = [(far, 16), (near, 0), (middle, 4)].into_iter().collect();
    let result = world.source.load_or_generate(middle).unwrap();
    world.in_flight.insert(middle, 100);
    world.ready.push((middle, 100, Ok(result)));
    for pos in [far, near] {
        let (chunk, _) = world.source.load_or_generate(pos).unwrap();
        world.recent.insert(pos, Arc::new(chunk));
    }
    for expected in [near, middle, far] {
        world.admission_us_per_chunk = super::ADMISSION_BUDGET_US;
        assert_eq!(world.ensure(&want), vec![expected]);
        assert!(world.chunk(expected).is_some());
        let mut workspace = mc_world::light::LightWorkspace::new();
        let table = world.light.table();
        for pos in world.resident_positions() {
            let neighbours = std::array::from_fn(|z| {
                std::array::from_fn(|x| {
                    world.chunk(mc_world::ChunkPos {
                        x: pos.x + x as i32 - 1,
                        z: pos.z + z as i32 - 1,
                    })
                })
            });
            let expected =
                mc_world::light::compute_chunk_light_in(&mut workspace, neighbours, table);
            assert_eq!(
                mc_world::light::ChunkLight::from_chunk(world.chunk(pos).unwrap()),
                Some(expected),
                "each bounded publication already matches the light oracle at {pos:?}",
            );
        }
    }
    assert!(world.ensure(&want).is_empty(), "never admit a result twice");
    assert!(world.ready.is_empty());
    assert!(world.in_flight.is_empty());
}

#[test]
fn tick_admission_spends_actual_slack_and_stops_after_an_overrun() {
    use std::time::{Duration, Instant};

    for (case, clock_ms, expected) in [
        ("headroom", [0, 1, 2, 3, 4], 3),
        ("expired deadline", [0, 25, 25, 25, 25], 1),
        ("one extra chunk overruns", [0, 1, 25, 25, 25], 2),
    ] {
        let mut world = test_world(None);
        let positions =
            std::array::from_fn::<_, 3, _>(|x| mc_world::ChunkPos { x: x as i32, z: 0 });
        let want = positions
            .iter()
            .enumerate()
            .map(|(priority, pos)| (*pos, priority as i32))
            .collect();
        for (index, pos) in positions.iter().enumerate() {
            let ticket = index as u64 + 1;
            let result = world.source.load_or_generate(*pos).unwrap();
            world.in_flight.insert(*pos, ticket);
            world.ready.push((*pos, ticket, Ok(result)));
        }
        let start = Instant::now();
        world.begin_admission_tick(start + Duration::from_millis(20));
        let mut clock_ms = clock_ms.into_iter();
        let mut last_ms = 0;
        let loaded = world.ensure_with_clock(&want, || {
            last_ms = clock_ms.next().expect("bounded clock reads");
            start + Duration::from_millis(last_ms)
        });
        assert_eq!(loaded, positions[..expected], "{case}");
        assert_eq!(world.admission_elapsed(), Duration::from_millis(last_ms));
        world.begin_admission_tick(start + Duration::from_millis(40));
        assert_eq!(world.admission_elapsed(), Duration::ZERO);
        assert!(world.ready.len() + world.loader.pending() <= crate::chunk_loader::READY_LOADS_MAX);
        let mut workspace = mc_world::light::LightWorkspace::new();
        for pos in loaded {
            let neighbours = std::array::from_fn(|z| {
                std::array::from_fn(|x| {
                    world.chunk(mc_world::ChunkPos {
                        x: pos.x + x as i32 - 1,
                        z: pos.z + z as i32 - 1,
                    })
                })
            });
            let expected = mc_world::light::compute_chunk_light_in(
                &mut workspace,
                neighbours,
                world.light.table(),
            );
            assert_eq!(
                mc_world::light::ChunkLight::from_chunk(world.chunk(pos).unwrap()),
                Some(expected),
                "{case}: extra chunks must settle before publication",
            );
        }
    }
}

#[test]
fn tick_admission_slack_keeps_the_whole_chunk_queue_bound() {
    let mut world = test_world(None);
    let mut want = HashMap::default();
    for x in 0..=crate::chunk_loader::READY_LOADS_MAX as i32 {
        let pos = mc_world::ChunkPos { x, z: 0 };
        let (chunk, _) = world.source.load_or_generate(pos).unwrap();
        world.recent.insert(pos, Arc::new(chunk));
        want.insert(pos, x);
    }
    let start = std::time::Instant::now();
    world.begin_admission_tick(start + std::time::Duration::from_secs(1));
    assert_eq!(
        world.ensure_with_clock(&want, || start).len(),
        crate::chunk_loader::READY_LOADS_MAX,
        "spare time must not turn one tick into an unbounded admission loop",
    );
    assert_eq!(world.recent.chunks.len(), 1);
}

#[test]
fn tick_admission_drops_unwanted_superseded_and_duplicate_ready_entries() {
    let mut world = test_world(None);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    let unwanted = mc_world::ChunkPos { x: 1, z: 0 };
    let want = wants(&[pos]);
    for (position, ticket) in [(pos, 1), (unwanted, 3), (pos, 2), (pos, 2)] {
        let result = world.source.load_or_generate(position).unwrap();
        world.ready.push((position, ticket, Ok(result)));
    }
    world.in_flight.insert(pos, 2);
    world.in_flight.insert(unwanted, 3);
    assert_eq!(world.ensure(&want), vec![pos]);
    let revision = world.chunk_revision(pos.x, pos.z);
    assert!(world.ensure(&want).is_empty());
    assert!(world.ready.is_empty());
    assert!(world.in_flight.is_empty());
    assert_eq!(world.chunk_revision(pos.x, pos.z), revision);
    assert!(world.chunk(unwanted).is_none());
}

#[test]
fn admission_backpressure_bounds_ready_and_worker_loads_together() {
    let mut world = test_world(None);
    let mut want = HashMap::default();
    for x in 0..crate::chunk_loader::READY_LOADS_MAX as i32 {
        let pos = mc_world::ChunkPos { x, z: 0 };
        let result = world.source.load_or_generate(pos).unwrap();
        world.in_flight.insert(pos, x as u64 + 1);
        world.ready.push((pos, x as u64 + 1, Ok(result)));
        want.insert(pos, x);
    }
    let extra = mc_world::ChunkPos { x: -1, z: 0 };
    want.insert(extra, 100);
    assert!(!world.loader.try_submit(extra, 999, world.ready.len()));
    let loaded = world.ensure(&want);
    assert_eq!(
        loaded.len(),
        1,
        "the initial cost estimate admits one chunk"
    );
    assert!(world.ready.len() + world.loader.pending() <= crate::chunk_loader::READY_LOADS_MAX);
    assert!(world.chunk(extra).is_none());
    assert!(
        world.in_flight.contains_key(&extra),
        "freeing one ready slot immediately allows the next worker request",
    );
    world.ensure_blocking(&want);
    assert!(world.chunk(extra).is_some());
    assert!(world.ready.is_empty());
    assert_eq!(world.loader.pending(), 0);
}

#[test]
fn hydration_work_is_driven_by_admission_and_chunk_mutations() {
    let mut world = test_world(None);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    assert_eq!(world.take_hydration_chunks(), vec![pos]);
    assert!(world.take_hydration_chunks().is_empty());
    world.mark_block_entity_dirty(mc_world::BlockPos { x: 0, y: 64, z: 0 });
    assert_eq!(world.take_hydration_chunks(), vec![pos]);
    assert!(world.take_hydration_chunks().is_empty());
}

#[test]
fn eviction_rechecks_removed_wants_and_only_matching_checkpoint_acks() {
    let mut world = test_world(None);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    let want = wants(&[pos]);
    world.ensure_blocking(&want);
    assert!(world.evict_outside(&want).is_empty());
    // Stationary wants consume the admission candidate; dropping the want
    // must still find the chunk without rescanning the resident map.
    assert_eq!(world.evict_outside(&HashMap::default()), vec![pos]);
    world.ensure(&want);
    world.mark_block_entity_dirty(mc_world::BlockPos { x: 0, y: 64, z: 0 });
    let old = world.storage_revision(pos);
    assert!(world.evict_outside(&HashMap::default()).is_empty());
    world.mark_block_entity_dirty(mc_world::BlockPos { x: 0, y: 64, z: 0 });
    world.mark_flushed(&[(pos, old)]);
    assert!(world.evict_outside(&HashMap::default()).is_empty());
    world.mark_flushed(&[(pos, world.storage_revision(pos))]);
    assert_eq!(world.evict_outside(&HashMap::default()), vec![pos]);
}

#[test]
fn admission_budget_uses_measured_per_chunk_ewma_with_progress_and_queue_bounds() {
    let mut world = test_world(None);
    assert_eq!(world.admission_limit(), 1);
    world.record_admission_cost(4_000, 4);
    assert_eq!(world.admission_us_per_chunk, 6_250);
    assert_eq!(world.admission_limit(), 1);
    world.admission_us_per_chunk = 1_000;
    assert_eq!(world.admission_limit(), 8);
    world.record_admission_cost(40_000, 1);
    assert_eq!(world.admission_us_per_chunk, 10_750);
    assert_eq!(
        world.admission_limit(),
        1,
        "one slow chunk still makes progress"
    );
    world.record_admission_cost(100_000, 0);
    assert_eq!(
        world.admission_us_per_chunk, 10_750,
        "empty ticks do not train cost"
    );
    world.admission_us_per_chunk = 1;
    assert_eq!(
        world.admission_limit(),
        crate::chunk_loader::READY_LOADS_MAX
    );
    world.record_admission_cost(0, 1);
    assert_eq!(
        world.admission_us_per_chunk, 1,
        "clock rounding cannot divide by zero"
    );
}

#[test]
fn admission_budget_batches_ready_and_recent_chunks_in_nearest_order() {
    let mut world = test_world(None);
    let positions: Vec<_> = (0..6).map(|x| mc_world::ChunkPos { x, z: 0 }).collect();
    let want = positions
        .iter()
        .enumerate()
        .map(|(priority, pos)| (*pos, priority as i32))
        .collect();
    for (index, pos) in positions.iter().copied().enumerate() {
        let (chunk, generated) = world.source.load_or_generate(pos).unwrap();
        if index % 2 == 0 {
            world.recent.insert(pos, Arc::new(chunk));
        } else {
            let ticket = index as u64 + 1;
            world.in_flight.insert(pos, ticket);
            world.ready.push((pos, ticket, Ok((chunk, generated))));
        }
    }
    world.admission_us_per_chunk = 2_000;
    assert_eq!(world.ensure(&want), positions[..4]);
    assert_eq!(world.ready.len(), 1);
    assert!(world.recent.chunks.contains_key(&positions[4]));
    // Ordering uses a controlled cost estimate, not this machine's elapsed
    // time; the EWMA arithmetic and slow-chunk floor are tested separately.
    world.admission_us_per_chunk = 2_000;
    assert_eq!(world.ensure(&want), positions[4..]);
    assert!(world.ready.is_empty());
    assert!(world.in_flight.is_empty());
}

/// The pushed POI revision set records every bump and every unload, so an
/// allocator consume between an edit, an eviction and a reload still sees all
/// three phases from one event per chunk.
#[test]
fn poi_revisions_record_edits_unloads_and_reloads() {
    use rustc_hash::FxHashSet as HashSet;
    let mut world = test_world(None);
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    world.ensure_blocking(&wants(&[pos]));
    let mut pending = HashSet::default();
    pending.insert(mc_world::ChunkPos { x: 7, z: 9 });
    world.swap_poi_revisions(&mut pending);
    assert!(pending.contains(&pos), "admission bump is a POI event");
    assert_eq!(pending.len(), 1, "consume clears the caller buffer first");
    let registry =
        crate::registry::CoreRegistry::load(Some(&world_root())).expect("registry loads");
    world.set_block(4, 80, 4, registry.states.dirt);
    world.fold_overlay();
    world.swap_poi_revisions(&mut pending);
    assert!(pending.contains(&pos), "repeated edits still report");
    world.swap_poi_revisions(&mut pending);
    assert!(pending.is_empty(), "consume drains, not peeks");
    // Flush the folded dirt edit so the next queued write leaves a clean
    // chunk evictable (dirty chunks are retained until flushed).
    world.mark_flushed(&[(pos, world.storage_revision(pos))]);
    // A queued (unflushed) edit followed by unload still reports the unload,
    // and the later reload reports again through `bump_revision`.
    world.set_block(4, 80, 4, registry.states.stone);
    assert_eq!(
        world.evict_outside(&HashMap::default()),
        vec![pos],
        "queued edit must not pin a clean chunk"
    );
    world.swap_poi_revisions(&mut pending);
    assert!(pending.contains(&pos), "unload is a POI event");
    world.ensure_blocking(&wants(&[pos]));
    world.swap_poi_revisions(&mut pending);
    assert!(pending.contains(&pos), "reload is a POI event");
}

#[test]
fn durable_bookkeeping_is_clean_but_newer_wal_and_active_clock_projection_are_dirty() {
    for (replay_tick, image_tick, active, value, last_sim, expected_chunks) in [
        (20, 20, false, 50, 20, 0),
        (21, 21, false, 50, 20, 0),
        (21, 21, false, 51, 21, 1),
        (21, 20, true, 50, 20, 1),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut world = test_world(Some(directory.path().to_owned()));
        let pos = mc_world::ChunkPos { x: 0, z: 0 };
        let replay = crate::journal::Replay {
            tick: replay_tick,
            inhabited_time: [(
                [0, 0],
                crate::journal::InhabitedTimeImage {
                    chunk: [0, 0],
                    value,
                    tick: image_tick,
                    active,
                },
            )]
            .into(),
            sim_debt: [(
                [0, 0],
                crate::journal::SimDebtImage {
                    chunk: [0, 0],
                    last_sim,
                    remaining: 7,
                    active,
                    tick: image_tick,
                },
            )]
            .into(),
            ..Default::default()
        };
        let checkpoint = crate::journal::CheckpointManifest {
            tick: 20,
            inhabited_time: vec![crate::journal::InhabitedTimeImage {
                chunk: [0, 0],
                value: 50,
                tick: 20,
                active: true,
            }],
            debt: vec![crate::journal::SimDebtImage {
                chunk: [0, 0],
                last_sim: 20,
                remaining: 7,
                active,
                tick: 20,
            }],
            ..Default::default()
        };
        world.restore_inhabited_time(&replay, Some(&checkpoint));
        world.restore_sim_debt(&replay, Some(&checkpoint)).unwrap();
        assert_eq!(world.simulation_debt(pos), 7);
        let capture = world.capture_dirty(&Default::default());
        assert_eq!(capture.written().len(), expected_chunks);
        assert!(!world.has_chunk(0, 0), "restoration never admits a chunk");
        if expected_chunks == 0 {
            assert!(
                world.debt_changes.is_empty(),
                "durable inactive debt has no new WAL image"
            );
        }
    }
}

#[test]
fn physics_reader_matches_direct_reads_across_columns_and_overlay() {
    let mut world = test_world(None);
    world.ensure_blocking(&wants(&[
        mc_world::ChunkPos { x: -1, z: -1 },
        mc_world::ChunkPos { x: 0, z: 0 },
    ]));
    world.overlay.insert((-1, 70, -1), 123);
    world.overlay.insert((32, 70, 32), 456);
    let reader = world.physics_reader();
    for (x, z) in [(-1, -1), (0, 0), (-16, -16), (15, 15), (16, 16), (32, 32)] {
        for y in [i32::MIN, -65, -64, 0, 70, 319, 320, i32::MAX] {
            assert_eq!(
                reader.block_at(x, y, z),
                world.physics_block_at(x, y, z),
                "cached read differs at {x},{y},{z}"
            );
        }
    }
    assert_eq!(reader.block_at(-1, 70, -1), Some(123));
    assert_eq!(reader.block_at(32, 70, 32), Some(456));
    assert_eq!(reader.block_at(16, 320, 16), None);
    assert_eq!(reader.block_at(0, 320, 0), Some(world._air.0));
}

#[test]
fn physics_reader_section_cache_survives_transitions_and_vertical_extremes() {
    let mut world = test_world(None);
    world.ensure_blocking(&wants(&[
        mc_world::ChunkPos { x: -1, z: 0 },
        mc_world::ChunkPos { x: 0, z: 0 },
        mc_world::ChunkPos { x: 1, z: 0 },
    ]));
    let registry =
        crate::registry::CoreRegistry::load(Some(&world_root())).expect("registry loads");
    let dirt = registry.states.dirt;
    // Distinct states in adjacent sections of the same column: repeated reads
    // must re-key the borrowed section by base Y, not serve a stale section.
    world.set_block(4, 0, 4, dirt);
    world.set_block(4, 16, 4, world._air.0);
    world.fold_overlay();
    let air = world._air.0;
    {
        let reader = world.physics_reader();
        assert_eq!(reader.block_at(4, 0, 4), Some(dirt));
        assert_eq!(reader.block_at(4, 1, 4), world.block_at(4, 1, 4));
        assert_eq!(reader.block_at(4, 16, 4), Some(air));
        assert_eq!(reader.block_at(4, 0, 4), Some(dirt));
        assert_eq!(reader.block_at(4, 15, 4), world.block_at(4, 15, 4));
        // Chunk-column transitions at x = 15/16 stay exact either direction.
        for x in [14, 15, 16, 17, 16, 15, 14] {
            assert_eq!(
                reader.block_at(x, 70, 4),
                world.block_at(x, 70, 4),
                "column transition differs at x={x}"
            );
        }
        // Vertical extremes resolve through the same path as `block_at` misses:
        // inside height mirrors the chunk, below/above resolve to air.
        assert_eq!(reader.block_at(4, -64, 4), world.block_at(4, -64, 4));
        assert_eq!(reader.block_at(4, 319, 4), world.block_at(4, 319, 4));
        assert_eq!(reader.block_at(4, -65, 4), Some(air));
        assert_eq!(reader.block_at(4, 320, 4), Some(air));
        // Negative-coordinate columns stay keyed by euclid chunk position.
        for x in [-1, -16, -17, 0, -1] {
            assert_eq!(
                reader.block_at(x, 70, 0),
                world.block_at(x, 70, 0),
                "negative column differs at x={x}"
            );
        }
        // Missing columns stay unknown even after caching resident ones.
        assert_eq!(reader.block_at(160, 70, 160), None);
    }
    // Overlay writes bypass every cache level: the reader borrows `&`, so the
    // world cannot change mid-step; a fresh step still sees overlay first.
    world.overlay.insert((4, 70, 4), 777);
    let reader = world.physics_reader();
    assert_eq!(reader.block_at(4, 70, 4), Some(777));
    assert_eq!(reader.block_at(160, 70, 160), None);
}

#[test]
fn section_cache_reuses_each_palette_probe_within_a_pass() {
    let mut world = test_world(None);
    world.ensure_blocking(&wants(&[mc_world::ChunkPos { x: 0, z: 0 }]));
    let registry =
        crate::registry::CoreRegistry::load(Some(&world_root())).expect("registry loads");
    world.set_block(4, 70, 4, registry.states.dirt);
    world.fold_overlay();
    let mut cache = SectionMatchCache::default();
    cache.reset(&world);
    let dirt = mc_world::BlockStateId(registry.states.dirt);
    let box_min = (0, 64, 0);
    let box_max = (15, 79, 15);
    let mut probes = 0usize;
    assert!(
        world.may_hold_matching_state_cached(box_min, box_max, &mut cache, |seen| {
            probes += 1;
            seen == dirt
        })
    );
    let first_probes = probes;
    assert!(first_probes > 0, "expected at least one palette probe");
    // Same predicate, same world: every section verdict is cached, so no new
    // palette probes run even across many repeated queries.
    for _ in 0..16 {
        assert!(
            world.may_hold_matching_state_cached(box_min, box_max, &mut cache, |seen| {
                probes += 1;
                seen == dirt
            })
        );
    }
    assert_eq!(
        probes, first_probes,
        "repeated pass queries must reuse cached section verdicts"
    );
    // Same predicate, narrower box inside the already-hit section: the gate is
    // section-granular, so it reuses the hit verdict with no new probes.
    assert!(
        world.may_hold_matching_state_cached((0, 64, 0), (15, 65, 15), &mut cache, |seen| {
            probes += 1;
            seen == dirt
        })
    );
    assert_eq!(
        probes, first_probes,
        "same-section narrower box must reuse the cached hit"
    );
}

#[test]
fn section_cache_reset_sees_later_edits_and_overlay_chunks_fail_open() {
    let mut world = test_world(None);
    world.ensure_blocking(&wants(&[mc_world::ChunkPos { x: 0, z: 0 }]));
    let registry =
        crate::registry::CoreRegistry::load(Some(&world_root())).expect("registry loads");
    let lava = registry.states.lava_levels[0];
    let mut cache = SectionMatchCache::default();
    cache.reset(&world);
    let box_min = (0, 64, 0);
    let box_max = (15, 79, 15);
    assert!(
        !world.may_hold_matching_state_cached(box_min, box_max, &mut cache, |seen| {
            seen.0 == lava
        })
    );
    // Folded edit: the old cached miss is stale until `reset`, which must
    // observe the newly matching section.
    world.set_block(4, 70, 4, lava);
    world.fold_overlay();
    assert!(
        !world.may_hold_matching_state_cached(box_min, box_max, &mut cache, |seen| {
            seen.0 == lava
        })
    );
    cache.reset(&world);
    assert!(
        world.may_hold_matching_state_cached(box_min, box_max, &mut cache, |seen| {
            seen.0 == lava
        })
    );
    // Unfolded overlay write in a fresh world: the reset seeds the chunk
    // mark, so the probe fails open without touching palettes.
    let mut overlaid = test_world(None);
    overlaid.ensure_blocking(&wants(&[mc_world::ChunkPos { x: 0, z: 0 }]));
    overlaid.set_block(5, 70, 5, lava);
    assert!(
        !overlaid.overlay_is_empty(),
        "fixture must stage a real write"
    );
    let mut overlaid_cache = SectionMatchCache::default();
    overlaid_cache.reset(&overlaid);
    let mut probes = 0usize;
    assert!(overlaid.may_hold_matching_state_cached(
        box_min,
        box_max,
        &mut overlaid_cache,
        |seen| {
            probes += 1;
            seen.0 == lava
        }
    ));
    assert_eq!(probes, 0, "overlay chunks must fail open before any probe");
}

#[test]
fn section_cache_fails_open_on_missing_and_clips_extreme_spans() {
    let mut world = test_world(None);
    world.ensure_blocking(&wants(&[mc_world::ChunkPos { x: 0, z: 0 }]));
    let mut cache = SectionMatchCache::default();
    cache.reset(&world);
    // Missing column fails open even when the predicate never matches.
    assert!(world.may_hold_matching_state_cached(
        (160, 64, 160),
        (175, 79, 175),
        &mut cache,
        |_| false
    ));
    // Extreme y bounds clip to geometry, never loop the full height: no match.
    assert!(!world.may_hold_matching_state_cached(
        (0, i32::MIN, 0),
        (15, i32::MAX, 15),
        &mut cache,
        |_| false
    ));
    // Empty box stays false even with a match-all predicate.
    assert!(!world.may_hold_matching_state_cached((0, 70, 0), (15, 69, 15), &mut cache, |_| true));
}

#[test]
fn contact_revision_proof_wakes_for_overlay_fold_and_missing_columns() {
    let mut world = test_world(None);
    world.ensure_blocking(&wants(&[
        mc_world::ChunkPos { x: 0, z: 0 },
        mc_world::ChunkPos { x: 1, z: 0 },
    ]));
    let registry =
        crate::registry::CoreRegistry::load(Some(&world_root())).expect("registry loads");
    let mut cache = SectionMatchCache::default();
    cache.reset(&world);
    let min = (4, 69, 4);
    let max = (5, 72, 5);
    let checked = world.revision_now();
    assert!(world.contact_unchanged_since(min, max, checked, &cache));
    // A different column cannot invalidate this negative body probe.
    world.set_block(20, 70, 4, registry.states.lava_levels[0]);
    world.fold_overlay();
    cache.reset(&world);
    assert!(world.contact_unchanged_since(min, max, checked, &cache));
    // Overlay edits are visible before the committed chunk revision changes.
    let before_overlay = world.revision_now();
    world.set_block(4, 70, 4, registry.states.lava_levels[0]);
    assert!(!world.overlay_is_empty(), "fixture must stage a real edit");
    assert_eq!(world.revision_now(), before_overlay);
    cache.reset(&world);
    assert!(!world.contact_unchanged_since(min, max, checked, &cache));
    world.fold_overlay();
    cache.reset(&world);
    assert!(!world.contact_unchanged_since(min, max, checked, &cache));
    assert!(world.contact_unchanged_since(min, max, world.revision_now(), &cache));
    // Every crossed column must be resident, including a neighboring miss.
    assert!(
        !world.contact_unchanged_since((31, 70, 4), (32, 72, 5), world.revision_now(), &cache,)
    );
}

/// Old per-chunk double-hash proof, kept only as the micro baseline.
fn contact_unchanged_since_nested(
    world: &ChunkWorld,
    min: (i32, i32, i32),
    max: (i32, i32, i32),
    since: u64,
    cache: &SectionMatchCache,
) -> bool {
    (min.0.div_euclid(16)..=max.0.div_euclid(16)).all(|cx| {
        (min.2.div_euclid(16)..=max.2.div_euclid(16)).all(|cz| {
            !cache
                .overlay_chunks
                .contains(&mc_world::ChunkPos { x: cx, z: cz })
                && world.chunk_revision(cx, cz) <= since
        })
    })
}

/// Release micro: `cargo test --release -p mc-core contact_proof_micro -- --ignored --nocapture`.
/// Runs the nested baseline (B) and the shipped proof (A) as B,A,B,A over
/// 100k bodies (90% single chunk, 10% straddling) on a 48x48 resident grid.
#[test]
#[ignore = "release micro: prints proof timings"]
fn contact_proof_micro() {
    let mut world = test_world(None);
    for x in -24..24 {
        for z in -24..24 {
            world.wire_revisions.insert(
                mc_world::ChunkPos { x, z },
                10 + (x * 48 + z).unsigned_abs() as u64,
            );
        }
    }
    let mut cache = SectionMatchCache::default();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let bodies: Vec<_> = (0..100_000)
        .map(|index| {
            let x = (next() % 760) as i32 - 380;
            let z = (next() % 760) as i32 - 380;
            let wide = i32::from(index % 10 == 0);
            ((x, 64, z), (x + wide * 2, 66, z + wide * 2))
        })
        .collect();
    for overlay in [false, true] {
        cache.overlay_chunks.clear();
        if overlay {
            for x in 0..4 {
                cache.overlay_chunks.insert(mc_world::ChunkPos { x, z: 1 });
            }
        }
        let run = |shipped: bool| {
            let started = std::time::Instant::now();
            let mut quiet = 0_u32;
            for _ in 0..20 {
                for &(min, max) in &bodies {
                    let since = 1000;
                    quiet += u32::from(if shipped {
                        world.contact_unchanged_since(min, max, since, &cache)
                    } else {
                        contact_unchanged_since_nested(&world, min, max, since, &cache)
                    });
                }
            }
            (started.elapsed(), quiet)
        };
        let mut agree = None;
        for (label, shipped) in [("B", false), ("A", true), ("B", false), ("A", true)] {
            let (elapsed, quiet) = run(shipped);
            assert_eq!(*agree.get_or_insert(quiet), quiet, "proof results differ");
            println!(
                "contact_proof_micro overlay={overlay} {label}: {:.2} ns/proof ({quiet} quiet)",
                elapsed.as_nanos() as f64 / 2_000_000.0
            );
        }
    }
}
