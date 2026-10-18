use std::sync::Arc;

use mc_world::{BlockStateId, Chunk, ChunkGenerator, ChunkPos};
use rustc_hash::FxHashMap;

use crate::registry::CoreRegistry;
use crate::world::ChunkWorld;

struct EmptyWorld {
    air: u32,
    stone: u32,
    roof: bool,
}

impl ChunkGenerator for EmptyWorld {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(
            pos,
            BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        );
        if self.roof && pos == (ChunkPos { x: 0, z: 0 }) {
            for x in 0..16 {
                for z in 0..16 {
                    chunk.set_block(x, 101, z, BlockStateId(self.stone));
                }
            }
        }
        chunk
    }
}

#[cfg(test)]
fn light_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
}

fn world(dir: Option<std::path::PathBuf>, roof: bool) -> (ChunkWorld, Arc<CoreRegistry>) {
    let registry = Arc::new(CoreRegistry::load(Some(&light_root())).unwrap());
    let generator: Arc<dyn ChunkGenerator> = Arc::new(EmptyWorld {
        air: registry.states.air,
        stone: registry.states.stone,
        roof,
    });
    let air = BlockStateId(registry.states.air);
    let world = if let Some(dir) = dir {
        let config = crate::CoreConfig::new(dir, 42);
        ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&config).unwrap(),
            crate::native_checkpoint::dimension_id(crate::dimensions::Dimension::Overworld),
            generator,
            &registry,
            air,
            [0, 100, 0],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            42,
            generator,
            &registry,
            air,
            [0, 100, 0],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    (world, registry)
}

fn want(positions: &[ChunkPos]) -> FxHashMap<ChunkPos, i32> {
    positions.iter().map(|pos| (*pos, 0)).collect()
}

const CENTRE: ChunkPos = ChunkPos { x: 0, z: 0 };
const EAST: ChunkPos = ChunkPos { x: 1, z: 0 };

#[test]
fn torch_place_remove_relights_across_chunk_boundary_without_full_recompute() {
    let (mut world, registry) = world(None, false);
    world.ensure_blocking(&want(&[CENTRE, EAST]));
    assert_eq!(world.light_at(8, i32::MIN, 8), None);
    assert_eq!(world.light_at(8, i32::MAX, 8), None);
    assert_eq!(world.light_at(8, mc_world::MAX_Y, 8), None);
    let before = world.snapshot();
    let full = world.light.full_recomputes;
    assert!(
        full > 0,
        "blocking startup uses its full oracle before incremental tick edits"
    );
    world.set_block(15, 100, 8, registry.states.torch);
    world.fold_overlay();
    assert_eq!(world.light_at(15, 100, 8), Some((15, 14)));
    assert_eq!(world.light_at(16, 100, 8), Some((15, 13)));
    assert_eq!(world.light_at(28, 100, 8), Some((15, 1)));
    assert_eq!(world.light_at(29, 100, 8), Some((15, 0)));
    assert_eq!(
        world.light.full_recomputes, full,
        "single edits never recompute a chunk"
    );
    let old_east = before.iter().find(|entry| entry.chunk.pos == EAST).unwrap();
    assert_eq!(
        mc_world::light::ChunkLight::from_chunk(&old_east.chunk)
            .unwrap()
            .block_at(0, 100, 8),
        0,
        "snapshot stays immutable"
    );
    assert_ne!(
        world.chunk_revision(1, 0),
        old_east.revision,
        "neighbor light-only update bumps revision"
    );
    for old in before.iter() {
        let current = world.chunk(old.pos).unwrap();
        let changed_sections = old
            .chunk
            .section_lights
            .iter()
            .zip(&current.section_lights)
            .filter(|(previous, next)| previous != next)
            .count();
        assert!(
            changed_sections <= 3,
            "torch affects only its bounded vertical radius"
        );
    }
    world.set_block(15, 100, 8, registry.states.air);
    world.fold_overlay();
    for x in 2..=28 {
        assert_eq!(
            world.light_at(x, 100, 8).unwrap().1,
            0,
            "stale torch light at x={x}"
        );
    }
    assert_eq!(world.light.full_recomputes, full);
}

