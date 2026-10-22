//! Failing-first regressions for bounded immutable terrain sampling.
//!
//! Pure unit scope (no worlds): shape constants, leaf-at-halo-offset layout
//! with negative sections, coordinate overflow fails closed, fold
//! order-sensitivity, the publication fence (starts at zero, advances only on
//! publication, never moves backwards), epoch format + uniqueness.
//!
//! Integration scope (real `Core::open`, real generator, real native store):
//! 36 real chunks with leaf geometry; negative sections; unknown dimension
//! and coordinate overflow fail closed; missing chunks generate without
//! simulation residency; idle ticks never advance the fence (no false
//! refresh); stored all-air stays all-air through sampling (never generated
//! substitution); live edits outrank the native image, change the revision,
//! pin a strictly greater stamp and advance the fence; flushed edits survive
//! eviction via the native image; the epoch is opaque, stable across reopen
//! (with stable content revisions) and rotated on wipe; the supplement covers
//! unflushed dirty/overlay edits off-tick and rejects out-of-geometry writes;
//! mismatched producer chunks fail closed.

use std::sync::Arc;

use mc_world::{BlockStateId, ChunkPos};

use crate::boundary::{BlockPos, ConnectionId, PlayerInput};
use crate::dimensions::Dimension;
use crate::terrain::{
    DirtySupplement, TERRAIN_CHUNKS, TERRAIN_HALO, TERRAIN_LEAF, TERRAIN_WIDTH, TerrainSource,
    fold_revision, leaf_base, section_positions,
};

#[test]
fn leaf_geometry_is_4x4_centered_in_6x6_with_halo_offset() {
    assert_eq!(TERRAIN_LEAF, 4);
    assert_eq!(TERRAIN_HALO, 1);
    assert_eq!(TERRAIN_WIDTH, 6);
    assert_eq!(TERRAIN_CHUNKS, 36);
    let (base_x, base_z) = leaf_base(0, 0).expect("origin leaf");
    assert_eq!((base_x, base_z), (0, 0));
    let (base_x, base_z) = leaf_base(1, -1).expect("leaf 1,-1");
    assert_eq!((base_x, base_z), (4, -4));
    let positions = section_positions(0, 0).expect("origin positions");
    assert_eq!(positions.len(), TERRAIN_CHUNKS);
    // Row-major: index dz*6+dx; leaf (dx/dz 1..5) covers chunks 0..4.
    assert_eq!(positions[0], ChunkPos { x: -1, z: -1 });
    assert_eq!(positions[TERRAIN_WIDTH + 1], ChunkPos { x: 0, z: 0 });
    assert_eq!(positions[TERRAIN_CHUNKS - 1], ChunkPos { x: 4, z: 4 });
    // Negative sections use Euclidean-correct checked math, not truncation.
    let positions = section_positions(-1, -1).expect("negative positions");
    assert_eq!(positions[TERRAIN_WIDTH + 1], ChunkPos { x: -4, z: -4 });
    assert_eq!(positions[0], ChunkPos { x: -5, z: -5 });
    assert_eq!(positions[TERRAIN_CHUNKS - 1], ChunkPos { x: 0, z: 0 });
    // Overflow fails closed.
    assert!(leaf_base(i32::MAX, 0).is_err());
    assert!(leaf_base(0, i32::MIN).is_err());
}

#[test]
fn fold_revision_is_order_sensitive_and_stable() {
    let keys: Vec<u64> = (0..TERRAIN_CHUNKS as u64).collect();
    let forward = fold_revision(&keys);
    let mut reversed = keys.clone();
    reversed.reverse();
    assert_ne!(forward, fold_revision(&reversed));
    assert_eq!(forward, fold_revision(&keys));
}

