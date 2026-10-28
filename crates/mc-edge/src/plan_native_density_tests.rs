//! Native-density edge fanout bench: real initial density (~3.5k/viewer).
//!
//! Fixture mirrors the live edge-fanout scene: 100k mobs across 104 regions
//! (eight kinds x 13 regions), 64 villagers, 100 self players, 100 viewers on
//! 96-block anchors holding a full 13x13 chunk square (view distance 6).
//! Chunks are empty so no chunk encoding is measured; only `publish` is timed.

use super::*;
use std::time::Instant;

/// Canonical types, resolved once during setup before any timed publish.
/// Fixture order: cow, sheep, pig, chicken, zombie, skeleton, spider, creeper.
static MOB_KINDS: std::sync::LazyLock<[u32; 8]> = std::sync::LazyLock::new(|| {
    [
        "minecraft:cow",
        "minecraft:sheep",
        "minecraft:pig",
        "minecraft:chicken",
        "minecraft:zombie",
        "minecraft:skeleton",
        "minecraft:spider",
        "minecraft:creeper",
    ]
    .map(density_type_id)
});

static VILLAGER_TYPE_ID: std::sync::LazyLock<u32> =
    std::sync::LazyLock::new(|| density_type_id("minecraft:villager"));

fn density_type_id(name: &str) -> u32 {
    mc_data::entity_types::entity_type_contract_26_1_2_by_name(name)
        .expect("canonical density fixture entity")
        .protocol_id
}

#[test]
fn density_fixture_uses_canonical_entity_types() {
    let names = [
        "minecraft:cow",
        "minecraft:sheep",
        "minecraft:pig",
        "minecraft:chicken",
        "minecraft:zombie",
        "minecraft:skeleton",
        "minecraft:spider",
        "minecraft:creeper",
        "minecraft:villager",
    ];
    let actual: Vec<_> = MOB_KINDS
        .iter()
        .copied()
        .chain(std::iter::once(*VILLAGER_TYPE_ID))
        .map(|id| {
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(id)
                .map(|contract| contract.name.to_owned())
        })
        .collect();
    assert_eq!(actual, names.map(|name| Some(name.to_owned())));
}

fn region_center(region: i32) -> (f64, f64) {
    (
        f64::from(32 + 96 * (region % 11)),
        f64::from(32 + 96 * (region / 11)),
    )
}

fn region_count(region: i32) -> i32 {
    if region % 13 == 12 { 212 } else { 1024 }
}

fn density_stats(mut counts: Vec<usize>) -> (usize, f64, f64, usize) {
    counts.sort_unstable();
    let min = counts[0];
    let max = counts[counts.len() - 1];
    let p50 = (counts[49] + counts[50]) as f64 / 2.0;
    let mean = counts.iter().sum::<usize>() as f64 / counts.len() as f64;
    (min, p50, mean, max)
}

/// Actual planner-tracked entities per viewer (excluding self), after warmup.
/// Allocation happens here, outside any timed `publish`.
fn tracked_stats(viewers: &[ViewerState]) -> (usize, f64, f64, usize) {
    let counts: Vec<usize> = viewers
        .iter()
        .map(|viewer| {
            viewer
                .tracked_entities
                .keys()
                .filter(|id| **id != viewer.entity_id)
                .count()
        })
        .collect();
    density_stats(counts)
}