#[test]
fn removal_keeps_an_independent_neighbouring_emitter() {
    let (mut world, registry) = world(None, false);
    world.ensure_blocking(&want(&[CENTRE, EAST]));
    world.set_block(15, 100, 8, registry.states.torch);
    world.set_block(18, 100, 8, registry.states.torch);
    world.fold_overlay();
    world.set_block(15, 100, 8, registry.states.air);
    world.fold_overlay();
    assert_eq!(world.light_at(18, 100, 8).unwrap().1, 14);
    assert_eq!(world.light_at(15, 100, 8).unwrap().1, 11);
}

#[test]
fn sky_under_overhang_uses_sideways_light_and_reseeds_opened_column() {
    let (mut world, registry) = world(None, true);
    world.ensure_blocking(&want(&[CENTRE]));
    assert_eq!(world.light_at(8, 102, 8).unwrap().0, 15);
    let shaded = world.light_at(8, 100, 8).unwrap().0;
    assert!(
        shaded < 9 && shaded > 0,
        "overhang gets lateral sky light, got {shaded}"
    );
    assert_eq!(world.light_at(0, 100, 8).unwrap().0, 14);
    let full = world.light.full_recomputes;
    world.set_block(8, 101, 8, registry.states.air);
    world.fold_overlay();
    assert_eq!(world.light_at(8, 100, 8).unwrap().0, 15);
    assert_eq!(
        world.light_at(8, mc_world::MIN_Y, 8).unwrap().0,
        15,
        "opening skylight reseeds the full vertical column"
    );
    world.set_block(8, 101, 8, registry.states.stone);
    world.fold_overlay();
    assert_eq!(world.light_at(8, 100, 8).unwrap().0, shaded);
    assert_eq!(world.light.full_recomputes, full);
}

#[test]
fn late_neighbour_admission_recovers_boundary_light() {
    let (mut world, registry) = world(None, false);
    world.ensure_blocking(&want(&[CENTRE]));
    world.set_block(15, 100, 8, registry.states.torch);
    world.fold_overlay();
    assert_eq!(world.light_at(16, 100, 8), None);
    world.ensure_blocking(&want(&[CENTRE, EAST]));
    assert_eq!(world.light_at(16, 100, 8).unwrap().1, 13);
}