#[test]
fn publication_fence_advances_only_on_publication() {
    let registry = crate::registry::CoreRegistry::load(None).expect("registry");
    let source = empty_source(&registry, 0);
    let sampler = TerrainSource::new(Arc::clone(&source), "fence-probe".into());
    assert_eq!(sampler.revision(), 0, "fence starts at zero, never MAX");
    assert!(
        sampler.supplement().chunks.is_empty() && sampler.supplement().overlay.is_empty(),
        "supplement starts empty"
    );
    sampler.begin_publish();
    assert_eq!(sampler.revision(), u64::MAX, "publication is busy");
    let error = sampler.section(0, 0, 0).expect_err("busy sampler rejects");
    assert!(error.to_string().contains("publication busy"), "{error}");
    sampler.finish_publish(None, TerrainSource::empty_supplement(), 5);
    assert_eq!(sampler.revision(), 5);
    assert!(sampler.section(0, 0, 0).is_ok(), "finished sampler serves");
    sampler.begin_publish();
    assert_eq!(sampler.revision(), u64::MAX);
    sampler.finish_publish(None, TerrainSource::empty_supplement(), 6);
    assert_eq!(sampler.revision(), 6, "next publication advances");
}

#[test]
fn unknown_dimension_and_overflow_fail_closed() {
    let (_dir, core) = test_core(42);
    let error = core
        .terrain_section(7, 0, 0)
        .expect_err("unknown dimension must fail");
    assert!(error.to_string().contains("dimension 7"), "{error}");
    assert_eq!(core.terrain_revision(7), u64::MAX);
    // Coordinate overflow (leaf multiply and halo add) fails closed too.
    for (sx, sz) in [(i32::MAX, 0), (536_870_911, 0), (0, i32::MIN)] {
        core.terrain_section(0, sx, sz)
            .expect_err("coordinate overflow must fail closed");
    }
    core.shutdown();
}

#[test]
fn section_samples_36_real_chunks_with_leaf_geometry() {
    let (_dir, core) = test_core(42);
    let section = core.terrain_section(0, 0, 0).expect("origin section");
    assert_eq!(section.chunks.len(), TERRAIN_CHUNKS);
    assert_eq!(
        (section.dimension, section.section_x, section.section_z),
        (0, 0, 0)
    );
    assert_eq!(section.chunks[0].pos, ChunkPos { x: -1, z: -1 });
    assert_eq!(
        section.chunks[TERRAIN_WIDTH + 1].pos,
        ChunkPos { x: 0, z: 0 }
    );
    assert_eq!(section.epoch, core.terrain_identity());
    assert_ne!(core.terrain_revision(0), u64::MAX);
    assert!(section.modified_ms > 0);
    // Same revision re-samples to the same stamp (pinned, not wall-now).
    let again = core.terrain_section(0, 0, 0).expect("re-sample");
    assert_eq!(again.revision, section.revision);
    assert_eq!(again.modified_ms, section.modified_ms);
    // Geometry matches the overworld and chunks are real (sections exist).
    for chunk in &section.chunks {
        assert_eq!(chunk.geometry(), mc_world::OVERWORLD_GEOMETRY);
        assert!(!chunk.sections.is_empty());
    }
    core.shutdown();
}

#[test]
fn negative_sections_sample_negative_chunks() {
    let (_dir, core) = test_core(42);
    let section = core.terrain_section(0, -1, -1).expect("negative section");
    assert_eq!(
        section.chunks[TERRAIN_WIDTH + 1].pos,
        ChunkPos { x: -4, z: -4 }
    );
    assert_eq!(section.chunks[0].pos, ChunkPos { x: -5, z: -5 });
    core.shutdown();
}

#[test]
fn missing_chunks_generate_without_residency_and_idle_ticks_keep_the_fence() {
    // Fresh world: no resident chunks anywhere; sampling generates.
    let (_dir, core) = test_core(4242);
    let mut rx = core.subscribe();
    let section = core.terrain_section(0, 3, -2).expect("uncached section");
    assert_eq!(section.chunks.len(), TERRAIN_CHUNKS);
    assert_eq!(
        section.chunks[TERRAIN_WIDTH + 1].pos,
        ChunkPos { x: 12, z: -8 }
    );
    // Sampling must not admit residency: the hub snapshot carries none of
    // the sampled positions.
    let positions = section_positions(3, -2).expect("positions");
    assert!(
        core.latest_snapshot(Dimension::Overworld)
            .is_none_or(|s| s.chunks.iter().all(|c| !positions.contains(&c.pos))),
        "sampling must not make chunks resident"
    );
    // Idle ticks carry no terrain changes, so the fence must not advance:
    // no false edge-cache invalidation. Deterministic tick waits, no sleeps.
    let fence = core.terrain_revision(0);
    assert_ne!(fence, u64::MAX);
    for _ in 0..5 {
        recv_tick(&mut rx);
    }
    assert_eq!(
        core.terrain_revision(0),
        fence,
        "idle ticks must not invalidate edge caches"
    );
    core.shutdown();
}