/// Short, in-process fanout benchmark at native density. Run pinned in
/// release, with --ignored --nocapture --test-threads=1; setup and snapshot
/// construction are not timed.
#[test]
#[ignore = "release performance receipt; not a correctness gate"]
fn edge_plan_native_density_bench() {
    let shared_appearance = Arc::new(EntityAppearance::default());
    let player_type = player_entity_type_id();
    let mut entities: Vec<EntityView> = Vec::with_capacity(100_000 + 64 + 100);
    let mut mob_positions: Vec<[f64; 3]> = Vec::with_capacity(100_000 + 64);
    let mut next_mob_id = 10_000;
    for (kind, &type_id) in MOB_KINDS.iter().enumerate() {
        for region_in_kind in 0..13 {
            let region = kind as i32 * 13 + region_in_kind;
            let (center_x, center_z) = region_center(region);
            for n in 0..region_count(region) {
                let position = [
                    center_x - 32.0 + 0.5 + 2.0 * f64::from(n % 32),
                    64.0,
                    center_z - 32.0 + 0.5 + 2.0 * f64::from(n / 32),
                ];
                let mut mob = entity(next_mob_id, position);
                mob.type_id = type_id;
                mob.appearance = Arc::clone(&shared_appearance);
                entities.push(mob);
                mob_positions.push(position);
                next_mob_id += 1;
            }
        }
    }
    assert_eq!(next_mob_id, 110_000, "eight kinds x 12500 mobs");
    for n in 0..64 {
        let position = [
            -95.5 + 8.0 * f64::from(n % 8),
            64.0,
            2.5 + 8.0 * f64::from(n / 8),
        ];
        let mut villager = entity(200_000 + n, position);
        villager.type_id = *VILLAGER_TYPE_ID;
        villager.appearance = Arc::clone(&shared_appearance);
        entities.push(villager);
        mob_positions.push(position);
    }
    for n in 0..100 {
        let (anchor_x, anchor_z) = region_center(n);
        let mut own = entity(n + 1, [anchor_x, 64.0, anchor_z]);
        own.type_id = player_type;
        own.appearance = Arc::clone(&shared_appearance);
        entities.push(own);
    }

    let mut viewers: Vec<_> = (0..100)
        .map(|n: i32| {
            let (anchor_x, anchor_z) = region_center(n);
            let mut viewer = viewer_at(n as u64 + 1, [anchor_x, 64.0, anchor_z]);
            let (chunk_x, chunk_z) = entity_chunk(viewer.position);
            for dx in -6..=6 {
                for dz in -6..=6 {
                    viewer.loaded_chunks.insert((chunk_x + dx, chunk_z + dz), 0);
                }
            }
            viewer.world_credit = 512;
            viewer
        })
        .collect();

    // Geometric admission census (mobs + villagers, excluding own players):
    // fine = inclusive 96-block square, coarse = 13x13 chunk square.
    let mut fine_counts = Vec::with_capacity(100);
    let mut coarse_counts = Vec::with_capacity(100);
    for viewer in &viewers {
        let (chunk_x, chunk_z) = entity_chunk(viewer.position);
        let mut fine = 0;
        let mut coarse = 0;
        for position in &mob_positions {
            if (position[0] - viewer.position[0]).abs() <= 96.0
                && (position[2] - viewer.position[2]).abs() <= 96.0
            {
                fine += 1;
            }
            let (mob_chunk_x, mob_chunk_z) = entity_chunk(*position);
            if (mob_chunk_x - chunk_x).abs() <= 6 && (mob_chunk_z - chunk_z).abs() <= 6 {
                coarse += 1;
            }
        }
        fine_counts.push(fine);
        coarse_counts.push(coarse);
    }
    let (fine_min, fine_p50, fine_mean, fine_max) = density_stats(fine_counts);
    let (coarse_min, coarse_p50, coarse_mean, coarse_max) = density_stats(coarse_counts);
    println!(
        "EDGE_PLAN_NATIVE_DENSITY_TRACK viewers=100 mobs=100000 fine_96block min={fine_min} p50={fine_p50} mean={fine_mean:.2} max={fine_max}",
    );
    println!(
        "EDGE_PLAN_NATIVE_DENSITY_TRACK viewers=100 mobs=100000 coarse_13x13 min={coarse_min} p50={coarse_p50} mean={coarse_mean:.2} max={coarse_max}",
    );
    assert!(
        (fine_p50 - 3540.0).abs() < 1e-9,
        "fine p50 drifted: {fine_p50}"
    );
    assert!(
        (fine_mean - 3512.72).abs() < 1e-6,
        "fine mean drifted: {fine_mean}"
    );
    assert!(
        (coarse_p50 - 4372.0).abs() < 1e-9,
        "coarse p50 drifted: {coarse_p50}"
    );
    assert!(
        (coarse_mean - 4396.92).abs() < 1e-6,
        "coarse mean drifted: {coarse_mean}"
    );

    let mut output = output(vec![], entities, ChangeSet::default());
    // Warm up past initial admission (~3.5k spawns at 512 credit each).
    for _ in 0..24 {
        std::hint::black_box(crate::fanout::publish(&output, viewers.iter_mut(), 6));
    }
    let (tracked_min, tracked_p50, tracked_mean, tracked_max) = tracked_stats(&viewers);
    println!(
        "EDGE_PLAN_NATIVE_DENSITY_TRACK_ACTUAL viewers=100 tracked_per_viewer min={tracked_min} p50={tracked_p50} mean={tracked_mean:.2} max={tracked_max}",
    );
    let mut samples = Vec::with_capacity(40);
    let mut bytes = 0_usize;
    let mut packets_total = 0_usize;
    for tick in 2..=41 {
        output.tick = tick;
        let snapshot = Arc::get_mut(&mut output.snapshot).unwrap();
        snapshot.tick = tick;
        snapshot.day_time += 1;
        let mut rows: Vec<EntityView> = snapshot.entities.iter().cloned().collect();
        for row in &mut rows {
            if row.id.0 >= 10_000 {
                row.position[0] += 0.01;
            }
        }
        snapshot.entities = rows.into();
        let start = Instant::now();
        let packets = crate::fanout::publish(&output, viewers.iter_mut(), 6);
        samples.push(start.elapsed().as_secs_f64() * 1e6);
        bytes += packets
            .iter()
            .map(|packet| packet.data.len())
            .sum::<usize>();
        packets_total += packets.len();
        std::hint::black_box(packets);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "EDGE_PLAN_NATIVE_DENSITY viewers=100 entities=100164 samples={} p50_us={:.3} p99_us={:.3} us_per_viewer={:.3} bytes_per_second_per_viewer={:.1} packets_per_second_per_viewer={:.1}",
        samples.len(),
        samples[20],
        samples[39],
        samples[20] / 100.0,
        bytes as f64 * 20.0 / (40.0 * 100.0),
        packets_total as f64 * 20.0 / (40.0 * 100.0),
    );
}