#[test]
fn checkpoint_persists_both_light_channels_and_journal_replay_removes_light() {
    let dir = tempfile::tempdir().unwrap();
    let (mut first, registry) = world(Some(dir.path().to_path_buf()), true);
    first.ensure_blocking(&want(&[CENTRE, EAST]));
    first.set_block(15, 100, 8, registry.states.torch);
    first.fold_overlay();
    let expected: Vec<_> = [CENTRE, EAST]
        .into_iter()
        .map(|pos| {
            (
                pos,
                mc_world::light::ChunkLight::from_chunk(first.chunk(pos).unwrap()).unwrap(),
            )
        })
        .collect();
    assert_eq!(first.flush_dirty(&Default::default()).unwrap(), 2);
    drop(first);
    let config = crate::CoreConfig::new(dir.path().to_owned(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let dimension = crate::native_checkpoint::dimension_id(crate::dimensions::Dimension::Overworld);
    for (pos, light) in &expected {
        let base = store
            .resolve_base(
                dimension,
                *pos,
                &registry.blocks,
                &registry.items,
                &registry.chest_loot,
                || Ok(None),
            )
            .unwrap()
            .expect("checkpoint persists the immutable native base");
        let chunk = store
            .compose(
                dimension,
                &base,
                &registry.blocks,
                &registry.items,
                &registry.chest_loot,
            )
            .unwrap();
        assert_eq!(
            mc_world::light::ChunkLight::from_chunk(&chunk).as_ref(),
            Some(light),
            "actual native storage retains sky+block arrays"
        );
    }
    drop(store);
    let mut writer = crate::journal::JournalWriter::open(dir.path(), 1).unwrap();
    writer
        .append(
            1,
            1,
            &[crate::journal::JournalEntry::Block {
                pos: [15, 100, 8],
                state: registry.states.air,
            }],
        )
        .unwrap();
    let batches =
        crate::journal::read_complete_batches_after(&dir.path().join("journal"), 0).unwrap();
    let replay = crate::journal::replay_batches(Default::default(), &batches);
    let (mut reopened, _) = world(Some(dir.path().to_path_buf()), true);
    for ((x, y, z), state) in replay.blocks {
        reopened.set_block(x, y, z, state);
    }
    reopened.ensure_blocking(&want(&[CENTRE, EAST]));
    let full = reopened.light.full_recomputes;
    reopened.fold_overlay();
    assert_eq!(reopened.light_at(15, 100, 8).unwrap().1, 0);
    assert_eq!(reopened.light_at(16, 100, 8).unwrap().1, 0);
    assert!(
        reopened.light_at(8, 100, 8).unwrap().0 < 9,
        "persisted roof still shades sky"
    );
    assert_eq!(
        reopened.light.full_recomputes, full,
        "replayed edits use incremental lighting too"
    );
}

#[test]
fn sky_darkening_is_time_and_weather_dependent_but_does_not_change_block_light() {
    assert_eq!(super::sky_darken(6_000, 0.0, 0.0), 0);
    assert_eq!(super::sky_darken(18_000, 0.0, 0.0), 11);
    assert_eq!(super::sky_darken(13_000, 0.0, 0.0), 6);
    assert_eq!(super::sky_darken(13_670, 0.0, 0.0), 11);
    assert_eq!(super::sky_darken(23_000, 0.0, 0.0), 6);
    assert!(super::sky_darken(6_000, 1.0, 1.0) > super::sky_darken(6_000, 1.0, 0.0));
    let (mut world, registry) = world(None, false);
    world.ensure_blocking(&want(&[CENTRE]));
    world.set_block(8, 100, 8, registry.states.torch);
    world.fold_overlay();
    let mut state = crate::tick::TickState::new(registry, world, 0, 64);
    state.day_time = 18_000;
    assert_eq!(state.brightness(8, 100, 8), 14);
    assert_eq!(state.brightness(0, 100, 0), 4);
    assert_eq!(
        state.world.light_at(0, 100, 0).unwrap().0,
        15,
        "night never changes baked skylight"
    );
}

fn admission_engine() -> super::LightEngine {
    super::LightEngine {
        table: Arc::new(mc_data::block_light::BlockLightTable::from_arrays(
            "admission-test",
            vec![0, 0, 0, 15, 0],
            vec![0, 15, 0, 0, 1],
            vec![true, false, true, true, false],
        )),
        cache: mc_world::light::LightCache::new(),
        workspace: mc_world::light::LightWorkspace::new(),
        admission_queue: Default::default(),
        admission_removal: Default::default(),
        admission_queued: Vec::new(),
        dirty_seams: Default::default(),
        admission_seed_cells: 0,
        full_recomputes: 0,
    }
}

fn admission_chunk(engine: &mut super::LightEngine, mut chunk: Chunk) -> Arc<Chunk> {
    let light = mc_world::light::compute_chunk_light_in(
        &mut engine.workspace,
        [[None; 3], [None, Some(&chunk), None], [None; 3]],
        &engine.table,
    );
    chunk.set_baked_light(&light);
    Arc::new(chunk)
}

fn admission_air(pos: ChunkPos, geometry: mc_world::ChunkGeometry, state: u32) -> Chunk {
    Chunk::empty_with_geometry(
        pos,
        BlockStateId(state),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
        geometry,
    )
}

fn assert_admission_matches_oracle(
    engine: &mut super::LightEngine,
    chunks: &mut FxHashMap<ChunkPos, Arc<Chunk>>,
    loaded: &[ChunkPos],
) {
    let mut targets: Vec<_> = chunks
        .keys()
        .copied()
        .filter(|pos| {
            loaded
                .iter()
                .any(|centre| (pos.x - centre.x).abs() <= 1 && (pos.z - centre.z).abs() <= 1)
        })
        .collect();
    targets.sort_unstable_by_key(|pos| (pos.x, pos.z));
    let untouched: Vec<_> = chunks
        .keys()
        .filter(|pos| !targets.contains(pos))
        .map(|pos| (*pos, engine.cache.get(*pos).cloned()))
        .collect();
    let updates = engine.admit(chunks, loaded);
    assert_eq!(
        updates.iter().map(|(pos, _)| *pos).collect::<Vec<_>>(),
        targets
    );
    for (pos, actual) in updates {
        let expected = mc_world::light::compute_chunk_light_in(
            &mut engine.workspace,
            super::neighbours(chunks, pos),
            &engine.table,
        );
        assert_eq!(
            actual, expected,
            "admission disagrees with the full oracle at {pos:?}"
        );
        assert_eq!(engine.cache.get(pos), Some(&actual));
        Arc::make_mut(chunks.get_mut(&pos).unwrap()).set_baked_light(&actual);
    }
    for (pos, previous) in untouched {
        assert_eq!(
            engine.cache.get(pos),
            previous.as_ref(),
            "halo cache changed at {pos:?}"
        );
    }
    assert_eq!(
        engine.full_recomputes, 0,
        "complete worker arrays must not be rebaked on tick"
    );
}

#[test]
fn admission_reconciles_sky_decreases_and_emitters_through_missing_corner_air() {
    let geometry = mc_world::ChunkGeometry::new(-32, 64).unwrap();
    let centre = ChunkPos { x: -3, z: -2 };
    let east = ChunkPos { x: -2, z: -2 };
    let diagonal = ChunkPos { x: -2, z: -1 };
    let north = ChunkPos { x: -3, z: -3 };
    let mut engine = admission_engine();
    let mut roof = admission_air(centre, geometry, 0);
    for x in 0..16 {
        for z in 0..16 {
            roof.set_block(x, -2, z, BlockStateId(1));
        }
    }
    let mut chunks = FxHashMap::default();
    chunks.insert(centre, admission_chunk(&mut engine, roof));
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[centre]);
    let before = engine.cache.get(centre).unwrap().sky_at(15, -4, 8);
    assert_eq!(before, 14, "missing air supplies side light under the roof");

    chunks.insert(
        east,
        admission_chunk(&mut engine, admission_air(east, geometry, 1)),
    );
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[east]);
    assert!(engine.cache.get(centre).unwrap().sky_at(15, -4, 8) < before);

    let mut emitter = admission_air(diagonal, geometry, 0);
    emitter.set_block(0, -4, 0, BlockStateId(3));
    chunks.insert(diagonal, admission_chunk(&mut engine, emitter));
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[diagonal]);
    assert_eq!(
        engine.cache.get(centre).unwrap().block_at(15, -4, 15),
        13,
        "the absent south chunk must transmit light around the opaque east chunk"
    );

    chunks.remove(&diagonal);
    engine.remove(diagonal);
    assert!(!engine.dirty_seams.is_empty());
    assert!(engine.dirty_seams.len() <= engine.cache.len());
    chunks.insert(
        north,
        admission_chunk(&mut engine, admission_air(north, geometry, 0)),
    );
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[north]);
    assert_eq!(engine.cache.get(centre).unwrap().block_at(15, -4, 15), 0);
    assert!(
        engine.dirty_seams.is_empty(),
        "installed targets consume eviction markers"
    );
}