#[test]
fn live_edit_outranks_native_image_and_changes_revision() {
    let (dir, core) = test_core(777);
    let spawn = core.world_spawn();
    let spos = ChunkPos {
        x: spawn[0].div_euclid(16),
        z: spawn[2].div_euclid(16),
    };
    let target = BlockPos {
        x: spawn[0],
        y: 200,
        z: spawn[2],
    };
    let connection = ConnectionId(1);
    let mut rx = core.subscribe();
    assert!(core.submit(PlayerInput::Join {
        connection,
        uuid: uuid::Uuid::from_u128(1),
        name: "probe".into(),
        permission_level: 4,
    }));
    // Commands refuse unloaded positions, so wait until the spawn chunk is
    // resident in the published snapshot (deterministic tick waits, no sleeps).
    let mut resident = false;
    for _ in 0..900 {
        recv_tick(&mut rx);
        if core
            .latest_snapshot(Dimension::Overworld)
            .is_some_and(|s| s.chunks.iter().any(|c| c.pos == spos))
        {
            resident = true;
            break;
        }
    }
    assert!(resident, "spawn chunk must become resident after join");
    // Sample the section covering the spawn chunk before the edit.
    let (sx, sz) = (
        spos.x.div_euclid(TERRAIN_LEAF as i32),
        spos.z.div_euclid(TERRAIN_LEAF as i32),
    );
    let before = sample_committed(&core, sx, sz, &mut rx);
    let idx = before
        .chunks
        .iter()
        .position(|c| c.pos == spos)
        .expect("spawn chunk sampled");
    let fence_before = core.terrain_revision(0);
    // Edit a block inside the player's resident chunk through the
    // authoritative tick path.
    assert!(core.submit(PlayerInput::Command {
        connection,
        command: format!("setblock {} 200 {} minecraft:stone", spawn[0], spawn[2]),
        permission_level: 4,
    }));
    // Wait until the committed tick carries our block write.
    let mut committed = false;
    for _ in 0..600 {
        let output = recv_tick(&mut rx);
        if output.changes.blocks.iter().any(|c| c.position == target) {
            committed = true;
            break;
        }
    }
    assert!(committed, "setblock must commit on a resident chunk");
    let after = sample_committed(&core, sx, sz, &mut rx);
    assert_ne!(
        after.revision, before.revision,
        "live edit must change the section revision"
    );
    assert!(
        after.modified_ms > before.modified_ms,
        "new revision must get a strictly greater stamp"
    );
    assert!(
        core.terrain_revision(0) > fence_before,
        "the edit publication must advance the fence"
    );
    assert_eq!(after.epoch, core.terrain_identity());
    let stone = BlockStateId(core.registry().states.stone);
    let (lx, lz) = (spawn[0].rem_euclid(16) as u8, spawn[2].rem_euclid(16) as u8);
    assert_eq!(
        after.chunks[idx].get_block(lx, 200, lz),
        Some(stone),
        "live edit outranks the native image"
    );
    // Flushed edits survive an eviction cycle through the native image:
    // checkpoint, then reopen with a fresh core (no residency at all) and
    // re-sample: the edit must come back from the durable store, not from
    // any surviving snapshot or supplement.
    assert!(core.checkpoint(), "pre-reopen checkpoint must commit");
    let world_dir = dir.path().to_path_buf();
    let epoch = core.terrain_identity().to_owned();
    core.shutdown();
    let core = crate::Core::open(crate::CoreConfig::new(world_dir, 777)).expect("reopen");
    assert_eq!(core.terrain_identity(), epoch);
    let mut rx = core.subscribe();
    let retained = sample_committed(&core, sx, sz, &mut rx);
    assert_eq!(
        retained.chunks[idx].pos, spos,
        "same slot carries the same chunk after reopen"
    );
    assert_eq!(
        retained.chunks[idx].get_block(lx, 200, lz),
        Some(stone),
        "flushed edit survives reopen via the native image"
    );
    assert!(
        core.latest_snapshot(Dimension::Overworld)
            .is_none_or(|s| s.chunks.iter().all(|c| c.pos != spos)),
        "reopened core samples the edit without residency"
    );
    core.shutdown();
}

