//! Headless benchmark: mobs, moving players, loaded container signals, or a horde.
//!
//! Reports tick p50/p95/p99 and per-phase times as JSON under
//! `.analysis/codex-logs/core-v2-bench/`. `--label baseline` and `--label post`
//! preserve separate receipts; existing labeled receipts are never overwritten.
//! Mob scenes also report physical bytes/tick for complete published WAL
//! batches wholly inside the measured tick interval, excluding warmup/seeding.
//!
//! Container optimization boundary: inventory/lid mutations -> cached analog
//! and trapped-chest signals, not chunk hydration, journal scans or automation.
//! The baseline bottleneck is redstone's per-tick walk over every inventory and
//! its slots, even when only four chests change. Dirty consumption belongs before
//! scheduled components; automation mutations retain their next-tick timing.
//! Correctness fence: every mutable storage/viewer access, insert/remove/clear
//! and reload must invalidate both affected chest halves, including transitions
//! to zero. This scene checks settled analog outputs and loaded nonempty storage;
//! double chests, viewers, removals and restart need separate regression gates.
//! Removal path: if parity or measured benefit fails, remove dirty signal caching
//! and tracking together and restore the full scan; retain this unchanged scene
//! as the A/B receipt producer, rather than bypassing ordinary run_tick phases.
//!
//! Horde optimization boundary (`horde100`, `horde1000`: 10k zombies, 5k villagers
//! and 100/1000 invulnerable survival players in one flat arena; `peaceful100`:
//! the no-regression scene with 50k villagers, 20k cows and no hostile mob): chaser target
//! search through the per-tick TargetGrid (target_grid.rs, ADR 0013) instead of a
//! linear scan of every player and villager per thinking zombie. Compare
//! `phases.entity` p50/p99 of a `--label before` and `--label after` run; the
//! scene drives ordinary run_tick and fences population and zombie movement.
//! Removal path: see target_grid.rs.

use std::time::Instant;

use mc_core::boundary::{ConnectionId, PlayerChange, PlayerInput};
use mc_core::{Core, CoreConfig};

#[cfg(test)]
#[path = "../core_bench_tests.rs"]
mod wal_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Spread,
    Crowded,
    Mixed,
    Moving,
    Containers,
    Horde100,
    Horde1000,
    Peaceful100,
}

impl Scenario {
    fn name(&self) -> &'static str {
        match self {
            Scenario::Spread => "spread-100k",
            Scenario::Crowded => "crowded-10k",
            Scenario::Mixed => "mixed",
            Scenario::Moving => "moving-100k-8players",
            Scenario::Containers => "containers-20k",
            Scenario::Horde100 => "horde-10kz-5kv-100p",
            Scenario::Horde1000 => "horde-10kz-5kv-1000p",
            Scenario::Peaceful100 => "peaceful-50kv-20kc-100p",
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let scenario = args
        .first()
        .map(|arg| match arg.as_str() {
            "spread" => Scenario::Spread,
            "crowded" => Scenario::Crowded,
            "mixed" => Scenario::Mixed,
            "moving" => Scenario::Moving,
            "containers" => Scenario::Containers,
            "horde100" => Scenario::Horde100,
            "horde1000" => Scenario::Horde1000,
            "peaceful100" => Scenario::Peaceful100,
            _ => Scenario::Mixed,
        })
        .unwrap_or(Scenario::Mixed);
    let ticks: usize = args.get(1).and_then(|arg| arg.parse().ok()).unwrap_or(300);
    assert!(ticks > 0, "benchmark requires at least one measured tick");
    // Optional third argument `cows` retains the existing all-cow scenes.
    let cows_only = !matches!(
        scenario,
        Scenario::Containers | Scenario::Horde100 | Scenario::Horde1000 | Scenario::Peaceful100
    ) && args.get(2).map(String::as_str) == Some("cows");
    let label = args.iter().position(|arg| arg == "--label").map(|index| {
        let value = args.get(index + 1).expect("--label requires a value");
        assert!(
            !value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
            "label must contain only ASCII letters, digits, '-' or '_'"
        );
        value.as_str()
    });
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.analysis/codex-logs/core-v2-bench");
    std::fs::create_dir_all(&dir).expect("bench log dir");
    let mix = if cows_only { "-cows" } else { "" };
    let suffix = label.map(|value| format!("-{value}")).unwrap_or_default();
    let path = dir.join(format!(
        "{}{mix}-ticks{ticks}{suffix}.json",
        scenario.name()
    ));
    // Reserve labeled receipts before running. A baseline command cannot
    // accidentally truncate its receipt when the optimized binary is invoked.
    let mut receipt = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .create_new(label.is_some())
        .truncate(label.is_none())
        .open(&path)
        .expect("open receipt (choose a fresh --label if it already exists)");
    let mut out = match scenario {
        Scenario::Containers => run_containers(ticks),
        Scenario::Horde100 => run_horde(ticks, 100, scenario),
        Scenario::Horde1000 => run_horde(ticks, 1000, scenario),
        Scenario::Peaceful100 => run_horde(ticks, 100, scenario),
        _ => run_scenario(scenario, ticks, cows_only),
    };
    out["label"] = serde_json::json!(label);
    serde_json::to_writer_pretty(&mut receipt, &out).expect("write receipt");
    println!("{out}");
    println!("wrote {}", path.display());
}

fn acknowledge_teleports(core: &Core) {
    for (connection, changes) in core.try_drain_outbox() {
        for change in changes {
            let teleport_id = match change {
                // JoinBurst sends the initial synchronization with id 1.
                PlayerChange::Joined { .. } => 1,
                PlayerChange::PositionCorrected { teleport_id, .. } => teleport_id,
                _ => continue,
            };
            core.submit(PlayerInput::ConfirmTeleport {
                connection,
                teleport_id,
            });
        }
    }
}

/// Vanilla content for bench registries: `SOLARIS_CONTENT_CACHE`, else the
/// repository's local `data/vanilla` extraction.
fn content_dir() -> std::path::PathBuf {
    std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        })
}