#[test]
fn admission_asymmetric_roofs_holes_and_attenuation_match_oracle_in_any_batch_order() {
    for geometry in [
        mc_world::ChunkGeometry::new(-64, 96).unwrap(),
        mc_world::ChunkGeometry::new(0, 32).unwrap(),
        mc_world::ChunkGeometry::new(-32, 64).unwrap(),
    ] {
        let mut forward = admission_engine();
        let mut chunks = FxHashMap::default();
        let mut loaded = Vec::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let pos = ChunkPos {
                    x: -4 + dx,
                    z: 7 + dz,
                };
                let roof_y = geometry.min_y() + geometry.height() / 2 + dx + dz;
                let mut chunk = admission_air(pos, geometry, 0);
                for x in 0..16 {
                    for z in 0..16 {
                        if (x, z) != ((4 + dx) as u8, (7 + dz) as u8) {
                            chunk.set_block(x, roof_y, z, BlockStateId(1));
                        }
                    }
                }
                chunk.set_block(7, roof_y - 1, 7, BlockStateId(4));
                chunk.set_block(7, roof_y - 2, 7, BlockStateId(4));
                if (dx + dz) % 2 == 0 {
                    chunk.set_block(15, roof_y - 4, 15, BlockStateId(3));
                }
                chunks.insert(pos, admission_chunk(&mut forward, chunk));
                loaded.push(pos);
            }
        }
        let mut backwards = admission_engine();
        let mut reversed_chunks = chunks.clone();
        assert_admission_matches_oracle(&mut forward, &mut chunks, &loaded);
        loaded.reverse();
        assert_admission_matches_oracle(&mut backwards, &mut reversed_chunks, &loaded);
        for pos in loaded {
            assert_eq!(forward.cache.get(pos), backwards.cache.get(pos));
        }
    }
}