#[test]
fn stored_all_air_stays_all_air_through_sampling() {
    // Stored all-air is canonical coverage: sampling must return the stored
    // image, never generated terrain. Import into the live core's own store
    // (same world dir, shared registry entry) and sample through the real
    // terrain path with no players around (nothing resident).
    let dir = tempfile::tempdir().expect("tempdir");
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
    let core = crate::Core::open(config.clone()).expect("open");
    let registry = crate::registry::CoreRegistry::load(None).expect("registry");
    let air = BlockStateId(registry.states.air);
    let biome = mc_data::Identifier::parse("minecraft:plains").expect("plains parses");
    let chunk = mc_world::Chunk::empty(ChunkPos { x: 0, z: 0 }, air, biome);
    crate::native_checkpoint::tests::fixture_store(&config)
        .expect("store")
        .import_chunk(0, &chunk, &registry.blocks, &registry.items)
        .expect("import all-air");
    let section = core.terrain_section(0, 0, 0).expect("section");
    let idx = section
        .chunks
        .iter()
        .position(|c| c.pos == ChunkPos { x: 0, z: 0 })
        .expect("stored chunk sampled");
    assert!(
        core.latest_snapshot(Dimension::Overworld)
            .is_none_or(|s| s.chunks.iter().all(|c| c.pos != ChunkPos { x: 0, z: 0 })),
        "stored chunk must resolve from the native image, not residency"
    );
    // A generated overworld chunk always carries solid ground somewhere in
    // the full column; the stored image must be air in every cell.
    for x in 0..16u8 {
        for z in 0..16u8 {
            for y in mc_world::MIN_Y..mc_world::MAX_Y {
                assert_eq!(
                    section.chunks[idx].get_block(x, y, z),
                    Some(air),
                    "stored air at ({x},{y},{z})"
                );
            }
        }
    }
    // Durable native images fold a stable key: re-sampling keeps the revision.
    let again = core.terrain_section(0, 0, 0).expect("re-sample");
    assert_eq!(again.revision, section.revision);
    core.shutdown();
}

#[test]
fn epoch_is_opaque_stable_across_reopen_and_rotated_on_wipe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
    let (epoch, revision) = {
        let core = crate::Core::open(config.clone()).expect("open");
        assert_eq!(core.terrain_identity().len(), 32);
        assert!(
            core.terrain_identity()
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
        let probe = core.terrain_section(0, 0, 0).expect("probe");
        assert!(core.checkpoint(), "epoch checkpoint must commit");
        let out = (core.terrain_identity().to_owned(), probe.revision);
        core.shutdown();
        out
    };
    // Reopen the same world: the epoch is preserved through the native owner
    // path, and unchanged content keeps a stable revision (stamps re-pin per
    // process, so only the revision is asserted here).
    let reopened = {
        let core = crate::Core::open(config.clone()).expect("reopen");
        let probe = core.terrain_section(0, 0, 0).expect("re-probe");
        let out = (core.terrain_identity().to_owned(), probe.revision);
        core.shutdown();
        out
    };
    assert_eq!(epoch, reopened.0, "epoch survives checkpoint + reopen");
    assert_eq!(
        revision, reopened.1,
        "unchanged content keeps a stable revision across restart"
    );
    // World reset (wipe the directory, same seed): the epoch rotates, never
    // seed-derived.
    std::fs::remove_dir_all(dir.path()).expect("wipe");
    let core = crate::Core::open(config).expect("reset open");
    assert_ne!(epoch, core.terrain_identity());
    core.shutdown();
}