fn run_scenario(scenario: Scenario, ticks: usize, cows_only: bool) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!(
        "core-bench-{}-{}",
        scenario.name(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = CoreConfig::new(dir.clone(), 42);
    config.simulation_distance = 8;
    // Small broadcast buffer: each TickOutput holds the chunk set + 100k
    // views; a deep buffer would pin GBs. The bench drains every tick.
    config.broadcast_cap = 4;
    config.vanilla_data_dir = Some(content_dir());
    let core = Core::open(config).expect("open");
    // Existing scenes retain their single stationary player. Moving uses eight
    // real boundary-input players; UUIDs identify their committed wire views.
    let players: Vec<_> = (1..=if scenario == Scenario::Moving { 8 } else { 1 })
        .map(|id| (ConnectionId(id), uuid::Uuid::new_v4()))
        .collect();
    for &(connection, uuid) in &players {
        core.submit(PlayerInput::Join {
            connection,
            uuid,
            name: if scenario == Scenario::Moving {
                format!("bench{}", connection.0)
            } else {
                "bench".to_owned()
            },
            permission_level: 0,
        });
    }
    // Let the world settle for a few ticks (worldgen-heavy, excluded).
    // A closed channel here means the tick thread died: fail loudly.
    let mut rx = core.subscribe();
    for _ in 0..5 {
        rx.blocking_recv().expect("tick thread alive during warmup");
        if scenario == Scenario::Moving {
            acknowledge_teleports(&core);
        }
    }
    // Spawn mobs directly through the tick state? No: use the public path.
    // core-bench drives load via debug-spawn inputs is out of scope, so it
    // seeds entities through `debug_spawn` on a test hook below.
    seed_entities(&core, scenario, cows_only, &mut rx);
    if scenario == Scenario::Moving {
        acknowledge_teleports(&core);
    }
    // Measure SERVER-SIDE tick cost: each received TickOutput carries its
    // own timings, so a sample is bound to its tick. Pace on broadcast
    // (never time the gaps); falling behind fails loudly below instead of
    // silently dropping ticks from the sample.
    let mut tick_times: Vec<f64> = Vec::with_capacity(ticks);
    let mut phases: Vec<[f64; 5]> = Vec::with_capacity(ticks);
    let t0 = Instant::now();
    let mut previous_positions = vec![None; players.len()];
    let mut moved_player_ticks = 0usize;
    let mut first_tick = None;
    let mut last_tick = 0;
    while tick_times.len() < ticks {
        let output = match rx.blocking_recv() {
            Ok(output) => output,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                panic!(
                    "bench fell behind the tick thread (slow consumer): skipped {skipped} ticks; results would undercount — reduce load or raise channel capacity"
                );
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                panic!("tick thread died mid-bench (recv closed)");
            }
        };
        first_tick.get_or_insert(output.tick);
        last_tick = output.tick;
        let times = output.times;
        if scenario == Scenario::Moving {
            acknowledge_teleports(&core);
            for (index, &(connection, uuid)) in players.iter().enumerate() {
                let view = output
                    .snapshot
                    .entities
                    .iter()
                    .find(|view| view.uuid == uuid)
                    .expect("moving player remains in the scene");
                if previous_positions[index].is_some_and(|position| position != view.position) {
                    moved_player_ticks += 1;
                }
                previous_positions[index] = Some(view.position);
                // Walk back and forth within the already resident spawn window.
                // Read the committed y, not a guessed terrain height.
                let direction = if (tick_times.len() / 80 + index).is_multiple_of(2) {
                    0.08
                } else {
                    -0.08
                };
                core.submit(PlayerInput::Move {
                    connection,
                    position: [
                        view.position[0] + direction,
                        view.position[1],
                        view.position[2],
                    ],
                    yaw: if direction > 0.0 { -90.0 } else { 90.0 },
                    pitch: 0.0,
                    on_ground: view.on_ground,
                });
            }
        }
        tick_times.push(times.total_us as f64 / 1000.0);
        phases.push([
            times.entity_us as f64 / 1000.0,
            times.merge_us as f64 / 1000.0,
            times.block_us as f64 / 1000.0,
            times.snapshot_us as f64 / 1000.0,
            times.views_us as f64 / 1000.0,
        ]);
    }
    let wall = t0.elapsed().as_secs_f64();
    // Inspect immutable published batches before shutdown's final checkpoint
    // can prune them. An unflushed boundary batch is excluded, never estimated.
    let wal = wal_metrics(&dir, first_tick.expect("measured tick"), last_tick);
    core.shutdown();
    if scenario == Scenario::Moving {
        assert!(
            moved_player_ticks >= ticks.saturating_sub(1) * players.len() / 2,
            "moving scene must commit movement, not only submit refused inputs"
        );
    }
    tick_times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| {
        let idx = ((p * tick_times.len() as f64).ceil() as usize)
            .saturating_sub(1)
            .min(tick_times.len().saturating_sub(1));
        tick_times.get(idx).copied().unwrap_or(0.0)
    };
    let report = serde_json::json!({
        "scenario": scenario.name(),
        "ticks": ticks,
        "players": players.len(),
        "moved_player_ticks": moved_player_ticks,
        "wall_seconds": wall,
        "wal": wal,
        "tick_ms": {
            "p50": pct(0.50),
            "p95": pct(0.95),
            "p99": pct(0.99),
            "max": tick_times.last().copied().unwrap_or(0.0),
        },
        "phases": phase_stats(&phases),
        "machine": machine_description(),
    });
    report
}