#[test]
fn admission_halo_reconciliation_does_not_publish_or_cache_wider_changes() {
    let geometry = mc_world::ChunkGeometry::new(-16, 32).unwrap();
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    let mut loaded = Vec::new();
    for x in 0..=4 {
        let pos = ChunkPos { x, z: 0 };
        let mut chunk = admission_air(pos, geometry, 0);
        chunk.set_block(15, 0, 8, BlockStateId(3));
        chunks.insert(pos, admission_chunk(&mut engine, chunk));
        loaded.push(pos);
    }
    assert_admission_matches_oracle(&mut engine, &mut chunks, &loaded);
    let snapshots = chunks.clone();
    let pos = ChunkPos { x: -1, z: 0 };
    chunks.insert(
        pos,
        admission_chunk(&mut engine, admission_air(pos, geometry, 1)),
    );
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[pos]);
    for x in 1..=4 {
        let pos = ChunkPos { x, z: 0 };
        assert!(Arc::ptr_eq(&chunks[&pos], &snapshots[&pos]));
    }
}

#[test]
fn worker_preserves_complete_arrays_and_bakes_omitted_layers() {
    let registry = CoreRegistry::load(Some(&light_root())).unwrap();
    let mut chunk = Chunk::empty(
        CENTRE,
        BlockStateId(registry.states.air),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.set_block(8, 0, 8, BlockStateId(registry.states.stone));
    chunk.set_baked_light(&mc_world::light::ChunkLight::filled(15, 15));
    super::bake_missing(&mut chunk, &registry.light);
    assert_eq!(
        mc_world::light::ChunkLight::from_chunk(&chunk).unwrap(),
        mc_world::light::ChunkLight::filled(15, 15),
        "complete producer arrays are not rebaked"
    );
    chunk.section_lights[0].sky = None;
    super::bake_missing(&mut chunk, &registry.light);
    let actual = mc_world::light::ChunkLight::from_chunk(&chunk).unwrap();
    let expected = mc_world::light::compute_chunk_light(
        [[None; 3], [None, Some(&chunk), None], [None; 3]],
        &registry.light,
    );
    assert_eq!(actual, expected);
    assert_eq!(actual.block_at(8, -1, 8), 0);
    assert_eq!(actual.sky_at(8, -1, 8), 14);
}

#[test]
fn admission_seeds_only_new_seams_in_a_cached_resident_halo() {
    let geometry = mc_world::ChunkGeometry::new(-16, 32).unwrap();
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    let mut resident = Vec::new();
    let roofed = |pos| {
        let mut chunk = admission_air(pos, geometry, 0);
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 0, z, BlockStateId(1));
            }
        }
        chunk
    };
    for z in -2..=2 {
        for x in -2..=2 {
            let pos = ChunkPos { x, z };
            if pos == CENTRE {
                continue;
            }
            chunks.insert(pos, admission_chunk(&mut engine, roofed(pos)));
            resident.push(pos);
        }
    }
    assert_admission_matches_oracle(&mut engine, &mut chunks, &resident);
    chunks.insert(CENTRE, admission_chunk(&mut engine, roofed(CENTRE)));
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[CENTRE]);
    assert!(
        engine.admission_seed_cells <= geometry.height() as usize * 128,
        "one admission must not seed every cached target/halo face: {} cells",
        engine.admission_seed_cells,
    );
}

#[test]
fn admission_also_seeds_uncached_real_context_seams() {
    let geometry = mc_world::ChunkGeometry::new(-16, 32).unwrap();
    let diagonal = ChunkPos { x: 1, z: 1 };
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    for pos in [CENTRE, EAST, diagonal] {
        let mut chunk = admission_air(pos, geometry, 0);
        if pos == EAST {
            // This source is too far from CENTRE's seam to start a correction
            // there, but must illuminate the other uncached resident.
            chunk.set_block(15, 0, 15, BlockStateId(3));
        }
        chunks.insert(pos, admission_chunk(&mut engine, chunk));
    }
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[CENTRE]);
    assert_eq!(engine.cache.get(diagonal).unwrap().block_at(15, 0, 0), 14);
}

