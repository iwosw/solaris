use super::*;
use crate::boundary::ChunkPos;
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_entity_phase, run_tick};
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
enum Mode {
    Scheduled,
    Legacy,
    Serial,
    Parallel,
}

thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::Scheduled) };
    static MEASURE: Cell<bool> = const { Cell::new(false) };
    static ELAPSED: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

/// The pre-prototype map/collect is retained only as a test oracle. Thread-local
/// selection is set inside pool.install, never inherited by another test.
pub(super) fn override_run(ctx: &StepCtx<'_>, output: &mut Vec<RowOutput>) -> bool {
    let mode = MODE.get();
    if matches!(mode, Mode::Scheduled) {
        return false;
    }
    let start = MEASURE.get().then(Instant::now);
    match mode {
        Mode::Legacy => ctx
            .rows
            .par_iter()
            .with_min_len(256)
            .map(|row| row.alive.then(|| step_entity(*row, ctx)))
            .collect_into_vec(output),
        Mode::Serial => execute(ctx, output, false),
        Mode::Parallel => execute(ctx, output, true),
        Mode::Scheduled => unreachable!(),
    }
    if let Some(start) = start {
        ELAPSED.set(ELAPSED.get() + start.elapsed());
    }
    true
}

fn fixture(registry: &Arc<CoreRegistry>, count: usize) -> TickState {
    let world = crate::horde_bench::flat_world(registry);
    let mut state = TickState::new(Arc::clone(registry), world, 0, 512);
    state.wall_clock = || 1_767_225_600;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.day_time = 14_000;
    for x in 0..2 {
        for z in 0..2 {
            state
                .tickets
                .insert(format!("graph-{x}-{z}"), (ChunkPos { x, z }, 0));
        }
    }
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state
        .world
        .set_active_chunks(want.keys().copied().collect(), 0);
    let kinds: Vec<_> = [
        "minecraft:cow",
        "minecraft:pig",
        "minecraft:sheep",
        "minecraft:chicken",
    ]
    .map(|name| {
        registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    })
    .into_iter()
    .collect();
    for index in 0..count {
        let pos = [
            2.0 + (index % 28) as f64,
            64.0 + f64::from(index.is_multiple_of(7)),
            2.0 + ((index / 28) % 28) as f64,
        ];
        let id = state.spawn_mob(kinds[index % kinds.len()], pos, 20.0);
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.set_ground(slot, !index.is_multiple_of(7));
        state.entities.set_vel(slot, [0.03, 0.0, -0.02]);
        state.entities.row_mut(slot).persistent = true;
        if index < 8 {
            state.entities.row_mut(slot).love = 600;
        }
    }
    state
}

/// Canonical affected state, following replay_determinism_tests: dense entity
/// bodies/cold rows plus exact float bits, clocks, sequences and block images.
/// This is not a claim to hash every unrelated plugin or connection cache.
fn state_hash(state: &TickState) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    state.tick.hash(&mut hash);
    state.day_time.hash(&mut hash);
    state.entities.len().hash(&mut hash);
    for slot in state.entities.slots() {
        format!(
            "{:?}|{:?}",
            state.entities.body(slot),
            state.entities.row(slot)
        )
        .hash(&mut hash);
        state.entities.pos(slot).map(f64::to_bits).hash(&mut hash);
        state.entities.vel(slot).map(f64::to_bits).hash(&mut hash);
        state.entities.rot(slot).map(f32::to_bits).hash(&mut hash);
        state.entities.ground(slot).hash(&mut hash);
    }
    let mut seq: Vec<_> = state.seq_counter.iter().collect();
    seq.sort_by_key(|(actor, _)| **actor);
    seq.hash(&mut hash);
    let mut chunks = state.world.resident_positions();
    chunks.sort_by_key(|pos| (pos.x, pos.z));
    for pos in chunks {
        (pos.x, pos.z).hash(&mut hash);
        let chunk = state.world.chunk(pos).unwrap();
        for section in &chunk.sections {
            section.non_air_count().hash(&mut hash);
            if let (Some(palette), Some(indices)) = (section.palette(), section.indices()) {
                for block in palette {
                    block.0.hash(&mut hash);
                }
                indices.bits_per_entry().hash(&mut hash);
                indices.words().hash(&mut hash);
            } else {
                section.get(0, 0, 0).0.hash(&mut hash);
            }
        }
    }
    hash.finish()
}

