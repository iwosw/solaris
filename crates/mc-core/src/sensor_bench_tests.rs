//! Same entity-phase fixture on the base and candidate binaries. No paced
//! thread, I/O, natural spawns or damage merge in the measurement boundary.
use super::*;

#[cfg(debug_assertions)]
thread_local! {
    pub(super) static COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

fn scene(players: u64) -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    let chunks: Vec<_> = (-2..6)
        .flat_map(|x| (-2..6).map(move |z| ChunkPos { x, z }))
        .collect();
    state
        .world
        .ensure_blocking(&chunks.iter().map(|&pos| (pos, 0)).collect());
    state
        .world
        .set_active_chunks(chunks.into_iter().collect(), 1);
    state.day_time = 14_000;
    for index in 0..players {
        let connection = ConnectionId(index + 1);
        input::session::join(
            &mut state,
            (
                &connection,
                &uuid::Uuid::from_u128(u128::from(index + 1)),
                &format!("sensor{index}"),
                &0,
            ),
            &mut ChangeSet::default(),
        );
        let player = state.connections.entity_of(connection).unwrap();
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.set_pos(
            slot,
            [
                6.5 + (index % 5) as f64 * 12.0,
                64.0,
                8.5 + (index / 5) as f64 * 16.0,
            ],
        );
        state.entities.set_ground(slot, true);
        state.entities.row_mut(slot).protected_until = u64::MAX;
    }
    let kind = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    for index in 0..1000 {
        let id = state.spawn_mob(
            kind,
            [
                0.5 + f64::from(index % 32) * 2.0,
                64.0,
                0.5 + f64::from(index / 32) * 2.0,
            ],
            20.0,
        );
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.set_ground(slot, true);
    }
    state.entity_phase_revision = state.world.revision_now();
    state
}

#[test]
#[cfg(debug_assertions)]
fn sensors_count_dense_acquisition_retention_and_quiet_queries() {
    let mut dense = scene(20);
    let _ = run_entity_phase(&mut dense, true);
    assert_eq!(COUNTS.get(), (1000, 16));
    assert_eq!(dense.hostile.targets.len(), 1000);
    dense.tick += 1;
    let _ = run_entity_phase(&mut dense, true);
    assert_eq!(COUNTS.get(), (0, 0));
    assert_eq!(dense.hostile.targets.len(), 1000);
    let mut quiet = scene(0);
    let _ = run_entity_phase(&mut quiet, true);
    assert_eq!(COUNTS.get(), (0, 0));
    assert!(quiet.hostile.targets.is_empty());
    eprintln!("SENSOR_QUERIES dense_acquisition=1000 broad_phases=16 dense_retained=0 quiet=0");
}

#[test]
#[ignore = "release timing only under Main's GO and mem-run -x -p"]
fn sensors_release_entity_phase_dense_and_quiet() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    for players in [20, 0] {
        let mut samples = Vec::new();
        for sample in 0..3 {
            let mut state = scene(players);
            pool.install(|| {
                for _ in 0..120 {
                    state.tick += 1;
                    let _ = run_entity_phase(&mut state, true);
                }
            });
            let start = std::time::Instant::now();
            pool.install(|| {
                for _ in 0..600 {
                    state.tick += 1;
                    std::hint::black_box(run_entity_phase(&mut state, true));
                }
            });
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(state.entities.len(), 1000 + players as usize);
            assert!(state.entities.slots().all(|slot| {
                let chunk = state.entities.chunk_of(slot);
                state.world.is_active_position(chunk.x, chunk.z)
            }));
            samples.push(elapsed);
            eprintln!(
                "SENSORS_TIME players={players} mobs=1000 sample={sample} entity_phase_600_ticks_ms={elapsed:.6}"
            );
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "SENSORS_MEDIAN players={players} mobs=1000 entity_phase_ms={:.6}",
            samples[1] / 600.0
        );
    }
}