#[test]
fn admission_uniform_bulk_allocates_no_queue_scratch() {
    let geometry = mc_world::ChunkGeometry::new(-32, 64).unwrap();
    for state in [0, 1] {
        let mut engine = admission_engine();
        let mut chunks = FxHashMap::default();
        let mut loaded = Vec::new();
        for z in -2..=2 {
            for x in -2..=2 {
                let pos = ChunkPos { x, z };
                chunks.insert(
                    pos,
                    admission_chunk(&mut engine, admission_air(pos, geometry, state)),
                );
                loaded.push(pos);
            }
        }
        assert_admission_matches_oracle(&mut engine, &mut chunks, &loaded);
        assert_eq!(engine.admission_seed_cells, 0, "uniform state {state}");
        assert_eq!(
            engine.admission_queue.capacity(),
            0,
            "uniform state {state}"
        );
        assert_eq!(
            engine.admission_queued.capacity(),
            0,
            "uniform state {state}"
        );
    }
}

#[test]
fn admission_matching_mixed_section_seams_need_no_cell_queue() {
    let geometry = mc_world::ChunkGeometry::new(-16, 32).unwrap();
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    for z in -2..=2 {
        for x in -2..=2 {
            let pos = ChunkPos { x, z };
            let mut chunk = admission_air(pos, geometry, 0);
            for y in -16..-8 {
                for z in 0..16 {
                    for x in 0..16 {
                        chunk.set_block(x, y, z, BlockStateId(1));
                    }
                }
            }
            let chunk = admission_chunk(&mut engine, chunk);
            if pos != CENTRE {
                engine.cache.insert(
                    pos,
                    mc_world::light::ChunkLight::from_chunk(&chunk).unwrap(),
                );
            }
            chunks.insert(pos, chunk);
        }
    }
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[CENTRE]);
    assert_eq!(engine.admission_seed_cells, 0);
    assert_eq!(engine.admission_queue.capacity(), 0);
    assert_eq!(engine.admission_queued.capacity(), 0);
}

#[test]
fn admission_cached_bulk_repairs_mixed_emitter_and_opaque_boundary_decreases() {
    let geometry = mc_world::ChunkGeometry::new(-32, 64).unwrap();
    let roofed = |pos| {
        let mut chunk = admission_air(pos, geometry, 0);
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 0, z, BlockStateId(1));
            }
        }
        chunk
    };
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    let mut loaded = Vec::new();
    for z in -1..=1 {
        for x in -1..=1 {
            let pos = ChunkPos { x, z };
            let mut chunk = if pos == EAST {
                admission_air(pos, geometry, 0)
            } else {
                roofed(pos)
            };
            if pos == CENTRE {
                chunk.set_block(15, -2, 15, BlockStateId(3));
            }
            chunks.insert(pos, admission_chunk(&mut engine, chunk));
            loaded.push(pos);
        }
    }
    assert_admission_matches_oracle(&mut engine, &mut chunks, &loaded);
    let previous_sky = engine.cache.get(CENTRE).unwrap().sky_at(15, -2, 8);
    let diagonal = ChunkPos { x: 1, z: 1 };
    assert_eq!(previous_sky, 14);
    assert_eq!(engine.cache.get(diagonal).unwrap().block_at(0, -2, 0), 13);

    // Recent chunks retain their authoritative derived arrays when evicted.
    // Return those old arrays alongside a changed emitter/opaque neighbour.
    let mut recent = std::mem::take(&mut chunks);
    for pos in &loaded {
        engine.remove(*pos);
    }
    assert!(engine.cache.is_empty());
    assert!(engine.dirty_seams.is_empty());
    recent.insert(CENTRE, admission_chunk(&mut engine, roofed(CENTRE)));
    recent.insert(
        EAST,
        admission_chunk(&mut engine, admission_air(EAST, geometry, 1)),
    );
    chunks = recent;
    loaded.reverse();
    assert_admission_matches_oracle(&mut engine, &mut chunks, &loaded);
    assert!(
        engine.admission_seed_cells > 0,
        "mixed seams still require reconciliation"
    );
    assert!(engine.cache.get(CENTRE).unwrap().sky_at(15, -2, 8) < previous_sky);
    assert_eq!(engine.cache.get(diagonal).unwrap().block_at(0, -2, 0), 0);
}