fn run_containers(ticks: usize) -> serde_json::Value {
    const WARMUP_TICKS: usize = 8;
    let mut scene = mc_core::ContainerBench::new(&content_dir()).expect("container scene");
    for _ in 0..WARMUP_TICKS {
        scene.tick();
    }
    assert_eq!(scene.assert_scene(), 15);
    let loaded_chunks = scene.loaded_chunks();
    let mut all_ticks = Vec::with_capacity(ticks * 2);
    let mut all_phases = Vec::with_capacity(ticks * 2);
    let start = Instant::now();
    let mut workloads = serde_json::Map::new();
    for (name, mutate) in [("idle", false), ("mutating", true)] {
        let mut tick_times = Vec::with_capacity(ticks);
        let mut phases = Vec::with_capacity(ticks);
        for tick in 0..ticks {
            if mutate {
                scene.mutate(tick);
            }
            let times = scene.tick();
            tick_times.push(times.total_us as f64 / 1000.0);
            phases.push([
                times.entity_us as f64 / 1000.0,
                times.merge_us as f64 / 1000.0,
                times.block_us as f64 / 1000.0,
                times.snapshot_us as f64 / 1000.0,
                times.views_us as f64 / 1000.0,
            ]);
        }
        all_ticks.extend_from_slice(&tick_times);
        all_phases.extend_from_slice(&phases);
        workloads.insert(
            name.to_owned(),
            serde_json::json!({
                "ticks": ticks,
                "inventory_mutations_per_tick": if mutate { mc_core::ContainerBench::COMPARATORS } else { 0 },
                "tick_ms": tick_stats(&mut tick_times),
                "phases": phase_stats(&phases),
            }),
        );
    }
    let wall = start.elapsed().as_secs_f64();
    // Untimed, bidirectional correctness fence: neither "never changed" nor
    // "only increases work" can pass merely because the last sample was full.
    scene.mutate(0);
    for _ in 0..WARMUP_TICKS {
        scene.tick();
    }
    assert_eq!(scene.assert_scene(), 4);
    scene.mutate(1);
    for _ in 0..WARMUP_TICKS {
        scene.tick();
    }
    assert_eq!(scene.assert_scene(), 15);
    assert_eq!(scene.loaded_chunks(), loaded_chunks);
    serde_json::json!({
        "scenario": Scenario::Containers.name(),
        "scene_version": 1,
        "seed": 42,
        "runner": "direct-run_tick",
        "timing_scope": "run_tick only; excludes mutation setup, pacing, journal I/O and output disposal",
        "ticks": ticks * 2,
        "ticks_per_workload": ticks,
        "warmup_ticks": WARMUP_TICKS,
        "correctness_fence_ticks": WARMUP_TICKS * 2,
        "players": 0,
        "chests": mc_core::ContainerBench::CHESTS,
        "slots_per_chest": 27,
        "item": "minecraft:stone",
        "initial_stack_count": 64,
        "mutation_stack_counts": [16, 64],
        "comparators": mc_core::ContainerBench::COMPARATORS,
        "loaded_chunks": loaded_chunks,
        "verified_comparator_outputs": [4, 15],
        "wall_seconds": wall,
        "tick_ms": tick_stats(&mut all_ticks),
        "phases": phase_stats(&all_phases),
        "workloads": workloads,
        "machine": machine_description(),
    })
}

