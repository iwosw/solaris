use super::*;
use crate::farming_tests::{Y, block, put, setup};

#[test]
fn eligibility_reuses_revision_and_invalidates_edits_and_reloads() {
    let registry = crate::registry::CoreRegistry::load(None).expect("registry");
    let pos = mc_world::ChunkPos { x: -1, z: 2 };
    let mut chunk = Chunk::empty(
        pos,
        BlockStateId(registry.states.air),
        mc_data::Identifier::parse("minecraft:plains").expect("biome"),
    );
    let mut cache = FarmSections::default();
    assert_eq!(cache.mask(&registry.farming, &chunk, 1), 0);
    chunk.sections[0].set(0, 0, 0, BlockStateId(registry.states.grass_block));
    assert_eq!(
        cache.mask(&registry.farming, &chunk, 1),
        0,
        "unchanged revision reuses eligibility"
    );
    assert_eq!(
        cache.mask(&registry.farming, &chunk, 2),
        1,
        "committed edit enables section"
    );
    // A different chunk at the same position must not inherit eligibility.
    let inert = Chunk::empty(
        pos,
        BlockStateId(registry.states.air),
        mc_data::Identifier::parse("minecraft:plains").expect("biome"),
    );
    assert_eq!(
        cache.mask(&registry.farming, &inert, 3),
        0,
        "reload invalidates positive cache"
    );
    assert_eq!(
        cache.mask(&registry.farming, &chunk, 4),
        1,
        "reload invalidates negative cache"
    );
    assert_eq!(cache.chunks.len(), 1);
}