#[test]
fn shutdown_releases_terrain_store_while_core_handle_remains_alive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
    let core = crate::Core::open(config.clone()).expect("open");
    core.shutdown();
    let reopened = crate::Core::open(config).expect("shutdown releases the native database");
    for dimension in 0..3 {
        let error = core
            .terrain_section(dimension, 0, 0)
            .expect_err("stopped core cannot sample terrain");
        assert!(
            error.to_string().contains("terrain source stopped"),
            "{error}"
        );
    }
    reopened.shutdown();
}

#[test]
fn mint_epoch_is_random_hex() {
    let first = crate::terrain::mint_epoch();
    let second = crate::terrain::mint_epoch();
    assert_eq!(first.len(), 32);
    assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_ne!(first, second);
}

#[test]
fn supplement_covers_unflushed_edits_off_tick() {
    // The world owner retains the generator; the sampler only borrows it and
    // immutable publications, with no simulation state or residency authority.
    let registry = crate::registry::CoreRegistry::load(None).expect("registry");
    let source = empty_source(&registry, 0);
    let sampler = TerrainSource::new(Arc::clone(&source), "supplement-probe".into());
    let (sx, sz) = (400, -400);
    let base = sampler.section(0, sx, sz).expect("baseline");
    assert_eq!(base.chunks.len(), TERRAIN_CHUNKS);
    assert_eq!(base.epoch, "supplement-probe");
    let positions = section_positions(sx, sz).expect("positions");
    let stone = BlockStateId(registry.states.stone);
    // Resident-dirty branch: an edited chunk published through the supplement
    // outranks the generated base and changes the revision with a strictly
    // greater stamp.
    let pos = positions[TERRAIN_WIDTH + 1];
    let mut edited = base.chunks[TERRAIN_WIDTH + 1].as_ref().clone();
    edited.set_block(3, 200, 5, stone);
    sampler.publish_supplement(Arc::new(DirtySupplement {
        chunks: vec![(pos, 0xC0FF_EE01u64, Arc::new(edited))],
        overlay: Vec::new(),
    }));
    let covered = sampler.section(0, sx, sz).expect("dirty-covered");
    assert_ne!(covered.revision, base.revision);
    assert!(
        covered.modified_ms > base.modified_ms,
        "covering revision pins a strictly greater stamp"
    );
    assert_eq!(covered.chunks[TERRAIN_WIDTH + 1].pos, pos);
    assert_eq!(
        covered.chunks[TERRAIN_WIDTH + 1].get_block(3, 200, 5),
        Some(stone)
    );
    // Non-resident overlay branch: writes apply copy-on-write over the base.
    let opos = positions[20];
    let (wx, wz) = (opos.x * 16 + 3, opos.z * 16 + 5);
    sampler.publish_supplement(Arc::new(DirtySupplement {
        chunks: Vec::new(),
        overlay: vec![((wx, 200, wz), registry.states.stone)],
    }));
    let overlaid = sampler.section(0, sx, sz).expect("overlay-covered");
    assert_ne!(overlaid.revision, base.revision);
    assert_eq!(overlaid.chunks[20].pos, opos);
    assert_eq!(overlaid.chunks[20].get_block(3, 200, 5), Some(stone));
    // Out-of-geometry overlay writes fail closed, never partial application.
    sampler.publish_supplement(Arc::new(DirtySupplement {
        chunks: Vec::new(),
        overlay: vec![((wx, 5000, wz), registry.states.stone)],
    }));
    let error = sampler
        .section(0, sx, sz)
        .expect_err("out-of-geometry overlay must fail closed");
    assert!(
        error.to_string().contains("outside chunk geometry"),
        "{error}"
    );
}