fn run_horde(ticks: usize, players: usize, scenario: Scenario) -> serde_json::Value {
    const WARMUP_TICKS: usize = 30;
    let mut scene = if scenario == Scenario::Peaceful100 {
        mc_core::HordeBench::peaceful(players)
    } else {
        mc_core::HordeBench::horde(players)
    }
    .expect("horde scene");
    for _ in 0..WARMUP_TICKS {
        scene.tick();
    }
    let loaded_chunks = scene.loaded_chunks();
    let mut tick_times = Vec::with_capacity(ticks);
    let mut phases = Vec::with_capacity(ticks);
    let start = Instant::now();
    for _ in 0..ticks {
        let times = scene.tick();
        tick_times.push(times.total_us as f64 / 1000.0);
        phases.push([
            times.entity_us as f64 / 1000.0,
            times.merge_us as f64 / 1000.0,
            times.block_us as f64 / 1000.0,
            times.snapshot_us as f64 / 1000.0,
            times.views_us as f64 / 1000.0,
        ]);
    }
    let wall = start.elapsed().as_secs_f64();
    let (zombies, villagers, cows, players_alive, moved_watched) = scene.assert_scene();
    serde_json::json!({
        "scenario": scenario.name(),
        "scene_version": 1,
        "runner": "direct-run_tick",
        "timing_scope": "run_tick only; excludes pacing, journal I/O and output disposal",
        "ticks": ticks,
        "warmup_ticks": WARMUP_TICKS,
        "players": players_alive,
        "zombies": zombies,
        "villagers": villagers,
        "cows": cows,
        "moved_watched_mobs": moved_watched,
        "loaded_chunks": loaded_chunks,
        "wall_seconds": wall,
        "tick_ms": tick_stats(&mut tick_times),
        "phases": phase_stats(&phases),
        "machine": machine_description(),
    })
}

fn tick_stats(tick_times: &mut [f64]) -> serde_json::Value {
    tick_times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| {
        let index = ((p * tick_times.len() as f64).ceil() as usize)
            .saturating_sub(1)
            .min(tick_times.len().saturating_sub(1));
        tick_times.get(index).copied().unwrap_or(0.0)
    };
    serde_json::json!({
        "p50": pct(0.50),
        "p95": pct(0.95),
        "p99": pct(0.99),
        "max": tick_times.last().copied().unwrap_or(0.0),
    })
}