#[test]
fn active_projection_tracks_membership_admission_and_preserves_chunk_order() {
    use std::sync::Arc;

    let registry = crate::registry::CoreRegistry::load(None).expect("registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let mut world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        BlockStateId(registry.states.air),
        [0, 64, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let origin = ChunkPos { x: 0, z: 0 };
    let later = ChunkPos { x: 1, z: 0 };
    world.ensure_blocking(&FxHashMap::from_iter([(origin, 0)]));
    let mut cache = FarmSections::default();
    cache.refresh_active(&world);
    assert!(cache.active_chunks.is_empty());
    world.set_active_chunks(rustc_hash::FxHashSet::from_iter([origin, later]), 0);
    cache.refresh_active(&world);
    assert_eq!(cache.active_chunks, [origin]);
    world.ensure_blocking(&FxHashMap::from_iter([(origin, 0), (later, 0)]));
    world.refresh_active_clock(1);
    cache.refresh_active(&world);
    let ordered: Vec<_> = world
        .chunks()
        .filter(|chunk| world.is_block_active(chunk.pos.x, chunk.pos.z))
        .map(|chunk| chunk.pos)
        .collect();
    assert_eq!(cache.active_chunks, ordered);
    assert_eq!(cache.active_chunks.len(), 2);
    let revision = cache.active_revision;
    world.set_block(0, 64, 0, registry.states.stone);
    world.fold_overlay();
    cache.refresh_active(&world);
    assert_eq!(cache.active_revision, revision);
    assert_eq!(cache.active_chunks, ordered);
    world.set_active_chunks(rustc_hash::FxHashSet::default(), 2);
    cache.refresh_active(&world);
    assert!(cache.active_chunks.is_empty());
}

#[test]
fn sample_roll_prefix_matches_full_hash_fold() {
    // `sample` folds the (seed, tick, chunk, section) prefix once and mixes
    // in the sample index per draw; the seed must equal the historical
    // six-part fold so RNG draws, order and distribution are unchanged.
    for (seed, tick, cx, cz, index, sample) in [
        (42u64, 7u64, -1i32, 2i32, 0u64, 0u64),
        (42u64, 7u64, -1i32, 2i32, 3u64, 5u64),
        (1u64, 0u64, 0i32, 0i32, 15u64, 4095u64),
    ] {
        let prefix = hash(&[seed, tick, cx as u64, cz as u64, index]);
        assert_eq!(
            hash(&[seed, tick, cx as u64, cz as u64, index, sample]),
            mix(prefix ^ sample),
            "seed/tick/chunk/section/sample hash fold"
        );
    }
}
/// The pre-parallel `sample`: one serial pass drawing, classifying and ticking
/// each cell in turn.
fn sample_reference(state: &mut TickState, speed: u32) -> Vec<super::Change> {
    use super::{Roll, hash, mix};
    let farming = &state.registry.farming;
    let overlay = !state.world.overlay_is_empty();
    let mut changes = Vec::new();
    if speed == 0 {
        return changes;
    }
    state.farm_sections.refresh_active(&state.world);
    for active in 0..state.farm_sections.active_chunks.len() {
        let pos = state.farm_sections.active_chunks[active];
        let chunk = state
            .world
            .chunk(pos)
            .expect("cached active resident chunk");
        let mut ticking = state.farm_sections.mask(
            farming,
            chunk,
            state.world.chunk_revision(chunk.pos.x, chunk.pos.z),
        );
        while ticking != 0 {
            let index = ticking.trailing_zeros() as usize;
            ticking &= ticking - 1;
            let section = &chunk.sections[index];
            let prefix = hash(&[
                state.world.seed() as u64,
                state.tick,
                chunk.pos.x as u64,
                chunk.pos.z as u64,
                index as u64,
            ]);
            let base_y = chunk.geometry().min_y() + (index * mc_world::SECTION_DIM) as i32;
            for sample in 0..u64::from(speed) {
                let mut roll = Roll::new(mix(prefix ^ sample));
                let local = [roll.below(16), roll.below(16), roll.below(16)];
                let at = BlockPos {
                    x: chunk.pos.x * 16 + local[0] as i32,
                    y: base_y + local[1] as i32,
                    z: chunk.pos.z * 16 + local[2] as i32,
                };
                let mut id = section
                    .get(local[0] as u8, local[1] as u8, local[2] as u8)
                    .0;
                if overlay {
                    id = state.world.block_at(at.x, at.y, at.z).unwrap_or(id);
                }
                let kind = farming.kind(id);
                if kind.ticks() {
                    super::tick_cell(state, at, id, kind, &mut roll, &mut changes);
                }
            }
        }
    }
    changes
}

fn describe(changes: &[super::Change]) -> Vec<String> {
    changes
        .iter()
        .map(|change| {
            let outcome = match &change.outcome {
                super::Outcome::Set(cells) => format!("set {cells:?}"),
                super::Outcome::Plants(plan) => {
                    format!("plants {:?} {:?}", plan.expects, plan.writes)
                }
                super::Outcome::LeafDecay => "decay".to_owned(),
                super::Outcome::LeafRefresh => "refresh".to_owned(),
                super::Outcome::Pop => "pop".to_owned(),
                super::Outcome::Hatch { cells, spawn } => {
                    format!("hatch {} cells, {} spawn", cells.len(), spawn.len())
                }
            };
            format!("{:?} {} {outcome}", change.at, change.was)
        })
        .collect()
}

#[test]
fn parallel_sample_matches_serial_reference_order_and_rolls() {
    let (mut state, _) = setup();
    let wet = block(&state, "farmland", &[("moisture", "7")]);
    let dry = block(&state, "farmland", &[("moisture", "0")]);
    let grass = state.registry.states.grass_block;
    let kinds = [
        wet,
        dry,
        grass,
        block(&state, "wheat", &[("age", "2")]),
        block(&state, "oak_sapling", &[("stage", "0")]),
        block(
            &state,
            "oak_leaves",
            &[
                ("distance", "7"),
                ("persistent", "false"),
                ("waterlogged", "false"),
            ],
        ),
        block(&state, "sugar_cane", &[("age", "0")]),
        block(&state, "copper_block", &[]),
    ];
    for x in -10_i32..=10 {
        for z in -10_i32..=10 {
            let pick = ((x * 31 + z * 17).rem_euclid(8)) as usize;
            put(&mut state, x, Y, z, kinds[pick % 3]);
            put(&mut state, x, Y + 1, z, kinds[3 + pick % 5]);
        }
    }
    // Chunk activity (and so eligibility) is established by a real tick.
    let _ = crate::tick::run_tick(&mut state, &[]);
    let mut total = 0;
    for tick in 0..6 {
        state.tick += 1;
        let speed = [3, 40, 700][tick % 3];
        let expected = describe(&sample_reference(&mut state, speed));
        total += expected.len();
        assert_eq!(
            describe(&super::sample(&mut state, speed, usize::MAX)),
            expected
        );
        assert_eq!(describe(&super::sample(&mut state, speed, 0)), expected);
    }
    assert!(total > 100, "fixture must produce ticking cells: {total}");
    // Unfolded edits sit in chunk (0, 0) only: its samples read through the
    // overlay (a removed crop, a crop above bare air), the other chunks read
    // committed cells. Both must match the overlay-for-all reference.
    let air = state.registry.states.air;
    let sprout = block(&state, "wheat", &[("age", "0")]);
    state.world.set_block(1, Y + 1, 1, air);
    state.world.set_block(2, Y + 2, 2, sprout);
    assert!(!state.world.overlay_is_empty());
    for _ in 0..3 {
        state.tick += 1;
        let expected = describe(&sample_reference(&mut state, 700));
        assert_eq!(
            describe(&super::sample(&mut state, 700, usize::MAX)),
            expected
        );
    }
}

/// B,A,B,A micro: `cargo test -p mc-core --release farming::cache_tests::parallel_sample_micro -- --ignored --nocapture`.
#[test]
#[ignore = "prints a measurement"]
fn parallel_sample_micro() {
    let (mut state, _) = setup();
    let radius = 12;
    let mut active = rustc_hash::FxHashSet::default();
    let mut want = FxHashMap::default();
    for cx in -radius..=radius {
        for cz in -radius..=radius {
            active.insert(ChunkPos { x: cx, z: cz });
            want.insert(ChunkPos { x: cx, z: cz }, 0);
        }
    }
    state.world.ensure_blocking(&want);
    state.world.set_active_chunks(active, 0);
    let grass = state.registry.states.grass_block;
    let leaves = block(
        &state,
        "oak_leaves",
        &[
            ("distance", "1"),
            ("persistent", "true"),
            ("waterlogged", "false"),
        ],
    );
    for x in -radius * 16..(radius + 1) * 16 {
        for z in -radius * 16..(radius + 1) * 16 {
            state.world.set_block(x, Y, z, grass);
            if (x + z) % 7 == 0 {
                state.world.set_block(x, Y + 1, z, leaves);
            }
        }
    }
    // Native-like: every section of every chunk is eligible, most samples miss.
    for cx in -radius..=radius {
        for cz in -radius..=radius {
            for y in (-64..320).step_by(16) {
                state.world.set_block(cx * 16, y, cz * 16, grass);
            }
        }
    }
    state.world.fold_overlay();
    let mut bench = |parallel: bool| {
        let start = std::time::Instant::now();
        let mut hits = 0;
        for _ in 0..40 {
            state.tick += 1;
            hits += if parallel {
                super::sample(&mut state, 3, super::PARALLEL_MIN_CHUNKS).len()
            } else {
                sample_reference(&mut state, 3).len()
            };
        }
        assert!(hits > 0);
        start.elapsed().as_secs_f64() * 1000.0 / 40.0
    };
    for parallel in [false, true, false, true] {
        println!(
            "{}: {:.3} ms/tick",
            if parallel { "parallel" } else { "reference" },
            bench(parallel)
        );
    }
}