#[test]
fn declared_system_matches_legacy_state_hashes_each_tick() {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let mut reference = Vec::new();
    for (mode, workers) in [
        (Mode::Legacy, 2),
        (Mode::Scheduled, 1),
        (Mode::Scheduled, 2),
        (Mode::Scheduled, 4),
    ] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        let hashes = pool.install(|| {
            MODE.set(mode);
            let mut state = fixture(&registry, 1024);
            let mut hashes = Vec::new();
            for tick in 0..48 {
                if tick == 12 {
                    // A geometry change must wake resting rows identically.
                    state.world.set_block(8, 63, 8, state.registry.states.air);
                    state.world.fold_overlay();
                }
                if tick == 24 {
                    // Compaction changes slot order, not entity identity.
                    state.remove_entity(crate::entities::EntitySlot(17));
                }
                let _ = run_tick(&mut state, &[]);
                hashes.push(state_hash(&state));
            }
            MODE.set(Mode::Scheduled);
            hashes
        });
        if matches!(mode, Mode::Legacy) {
            reference = hashes;
        } else {
            assert_eq!(hashes, reference, "{mode:?}, workers={workers}");
        }
    }
    eprintln!(
        "system-graph parity: 48 ticks, 1024 initial mobs, workers=1/2/4, final={:016x}",
        reference.last().unwrap()
    );
}

#[test]
fn declaration_fences_authority_and_preserves_empty_output() {
    assert_eq!(ENTITY_STEP.reads & ENTITY_STEP.writes, 0);
    assert_eq!(ENTITY_STEP.writes, bit(Resource::OrderedRowOutput));
    // Empty populations must not create rows or retain prior scratch output.
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let mut state = fixture(&registry, 0);
    assert!(run_entity_phase(&mut state, true).is_empty());
    assert!(state.entity_phase_scratch.writes.is_empty());
}

/// Run the built test binary, not Cargo, in Main's GO-controlled -x -p window.
/// Row-scope timing excludes fixture creation, prep, merge and hash cost. Full
/// phase timing includes prep/writeback; both consume identical seeded worlds.
#[test]
#[ignore = "exclusive two-core measurement; requires Main GO"]
fn measure_declared_system_two_cores() {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let mut expected = None;
    for count in [128, 8192] {
        for mode in [
            Mode::Parallel,
            Mode::Serial,
            Mode::Legacy,
            Mode::Serial,
            Mode::Parallel,
            Mode::Legacy,
        ] {
            let (row_time, phase_time, digest) = pool.install(|| {
                MODE.set(mode);
                let mut state = fixture(&registry, count);
                for _ in 0..8 {
                    let mut intents = run_entity_phase(&mut state, true);
                    state.merge(&mut intents);
                    state.tick += 1;
                }
                ELAPSED.set(Duration::ZERO);
                MEASURE.set(true);
                let start = Instant::now();
                for _ in 0..48 {
                    let mut intents = run_entity_phase(&mut state, true);
                    state.merge(&mut intents);
                    state.tick += 1;
                }
                let phase_time = start.elapsed();
                MEASURE.set(false);
                MODE.set(Mode::Scheduled);
                (ELAPSED.get(), phase_time, state_hash(&state))
            });
            if let Some((prior_count, prior_hash)) = expected
                && prior_count == count
            {
                assert_eq!(digest, prior_hash, "timing variants must remain identical");
            }
            expected = Some((count, digest));
            eprintln!(
                "system-graph count={count} mode={mode:?} workers=2 ticks=48 row_us={} phase_merge_us={} hash={digest:016x}",
                row_time.as_micros(),
                phase_time.as_micros()
            );
        }
    }
}