#[test]
fn admission_cached_dark_nonopaque_sections_relight_beside_missing_air() {
    let geometry = mc_world::ChunkGeometry::new(-16, 32).unwrap();
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    let mut loaded = Vec::new();
    for z in -1..=1 {
        for x in -1..=1 {
            let pos = ChunkPos { x, z };
            chunks.insert(
                pos,
                admission_chunk(&mut engine, admission_air(pos, geometry, 4)),
            );
            loaded.push(pos);
        }
    }
    assert_admission_matches_oracle(&mut engine, &mut chunks, &loaded);
    assert_eq!(engine.cache.get(CENTRE).unwrap().sky_at(0, 0, 8), 0);
    let recent = chunks.remove(&CENTRE).unwrap();
    chunks.clear();
    for pos in loaded {
        engine.remove(pos);
    }
    chunks.insert(CENTRE, recent);
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[CENTRE]);
    assert_eq!(engine.cache.get(CENTRE).unwrap().sky_at(0, 0, 8), 14);
}

#[test]
fn admission_uniform_new_chunk_receives_cached_diagonal_emitter_through_air() {
    let geometry = mc_world::ChunkGeometry::new(-16, 32).unwrap();
    let diagonal = ChunkPos { x: 1, z: 1 };
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    let mut source = admission_air(CENTRE, geometry, 0);
    source.set_block(15, -2, 15, BlockStateId(3));
    chunks.insert(CENTRE, admission_chunk(&mut engine, source));
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[CENTRE]);

    chunks.insert(
        diagonal,
        admission_chunk(&mut engine, admission_air(diagonal, geometry, 0)),
    );
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[diagonal]);
    assert_eq!(engine.cache.get(diagonal).unwrap().block_at(0, -2, 0), 13);
}

#[test]
fn blocking_bulk_uses_full_oracle_then_returns_to_incremental_admission() {
    let geometry = mc_world::ChunkGeometry::new(-16, 32).unwrap();
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    let mut loaded = Vec::new();
    for z in -1..=1 {
        for x in -1..=1 {
            let pos = ChunkPos { x, z };
            let mut chunk = admission_air(pos, geometry, 0);
            for x in 0..16 {
                for z in 0..16 {
                    chunk.set_block(x, 0, z, BlockStateId(1));
                }
            }
            if pos == CENTRE {
                chunk.set_block(15, -2, 15, BlockStateId(3));
            }
            chunks.insert(pos, admission_chunk(&mut engine, chunk));
            loaded.push(pos);
        }
    }
    let mut workspace = mc_world::light::LightWorkspace::new();
    for (pos, actual) in engine.admit_blocking(&chunks, &loaded) {
        let expected = mc_world::light::compute_chunk_light_in(
            &mut workspace,
            super::neighbours(&chunks, pos),
            &engine.table,
        );
        assert_eq!(actual, expected, "blocking target {pos:?}");
        assert_eq!(engine.cache.get(pos), Some(&actual));
        Arc::make_mut(chunks.get_mut(&pos).unwrap()).set_baked_light(&actual);
    }
    assert_eq!(engine.full_recomputes, loaded.len());
    assert_eq!(engine.admission_queue.capacity(), 0);
    assert_eq!(engine.admission_queued.capacity(), 0);
    let full = engine.full_recomputes;
    let new = ChunkPos { x: 2, z: 1 };
    chunks.insert(
        new,
        admission_chunk(&mut engine, admission_air(new, geometry, 0)),
    );
    for (pos, actual) in engine.admit(&chunks, &[new]) {
        let expected = mc_world::light::compute_chunk_light_in(
            &mut workspace,
            super::neighbours(&chunks, pos),
            &engine.table,
        );
        assert_eq!(actual, expected, "incremental target {pos:?}");
    }
    assert_eq!(
        engine.full_recomputes, full,
        "tick admission stays incremental"
    );
}

