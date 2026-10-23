//! In-process merge attribution. Ignored release fixture; no production probes.
use std::cell::RefCell;
use std::fmt::Write;
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::HordeBench;
use crate::tick::{TickState, run_tick};

#[derive(Clone, Copy, Default)]
pub(crate) struct Probe {
    pub sort_ns: u64,
    pub validation_ns: u64,
    pub journal_sort_ns: u64,
    pub effects_ns: u64,
    pub intents: usize,
    pub accepted: usize,
}

thread_local! {
    static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

pub(crate) fn start() -> Option<Instant> {
    PROBE.with(|probe| probe.borrow().as_ref().map(|_| Instant::now()))
}

pub(crate) fn record(start: Option<Instant>, update: impl FnOnce(&mut Probe, u64)) {
    if let Some(start) = start {
        let ns = start.elapsed().as_nanos() as u64;
        PROBE.with(|probe| update(probe.borrow_mut().as_mut().expect("enabled probe"), ns));
    }
}

struct HashWriter(Sha256);
impl std::fmt::Write for HashWriter {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.update(text.as_bytes());
        Ok(())
    }
}

fn state_hash(state: &TickState) -> String {
    let mut hash = HashWriter(Sha256::new());
    write!(hash, "{}:{}", state.tick, state.day_time).unwrap();
    for slot in state.entities.slots() {
        write!(
            hash,
            "{:?}|{:?}|{:?}|{:?}|{:?}",
            state.entities.body(slot),
            state.entities.row(slot),
            state.entities.pos(slot),
            state.entities.vel(slot),
            state.entities.rot(slot)
        )
        .unwrap();
    }
    let mut players: Vec<_> = state.players.iter().collect();
    players.sort_by_key(|(id, _)| **id);
    for (id, player) in players {
        write!(
            hash,
            "{id:?}|{:?}|{:?}|{:?}",
            player.inventory, player.vitals, player.advancement_progress
        )
        .unwrap();
    }
    let mut chunks = state.world.resident_positions();
    chunks.sort_by_key(|pos| (pos.x, pos.z));
    for pos in chunks {
        let chunk = state.world.chunk(pos).unwrap();
        write!(hash, "{pos:?}").unwrap();
        for section in &chunk.sections {
            write!(
                hash,
                "{}|{:?}|{:?}",
                section.non_air_count(),
                section.palette(),
                section
                    .indices()
                    .map(|indices| (indices.bits_per_entry(), indices.words()))
            )
            .unwrap();
        }
    }
    format!("{:x}", hash.0.finalize())
}

fn percentiles(values: &[u64]) -> serde_json::Value {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let at = |p: usize| sorted[(sorted.len() * p).div_ceil(100).saturating_sub(1)] as f64 / 1_000.0;
    serde_json::json!({"p50_ms": at(50), "p95_ms": at(95), "p99_ms": at(99)})
}

#[test]
#[ignore = "100k-entity release attribution; Main GO and mem-run -x -p required"]
fn merge_cost_profile() {
    let scene_name = std::env::var("MERGE_SCENE").unwrap_or_else(|_| "native100".into());
    let ticks: usize = std::env::var("MERGE_TICKS").map_or(240, |s| s.parse().unwrap());
    let warmup: usize = std::env::var("MERGE_WARMUP").map_or(32, |s| s.parse().unwrap());
    let population: usize =
        std::env::var("MERGE_POPULATION").map_or(100_000, |s| s.parse().unwrap());
    let trace = std::env::var_os("MERGE_TRACE").is_some();
    assert!(ticks > 0);
    let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let (zombies, villagers, cows) = match scene_name.as_str() {
        "native100" => (
            population * 3 / 10,
            population * 3 / 10,
            population * 4 / 10,
        ),
        "villagers" => (0, population, 0),
        _ => panic!("unknown scene {scene_name}"),
    };
    let mut scene = HordeBench::build(100, zombies, villagers, cows, &content).unwrap();
    scene.state.wall_clock = || 1_767_225_600;
    let mut total = Vec::with_capacity(ticks);
    let mut merge = Vec::with_capacity(ticks);
    let mut commit = Vec::with_capacity(ticks);
    let mut journal = Vec::with_capacity(ticks);
    let mut sort = Vec::with_capacity(ticks);
    let mut validation = Vec::with_capacity(ticks);
    let mut effects = Vec::with_capacity(ticks);
    let mut intents = 0;
    let mut accepted = 0;
    let mut wal_hash = HashWriter(Sha256::new());
    let mut journal_sort = Vec::with_capacity(ticks);
    for tick in 0..warmup + ticks {
        PROBE.with(|probe| *probe.borrow_mut() = trace.then(Probe::default));
        let start = Instant::now();
        let (output, _drains, times) = run_tick(&mut scene.state, &[]);
        let tick_us = start.elapsed().as_micros() as u64;
        let start = Instant::now();
        let entries = crate::core::journal_entries_for_tick(&mut scene.state, &output);
        let journal_us = start.elapsed().as_micros() as u64;
        let probe = PROBE
            .with(|probe| probe.borrow_mut().take())
            .unwrap_or_default();
        if tick >= warmup {
            total.push(tick_us + journal_us);
            merge.push(times.merge_us);
            commit.push(times.commit_us);
            journal.push(journal_us);
            sort.push(probe.sort_ns / 1_000);
            validation.push(probe.validation_ns / 1_000);
            effects.push(probe.effects_ns / 1_000);
            intents += probe.intents;
            accepted += probe.accepted;
            journal_sort.push(probe.journal_sort_ns / 1_000);
        }
        // Outside all clocks: ordered WAL and published entity trajectory.
        write!(wal_hash, "{entries:?}|{:?}", output.snapshot.entities).unwrap();
    }
    let state_hash = state_hash(&scene.state);
    let merge_share = merge.iter().sum::<u64>() as f64 / total.iter().sum::<u64>() as f64;
    let receipt = serde_json::json!({
        "scene": scene_name, "population": population, "players": 100,
        "warmup": warmup, "ticks": ticks, "trace": trace,
        "total_including_post_images": percentiles(&total),
        "merge": percentiles(&merge), "commit_phase": percentiles(&commit),
        "journal_post_images": percentiles(&journal), "sort": percentiles(&sort),
        "validation": percentiles(&validation), "commit_merge_effects": percentiles(&effects),
        "journal_sort_dedup": percentiles(&journal_sort),
        "merge_share": merge_share, "intents": intents, "accepted": accepted,
        "state_sha256": state_hash, "trajectory_sha256": format!("{:x}", wal_hash.0.finalize()),
        "dispatch_and_bookkeeping_ns_per_intent": if intents == 0 { 0.0 } else {
            (merge.iter().sum::<u64>().saturating_sub(sort.iter().sum::<u64>() + validation.iter().sum::<u64>())) as f64 * 1_000.0 / intents as f64
        },
        "exclusions": "network, paced owner, WAL writer I/O, output disposal; hashes outside clocks"
    });
    println!("MERGE_COST {receipt}");
    if let Ok(path) = std::env::var("MERGE_RECEIPT") {
        std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
}