#[test]
fn sampler_rejects_mismatched_producer_chunks() {
    let registry = crate::registry::CoreRegistry::load(None).expect("registry");
    let skewed = |geometry: mc_world::ChunkGeometry, shift: bool| {
        let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(SkewedGenerator {
            air: BlockStateId(registry.states.air),
            biome: mc_data::Identifier::parse("minecraft:plains").expect("plains parses"),
            geometry,
            shift,
        });
        probe_source(&registry, 0, generator)
    };
    // Wrong geometry: the producer contradicts the source contract.
    let nether_like = mc_world::ChunkGeometry::new(0, 256).expect("nether-like geometry");
    let source = skewed(nether_like, false);
    let error = TerrainSource::new(Arc::clone(&source), "skew-probe".into())
        .section(0, 0, 0)
        .expect_err("geometry mismatch must fail closed");
    assert!(error.to_string().contains("mismatch"), "{error}");
    // Wrong position: the producer answers for a different chunk.
    let source = skewed(mc_world::OVERWORLD_GEOMETRY, true);
    let error = TerrainSource::new(Arc::clone(&source), "skew-probe".into())
        .section(0, 0, 0)
        .expect_err("position mismatch must fail closed");
    assert!(error.to_string().contains("mismatch"), "{error}");
}

fn test_core(seed: i64) -> (tempfile::TempDir, crate::Core) {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), seed);
    let core = crate::Core::open(config).expect("open");
    (dir, core)
}

/// Next published tick, tolerating broadcast lag on busy cores. Deterministic:
/// blocks until the tick thread publishes, never sleeps.
fn recv_tick(
    rx: &mut tokio::sync::broadcast::Receiver<crate::boundary::TickOutput>,
) -> crate::boundary::TickOutput {
    loop {
        match rx.blocking_recv() {
            Ok(output) => return output,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(error) => panic!("tick thread exited: {error:?}"),
        }
    }
}

/// Chunk admissions and recovery may overlap sampling. Honor the documented
/// publication fence by waiting for its next tick, without timing sleeps.
fn sample_committed(
    core: &crate::Core,
    sx: i32,
    sz: i32,
    rx: &mut tokio::sync::broadcast::Receiver<crate::boundary::TickOutput>,
) -> crate::terrain::TerrainSection {
    for _ in 0..120 {
        match core.terrain_section(0, sx, sz) {
            Ok(section) => return section,
            Err(error) => {
                let reason = error.to_string();
                assert!(
                    reason.contains("publication busy")
                        || reason.contains("terrain changed during sampling"),
                    "unexpected sampling error: {error}"
                );
                recv_tick(rx);
            }
        }
    }
    panic!("terrain publication never stabilized after join");
}

/// Instant empty-chunk producer: keeps supplement unit tests off worldgen.
struct EmptyGenerator {
    air: BlockStateId,
    biome: mc_data::Identifier,
}

impl mc_world::ChunkGenerator for EmptyGenerator {
    fn generate(&self, pos: ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(pos, self.air, self.biome.clone())
    }
}

/// Producer that answers with the wrong geometry (or the wrong position when
/// `shift` is set): corrupt-input probe for the sampler trust boundary.
struct SkewedGenerator {
    air: BlockStateId,
    biome: mc_data::Identifier,
    geometry: mc_world::ChunkGeometry,
    shift: bool,
}

impl mc_world::ChunkGenerator for SkewedGenerator {
    fn generate(&self, pos: ChunkPos) -> mc_world::Chunk {
        let pos = if self.shift {
            ChunkPos {
                x: pos.x + 1,
                z: pos.z,
            }
        } else {
            pos
        };
        mc_world::Chunk::empty_with_geometry(pos, self.air, self.biome.clone(), self.geometry)
    }
}

fn probe_source(
    registry: &crate::registry::CoreRegistry,
    dimension: u32,
    generator: Arc<dyn mc_world::ChunkGenerator>,
) -> Arc<crate::chunk_loader::ChunkSource> {
    Arc::new(crate::chunk_loader::ChunkSource {
        native: None,
        dimension,
        generator,
        registry: Arc::clone(&registry.blocks),
        items: Arc::clone(&registry.items),
        chest_loot: Arc::clone(&registry.chest_loot),
        light: Arc::clone(&registry.light),
        geometry: mc_world::OVERWORLD_GEOMETRY,
    })
}

fn empty_source(
    registry: &crate::registry::CoreRegistry,
    dimension: u32,
) -> Arc<crate::chunk_loader::ChunkSource> {
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(EmptyGenerator {
        air: BlockStateId(registry.states.air),
        biome: mc_data::Identifier::parse("minecraft:plains").expect("plains parses"),
    });
    probe_source(registry, dimension, generator)
}