/// Real-delta native-density bench: same fixture through the public
/// `mc-core` entity-store APIs (`spawn` / `set_pos` / `views`), so snapshots
/// keep `changed` / `base_revision` continuity instead of a full rebuild
/// (`base_revision` 0) every tick. Setup and snapshot construction are not
/// timed; only `publish` is. Viewers persist across all phases.
///
/// Quiet/sparse phases use 20 samples. Moving phases use 100 samples and
/// preserve a full admitted chunk square through crossings. `walk21pc`
/// approximates the baseline report's 0.211 submitted moves/viewer/20-Hz
/// tick at 0.199 blocks/move; `walk100pc` exercises all viewers moving.
/// Submitted moves are not server acknowledgements, so these are explicit
/// workload estimates, not a replay of native client movement.
#[test]
#[ignore = "release performance receipt; not a correctness gate"]
fn edge_plan_native_delta_bench() {
    use mc_core::blueprint::MovementClass;
    use mc_core::entities::{ColdFields, EntityKind, EntityStore};

    let shared_appearance = Arc::new(EntityAppearance::default());
    let player_type = player_entity_type_id();
    let cold = |pos: [f64; 3]| ColdFields {
        pos,
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        ground: true,
        health: 20.0,
        breed_cooldown: 0,
        pickup_delay: 0,
        item: None,
        appearance: Arc::clone(&shared_appearance),
        movement: MovementClass::Walker,
        half_width: 0.3,
        height: 1.8,
        speed: 0.1,
        attack_damage: 1.0,
        sight_range: 16.0,
        attack_reach: 1.0,
    };
    let mut store = EntityStore::default();
    let mut mob_slots = Vec::with_capacity(100_000 + 64);
    let mut next_mob_id = 10_000;
    for (kind, &type_id) in MOB_KINDS.iter().enumerate() {
        for region_in_kind in 0..13 {
            let region = kind as i32 * 13 + region_in_kind;
            let (center_x, center_z) = region_center(region);
            for n in 0..region_count(region) {
                let position = [
                    center_x - 32.0 + 0.5 + 2.0 * f64::from(n % 32),
                    64.0,
                    center_z - 32.0 + 0.5 + 2.0 * f64::from(n / 32),
                ];
                mob_slots.push(store.spawn(
                    EntityId(next_mob_id),
                    uuid::Uuid::from_u128(next_mob_id as u128),
                    EntityKind::Mob { type_id },
                    cold(position),
                ));
                next_mob_id += 1;
            }
        }
    }
    assert_eq!(next_mob_id, 110_000, "eight kinds x 12500 mobs");
    for n in 0..64 {
        let position = [
            -95.5 + 8.0 * f64::from(n % 8),
            64.0,
            2.5 + 8.0 * f64::from(n / 8),
        ];
        let id = 200_000 + n;
        mob_slots.push(store.spawn(
            EntityId(id),
            uuid::Uuid::from_u128(id as u128),
            EntityKind::Mob {
                type_id: *VILLAGER_TYPE_ID,
            },
            cold(position),
        ));
    }
    let mut own_slots = Vec::with_capacity(100);
    for n in 0..100 {
        let (anchor_x, anchor_z) = region_center(n);
        let id = n + 1;
        own_slots.push(store.spawn(
            EntityId(id),
            uuid::Uuid::from_u128(id as u128),
            EntityKind::Mob {
                type_id: player_type,
            },
            cold([anchor_x, 64.0, anchor_z]),
        ));
    }
    let initial = store.views();
    assert_eq!(initial.len(), 100_164);
    assert_eq!(initial.changed().len(), 100_164);

    let mut viewers: Vec<_> = (0..100)
        .map(|n: i32| {
            let (anchor_x, anchor_z) = region_center(n);
            let mut viewer = viewer_at(n as u64 + 1, [anchor_x, 64.0, anchor_z]);
            let (chunk_x, chunk_z) = entity_chunk(viewer.position);
            for dx in -6..=6 {
                for dz in -6..=6 {
                    viewer.loaded_chunks.insert((chunk_x + dx, chunk_z + dz), 0);
                }
            }
            viewer.world_credit = 512;
            viewer
        })
        .collect();

    let mut output = output(vec![], vec![], ChangeSet::default());
    let snapshot = Arc::get_mut(&mut output.snapshot).unwrap();
    snapshot.entities = initial;
    // Warm up past initial admission (~3.5k spawns at 512 credit each).
    for _ in 0..24 {
        std::hint::black_box(crate::fanout::publish(&output, viewers.iter_mut(), 6));
    }
    let revision = output.snapshot.entities.revision();
    let base_revision = output.snapshot.entities.base_revision();
    let (tracked_min, tracked_p50, tracked_mean, tracked_max) = tracked_stats(&viewers);
    println!(
        "EDGE_PLAN_NATIVE_DENSITY_DELTA_TRACK viewers=100 entities=100164 revision={revision} base_revision={base_revision} tracked_per_viewer min={tracked_min} p50={tracked_p50} mean={tracked_mean:.2} max={tracked_max} moving_viewers=0/100 step_blocks_per_tick=0.0",
    );

    // Quiet: no store moves, so no new publication (revision is constant).
    let rev_first = output.snapshot.entities.revision();
    let mut samples = Vec::with_capacity(20);
    let mut bytes = 0_usize;
    let mut packets_total = 0_usize;
    for tick in 2..=21 {
        let views = store.views();
        output.tick = tick;
        let snapshot = Arc::get_mut(&mut output.snapshot).unwrap();
        snapshot.tick = tick;
        snapshot.day_time += 1;
        snapshot.entities = views;
        let start = Instant::now();
        let packets = crate::fanout::publish(&output, viewers.iter_mut(), 6);
        samples.push(start.elapsed().as_secs_f64() * 1e6);
        bytes += packets
            .iter()
            .map(|packet| packet.data.len())
            .sum::<usize>();
        packets_total += packets.len();
        std::hint::black_box(packets);
    }
    let rev_last = output.snapshot.entities.revision();
    report_delta_phase(
        "quiet",
        0,
        0.0,
        (rev_first, rev_last),
        &mut samples,
        bytes,
        packets_total,
    );

    // Sparse: a rotating 1% mob cohort steps +0.01 blocks each tick.
    let rev_first = output.snapshot.entities.revision();
    let mut samples = Vec::with_capacity(20);
    let mut bytes = 0_usize;
    let mut packets_total = 0_usize;
    for tick in 22..=41 {
        let cohort = tick as usize % 100;
        for (index, slot) in mob_slots.iter().enumerate() {
            if index % 100 == cohort {
                let mut pos = store.pos(*slot);
                pos[0] += 0.01;
                store.set_pos(*slot, pos);
            }
        }
        let views = store.views();
        output.tick = tick;
        let snapshot = Arc::get_mut(&mut output.snapshot).unwrap();
        snapshot.tick = tick;
        snapshot.day_time += 1;
        snapshot.entities = views;
        let start = Instant::now();
        let packets = crate::fanout::publish(&output, viewers.iter_mut(), 6);
        samples.push(start.elapsed().as_secs_f64() * 1e6);
        bytes += packets
            .iter()
            .map(|packet| packet.data.len())
            .sum::<usize>();
        packets_total += packets.len();
        std::hint::black_box(packets);
    }
    let rev_last = output.snapshot.entities.revision();
    report_delta_phase(
        "sparse1pc",
        0,
        0.0,
        (rev_first, rev_last),
        &mut samples,
        bytes,
        packets_total,
    );

    // Baseline bots-report: 238775 moves / 100 clients / 564.92445733 s /
    // 20 Hz = 0.21133 moves/viewer/tick; 47589.9048 / 238775 = 0.19931
    // blocks/submitted move. Also cover 100% moving as a denser bound.
    let mut tick = 41_u64;
    for (phase, moving_viewers, all_mobs) in [
        ("walk21pc", 21, false),
        ("walk100pc", 100, false),
        ("allmoving", 0, true),
    ] {
        let viewer_step = if moving_viewers == 0 { 0.0 } else { 0.2 };
        let rev_first = output.snapshot.entities.revision();
        let mut samples = Vec::with_capacity(100);
        let mut bytes = 0_usize;
        let mut packets_total = 0_usize;
        let mut crossings = 0;
        for _ in 0..100 {
            tick += 1;
            let cohort = tick as usize % 100;
            for (index, slot) in mob_slots.iter().enumerate() {
                if all_mobs || index % 100 == cohort {
                    let mut pos = store.pos(*slot);
                    pos[0] += 0.01;
                    store.set_pos(*slot, pos);
                }
            }
            for (i, slot) in own_slots.iter().enumerate() {
                if (i + tick as usize) % 100 >= moving_viewers {
                    continue;
                }
                let mut pos = store.pos(*slot);
                let before = entity_chunk(pos);
                pos[0] += 0.2;
                store.set_pos(*slot, pos);
                let after = entity_chunk(pos);
                if before != after {
                    crossings += 1;
                    // Admission is setup, not chunk encoding: keep the same
                    // full held square used by the stationary fixture.
                    viewers[i].loaded_chunks.clear();
                    for dx in -6..=6 {
                        for dz in -6..=6 {
                            viewers[i]
                                .loaded_chunks
                                .insert((after.0 + dx, after.1 + dz), 0);
                        }
                    }
                }
            }
            let views = store.views();
            assert_eq!(views.base_revision(), output.snapshot.entities.revision());
            output.tick = tick;
            let snapshot = Arc::get_mut(&mut output.snapshot).unwrap();
            snapshot.tick = tick;
            snapshot.day_time += 1;
            snapshot.entities = views;
            let start = Instant::now();
            let packets = crate::fanout::publish(&output, viewers.iter_mut(), 6);
            samples.push(start.elapsed().as_secs_f64() * 1e6);
            bytes += packets
                .iter()
                .map(|packet| packet.data.len())
                .sum::<usize>();
            packets_total += packets.len();
            std::hint::black_box(packets);
        }
        report_delta_phase(
            phase,
            moving_viewers,
            viewer_step,
            (rev_first, output.snapshot.entities.revision()),
            &mut samples,
            bytes,
            packets_total,
        );
        let (min, p50, mean, max) = tracked_stats(&viewers);
        println!(
            "EDGE_PLAN_NATIVE_DENSITY_DELTA_TRACK phase={phase} tracked_per_viewer min={min} p50={p50} mean={mean:.2} max={max} moving_viewers={moving_viewers}/100 chunk_crossings={crossings} step_blocks_per_tick={viewer_step}"
        );
    }
}

fn report_delta_phase(
    phase: &str,
    moving_viewers: usize,
    step: f64,
    revisions: (u64, u64),
    samples: &mut [f64],
    bytes: usize,
    packets_total: usize,
) {
    let (rev_first, rev_last) = revisions;
    samples.sort_by(f64::total_cmp);
    let len = samples.len();
    println!(
        "EDGE_PLAN_NATIVE_DENSITY_DELTA phase={phase} viewers=100 entities=100164 samples={len} rev_first={rev_first} rev_last={rev_last} moving_viewers={moving_viewers}/100 step_blocks_per_tick={step} p50_us={:.3} p99_us={:.3} us_per_viewer={:.3} bytes_per_second_per_viewer={:.1} packets_per_second_per_viewer={:.1}",
        samples[len / 2],
        samples[(len * 99 / 100).min(len - 1)],
        samples[len / 2] / 100.0,
        bytes as f64 / len as f64 * 20.0 / 100.0,
        packets_total as f64 / len as f64 * 20.0 / 100.0,
    );
}