#[test]
fn admission_evicted_emitter_decreases_resident_when_a_different_neighbour_arrives() {
    let geometry = mc_world::ChunkGeometry::new(-16, 32).unwrap();
    let west = ChunkPos { x: -1, z: 0 };
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    let mut resident = admission_air(CENTRE, geometry, 0);
    resident.set_block(4, -2, 8, BlockStateId(3));
    let mut source = admission_air(EAST, geometry, 0);
    source.set_block(0, -2, 8, BlockStateId(3));
    chunks.insert(CENTRE, admission_chunk(&mut engine, resident));
    chunks.insert(EAST, admission_chunk(&mut engine, source));
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[CENTRE, EAST]);
    assert_eq!(engine.cache.get(CENTRE).unwrap().block_at(15, -2, 8), 14);

    chunks.remove(&EAST);
    engine.remove(EAST);
    assert!(engine.dirty_seams.contains(&CENTRE));
    chunks.insert(
        west,
        admission_chunk(&mut engine, admission_air(west, geometry, 0)),
    );
    // EAST is outside this batch's target square. The stale cached light in
    // CENTRE must still be removed, without erasing its independent emitter.
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[west]);
    let light = engine.cache.get(CENTRE).unwrap();
    assert_eq!(light.block_at(15, -2, 8), 4);
    assert_eq!(light.block_at(4, -2, 8), 15);
}

#[test]
fn admission_increasing_residual_cannot_mask_stale_dirty_halo_support() {
    let geometry = mc_world::ChunkGeometry::new(-16, 32).unwrap();
    let south = ChunkPos { x: 0, z: 1 };
    let diagonal = ChunkPos { x: 1, z: 1 };
    let far_south = ChunkPos { x: 0, z: 2 };
    let roofed = |pos| {
        let mut chunk = admission_air(pos, geometry, 0);
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 0, z, BlockStateId(1));
            }
        }
        chunk
    };
    let mut engine = admission_engine();
    let mut chunks = FxHashMap::default();
    let mut emitter = roofed(EAST);
    emitter.set_block(5, -2, 15, BlockStateId(3));
    // Columns force the old emitter path into CENTRE to be longer than its
    // later path through missing diagonal air. Prevent vertical shortcuts.
    for (x, z) in [(3, 15), (2, 14), (0, 14), (0, 13)] {
        for y in geometry.min_y()..0 {
            emitter.set_block(x, y, z, BlockStateId(1));
        }
    }
    chunks.insert(CENTRE, admission_chunk(&mut engine, roofed(CENTRE)));
    chunks.insert(EAST, admission_chunk(&mut engine, emitter));
    chunks.insert(south, admission_chunk(&mut engine, roofed(south)));
    chunks.insert(
        diagonal,
        admission_chunk(&mut engine, admission_air(diagonal, geometry, 1)),
    );
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[CENTRE, EAST, south, diagonal]);
    assert_eq!(engine.cache.get(CENTRE).unwrap().block_at(15, -2, 15), 5);

    chunks.remove(&diagonal);
    engine.remove(diagonal);
    chunks.insert(far_south, admission_chunk(&mut engine, roofed(far_south)));
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[far_south]);
    // SOUTH publishes the shorter path through virtual diagonal air. CENTRE
    // is a dirty halo: its scratch correction to 7 must not enter its cache.
    assert_eq!(engine.cache.get(south).unwrap().block_at(15, -2, 0), 8);
    assert_eq!(engine.cache.get(CENTRE).unwrap().block_at(15, -2, 15), 5);
    assert!(engine.dirty_seams.contains(&CENTRE));

    chunks.remove(&EAST);
    engine.remove(EAST);
    chunks.insert(
        diagonal,
        admission_chunk(&mut engine, admission_air(diagonal, geometry, 1)),
    );
    // CENTRE's east face is classified before SOUTH's masking old level 8
    // falls. Its increasing residual 5 -> 7 must invalidate old support too.
    assert_admission_matches_oracle(&mut engine, &mut chunks, &[diagonal]);
    assert_eq!(engine.cache.get(CENTRE).unwrap().block_at(15, -2, 15), 0);
    assert_eq!(engine.cache.get(south).unwrap().block_at(15, -2, 0), 0);
}