/// Seed entities for the scenario through direct tick-state access.
/// core-bench is an in-crate binary: it may use `mc_core` internals via the
/// `bench_seed` hook.
fn seed_entities(
    core: &Core,
    scenario: Scenario,
    cows_only: bool,
    rx: &mut tokio::sync::broadcast::Receiver<mc_core::boundary::TickOutput>,
) {
    let (count, spread) = match scenario {
        Scenario::Spread => (100_000, true),
        Scenario::Crowded => (10_000, false),
        Scenario::Mixed => (50_000, true),
        Scenario::Moving => (100_000, true),
        Scenario::Containers | Scenario::Horde100 | Scenario::Horde1000 | Scenario::Peaceful100 => {
            unreachable!("direct-tick scenes have their own owners")
        }
    };
    let zombies = if cows_only { 0 } else { count / 2 };
    mc_core::bench_seed(core, count, spread, zombies);
    // Wait for the population in TickOutput (never a fixed sleep): the seeded
    // entities commit over the next ticks; start measuring at full load.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let seeded = loop {
        let output = match rx.blocking_recv() {
            Ok(output) => output,
            // bench_seed drains via its own receiver while seeding; lag here
            // is expected, not a slow-consumer failure (the measurement loop
            // below keeps the strict policy).
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                panic!("tick thread died while seeding")
            }
        };
        if output.snapshot.entities.len() >= count {
            break output;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "seeded population never committed: have {} want {count}",
                output.snapshot.entities.len()
            );
        }
    };
    // Seeding validity: spread scenarios must tile the full grid (bbox spans
    // the expected area) and the snapshot must cover the seeded chunks.
    if spread {
        let side = (count as f64).sqrt().ceil();
        let expect_span = side * 2.0;
        let (mut min_x, mut max_x) = (f64::MAX, f64::MIN);
        let (mut min_z, mut max_z) = (f64::MAX, f64::MIN);
        for view in &seeded.snapshot.entities {
            min_x = min_x.min(view.position[0]);
            max_x = max_x.max(view.position[0]);
            min_z = min_z.min(view.position[2]);
            max_z = max_z.max(view.position[2]);
        }
        assert!(
            max_x - min_x >= expect_span * 0.9 && max_z - min_z >= expect_span * 0.9,
            "spread must tile the full grid: x span {} z span {}, want {expect_span}",
            max_x - min_x,
            max_z - min_z,
        );
        let expect_chunks = ((expect_span / 16.0).ceil() as usize).pow(2);
        assert!(
            seeded.snapshot.chunks.len() >= expect_chunks / 2,
            "seeded chunks resident: {} want ~{expect_chunks}",
            seeded.snapshot.chunks.len(),
        );
    }
}

/// Physical bytes and covered ticks of complete batches in the sample.
fn wal_metrics(root: &std::path::Path, first_tick: u64, last_tick: u64) -> serde_json::Value {
    let mut bytes = 0u64;
    let mut ticks = 0u64;
    let mut batches = 0u64;
    let mut first = u64::MAX;
    let mut last = 0;
    for entry in std::fs::read_dir(root.join("journal")).expect("read benchmark WAL") {
        let entry = entry.expect("read benchmark WAL entry");
        let name = entry.file_name();
        let Some(interval) = name
            .to_str()
            .and_then(|name| name.strip_prefix("batch-"))
            .and_then(|name| name.strip_suffix(".bin"))
        else {
            continue;
        };
        let (lo, hi) = interval.split_once('-').expect("published WAL interval");
        let lo: u64 = lo.parse().expect("published WAL first tick");
        let hi: u64 = hi.parse().expect("published WAL last tick");
        if lo < first_tick || hi > last_tick {
            continue;
        }
        bytes += entry.metadata().expect("published WAL metadata").len();
        ticks += hi.checked_sub(lo).expect("ordered WAL interval") + 1;
        batches += 1;
        first = first.min(lo);
        last = last.max(hi);
    }
    assert!(ticks > 0, "no complete WAL batches inside measured ticks");
    serde_json::json!({
        "bytes": bytes,
        "ticks": ticks,
        "bytes_per_tick": bytes as f64 / ticks as f64,
        "batches": batches,
        "first_tick": first,
        "last_tick": last,
    })
}

/// Per-phase p50/p95 aggregates over the measured ticks.
fn phase_stats(phases: &[[f64; 5]]) -> serde_json::Value {
    let names = ["entity", "merge", "block", "snapshot", "views"];
    let mut map = serde_json::Map::new();
    for (i, name) in names.iter().enumerate() {
        let mut col: Vec<f64> = phases.iter().map(|row| row[i]).collect();
        col.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pct = |p: f64| {
            col.get(
                ((p * col.len() as f64).ceil() as usize)
                    .saturating_sub(1)
                    .min(col.len().saturating_sub(1)),
            )
            .copied()
            .unwrap_or(0.0)
        };
        map.insert(
            (*name).to_owned(),
            serde_json::json!({"p50": pct(0.50), "p95": pct(0.95), "p99": pct(0.99)}),
        );
    }
    serde_json::Value::Object(map)
}

fn machine_description() -> serde_json::Value {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    let mem: String = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("MemTotal:"))
                .map(|line| line.to_owned())
        })
        .unwrap_or_default();
    let cpu: String = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("model name"))
                .map(|line| line.to_owned())
        })
        .unwrap_or_default();
    serde_json::json!({ "cpus": cpus, "mem_total": mem.trim(), "cpu": cpu.trim(), "extra": std::env::var("CORE_BENCH_MACHINE").unwrap_or_default() })
}
