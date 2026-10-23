//! TargetGrid must return exactly what the linear scans it replaced returned.

use crate::boundary::EntityId;
use crate::target_grid::{Nearest, Target, TargetGrid, distance};

/// splitmix64: seeded, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// Lattice coordinate: coarse steps make equal distances common.
    fn coord(&mut self, half_extent: i64, step: i64) -> f64 {
        let cells = half_extent * 2 / step;
        ((self.below(cells as u64 + 1) as i64) * step - half_extent) as f64
    }
}

fn layout(rng: &mut Rng, first_id: i32, count: usize, half_extent: i64, step: i64) -> Vec<Target> {
    (0..count)
        .map(|index| {
            (
                EntityId(first_id + index as i32),
                [
                    rng.coord(half_extent, step),
                    rng.below(5) as f64,
                    rng.coord(half_extent, step),
                ],
            )
        })
        .collect()
}

/// One chained class: its table and the acceptance filter of the chaser.
type Class<'a> = (&'a [Target], &'a dyn Fn(&Target) -> bool);

/// The replaced chaser scan: one strict-`<` pass over the chained tables,
/// then the sight check on the winner.
fn linear_chain(classes: &[Class], from: [f64; 3], range: f64) -> Nearest {
    let mut nearest: Nearest = None;
    for (table, accept) in classes {
        for target in table.iter().filter(|target| accept(target)) {
            let dist = distance(from, target.1);
            if nearest.is_none_or(|(_, best)| dist < best) {
                nearest = Some((*target, dist));
            }
        }
    }
    nearest.filter(|(_, dist)| *dist <= range)
}

#[test]
fn chaser_chain_matches_linear_scan_on_random_layouts() {
    let mut rng = Rng(0x5EED_0001);
    let ranges = [0.0, 7.5, 16.0, 16.5, 24.0, 35.0, 64.0, 100.0, 1.0e9];
    for (count, half_extent, step) in [
        (0usize, 64i64, 1i64),
        (5, 64, 1),
        (31, 128, 2),
        (40, 128, 4),
        (300, 256, 4),
        (1500, 512, 2),
        (1500, 96, 8),
        (4000, 1024, 1),
    ] {
        let players = layout(&mut rng, 1, count / 5 + 1, half_extent, step);
        let villagers = layout(&mut rng, 10_000, count, half_extent, step);
        let golems = layout(&mut rng, 20_000, count / 20, half_extent, step);
        let zoglins = layout(&mut rng, 30_000, count / 10, half_extent, step);
        let grids = [
            TargetGrid::indexed(&players),
            TargetGrid::indexed(&villagers),
            TargetGrid::indexed(&golems),
            TargetGrid::indexed(&zoglins),
        ];
        for _ in 0..600 {
            let from = [
                rng.coord(half_extent + 32, 1) + 0.5,
                rng.below(5) as f64,
                rng.coord(half_extent + 32, 1) + 0.25,
            ];
            let range = ranges[rng.below(ranges.len() as u64) as usize];
            let only_player =
                (rng.below(4) == 0).then(|| players[rng.below(players.len() as u64) as usize].0);
            let own_id = EntityId(30_000 + rng.below((count / 10 + 1) as u64) as i32);
            let hunts_villagers = rng.below(2) == 0;
            let zoglin = rng.below(3) == 0;
            let player_filter = |(id, _): &Target| only_player.is_none_or(|only| only == *id);
            let always = |_: &Target| true;
            let hunts = |_: &Target| hunts_villagers;
            let not_self = |(id, _): &Target| zoglin && *id != own_id;
            let expected = linear_chain(
                &[
                    (
                        players.as_slice(),
                        &player_filter as &dyn Fn(&Target) -> bool,
                    ),
                    (villagers.as_slice(), &hunts),
                    (golems.as_slice(), &always),
                    (zoglins.as_slice(), &not_self),
                ],
                from,
                range,
            );
            let mut actual: Nearest = None;
            grids[0].consider(&mut actual, from, range, player_filter);
            if hunts_villagers {
                grids[1].consider(&mut actual, from, range, always);
            }
            grids[2].consider(&mut actual, from, range, always);
            if zoglin {
                grids[3].consider(&mut actual, from, range, |(id, _)| *id != own_id);
            }
            assert_eq!(
                actual, expected,
                "count {count} step {step} from {from:?} range {range}"
            );
        }
    }
}

#[test]
fn raider_metric_matches_filtered_min_by() {
    let mut rng = Rng(0x5EED_0002);
    for (count, half_extent, step) in [(12usize, 64i64, 1i64), (800, 192, 4), (3000, 512, 2)] {
        let targets = layout(&mut rng, 1, count, half_extent, step);
        let grid = TargetGrid::indexed(&targets);
        for _ in 0..800 {
            let from = [
                rng.coord(half_extent, 1) + 0.5,
                1.0,
                rng.coord(half_extent, 1) + 0.5,
            ];
            let range = [12.0, 16.0, 64.0][rng.below(3) as usize];
            // Stand-in for line of sight: a fixed pseudo-random subset.
            let visible = |(id, _): &Target| id.0.wrapping_mul(1_640_531_527).rem_euclid(3) != 0;
            let dist_sq = |at: [f64; 3]| (0..3).map(|i| (from[i] - at[i]).powi(2)).sum::<f64>();
            let expected = targets
                .iter()
                .filter(|target| dist_sq(target.1) <= range * range && visible(target))
                .min_by(|a, b| dist_sq(a.1).total_cmp(&dist_sq(b.1)))
                .copied();
            let actual = grid
                .nearest(from, range, |target| {
                    let dist = dist_sq(target.1);
                    (dist <= range * range && visible(target)).then_some(dist)
                })
                .map(|(index, _)| targets[index]);
            assert_eq!(
                actual, expected,
                "count {count} from {from:?} range {range}"
            );
        }
    }
}

#[test]
fn equal_distances_keep_the_lowest_table_index() {
    // Targets on a ring of equal distance around one point, repeated so the
    // table is indexed: the first table entry must win in every cell order.
    let offsets = [
        (5.0, 0.0),
        (-5.0, 0.0),
        (0.0, 5.0),
        (0.0, -5.0),
        (3.0, 4.0),
        (-3.0, 4.0),
        (3.0, -4.0),
        (-3.0, -4.0),
    ];
    let targets: Vec<Target> = (0..64)
        .map(|index| {
            let (dx, dz) = offsets[index % 8];
            (EntityId(index as i32 + 1), [0.5 + dx, 1.0, 0.5 + dz])
        })
        .collect();
    let grid = TargetGrid::indexed(&targets);
    let found = grid.nearest_within([0.5, 1.0, 0.5], 16.0, |_| true);
    assert_eq!(found.map(|(index, _)| index), Some(0));
}

#[test]
fn consumer_gate_only_changes_cost_never_results() {
    let mut rng = Rng(0x5EED_0004);
    let targets = layout(&mut rng, 1, 2000, 256, 2);
    let lazy = TargetGrid::for_consumers(&targets, 0);
    let built = TargetGrid::for_consumers(&targets, crate::target_grid::MIN_CONSUMERS);
    for _ in 0..500 {
        let from = [rng.coord(256, 1) + 0.5, 1.0, rng.coord(256, 1) + 0.5];
        let range = [16.0, 35.0, 64.0][rng.below(3) as usize];
        assert_eq!(
            lazy.nearest_within(from, range, |_| true),
            built.nearest_within(from, range, |_| true),
        );
    }
}

#[test]
fn bounded_existence_matches_linear_at_despawn_boundaries() {
    let mut rng = Rng(0x5EED_0005);
    for count in [0, 1, 31, 32, 100, 1000] {
        let targets = layout(&mut rng, 1, count, 1024, 1);
        let indexed = TargetGrid::indexed(&targets);
        let linear = TargetGrid::linear(&targets);
        for _ in 0..400 {
            let from = [rng.coord(1024, 1), rng.coord(128, 1), rng.coord(1024, 1)];
            for radius in [0.0_f64, 7.0, 32.0, 128.0] {
                let accepts = |(_, pos): &Target| {
                    (pos[0] - from[0]).powi(2)
                        + (pos[1] - from[1]).powi(2)
                        + (pos[2] - from[2]).powi(2)
                        <= radius * radius
                };
                assert_eq!(
                    indexed.any(from, radius, accepts),
                    linear.any(from, radius, accepts),
                    "count={count} from={from:?} radius={radius}"
                );
            }
        }
    }
    let targets: Vec<_> = (0..100)
        .map(|id| (EntityId(id), [-32.0, 0.0, 0.0]))
        .collect();
    let grid = TargetGrid::indexed(&targets);
    for radius in [32.0_f64, 128.0] {
        assert!(grid.any([0.0; 3], radius, |(_, pos)| pos[0].abs() <= radius));
        assert!(!grid.any([0.001, 0.0, 0.0], radius, |(_, pos)| {
            (pos[0] - 0.001).abs() <= 32.0
        }));
    }
}

#[test]
fn bounded_existence_skips_distant_players_and_stops_on_first_hit() {
    let targets: Vec<_> = (0..100)
        .map(|id| (EntityId(id), [f64::from(id) * 160.0 + 0.5, 0.0, 0.5]))
        .collect();
    let grid = TargetGrid::indexed(&targets);
    let mut visited = 0;
    assert!(grid.any([0.5, 0.0, 0.5], 32.0, |_| {
        visited += 1;
        true
    }));
    assert_eq!(
        visited, 1,
        "only the first local observer needs a distance test"
    );
    visited = 0;
    assert!(!grid.any([64.5, 0.0, 0.5], 7.0, |_| {
        visited += 1;
        false
    }));
    assert_eq!(
        visited, 0,
        "empty query cells do not inspect distant observers"
    );
    let linear = TargetGrid::linear(&targets);
    visited = 0;
    assert!(linear.any([0.0; 3], 128.0, |_| {
        visited += 1;
        true
    }));
    assert_eq!(
        visited, 1,
        "small-table/large-radius fallback also short-circuits"
    );
}

#[test]
fn batched_cells_match_unbatched_selection_and_class_ties() {
    let mut rng = Rng(0x5EED_BA7C);
    for count in [0, 20, 31, 32, 300, 1500] {
        let tables: Vec<_> = (0..4)
            .map(|class| layout(&mut rng, class * 10_000 + 1, count, 128, 4))
            .collect();
        let queries: Vec<_> = (0..1000)
            .map(|_| {
                (
                    [
                        rng.coord(160, 1) + 0.01,
                        rng.coord(16, 1),
                        rng.coord(160, 1) + 0.99,
                    ],
                    [0.0, 7.5, 16.0, 16.01, 35.0, 64.0][rng.below(6) as usize],
                )
            })
            .collect();
        let plain: Vec<_> = tables
            .iter()
            .map(|table| TargetGrid::indexed(table))
            .collect();
        let mut batched: Vec<_> = tables
            .iter()
            .map(|table| TargetGrid::indexed(table))
            .collect();
        for grid in &mut batched {
            grid.batch(queries.iter().copied());
        }
        for (from, reach) in queries {
            let mut expected = None;
            let mut actual = None;
            for (class, (plain, batched)) in plain.iter().zip(&batched).enumerate() {
                // Independent per-mob LOS/eligibility predicate, not a cell
                // visibility cache. Quantized coordinates force distance ties.
                let visible = |(id, _): &Target| {
                    (id.0 + from[0].floor() as i32 + class as i32).rem_euclid(3) != 0
                };
                plain.consider(&mut expected, from, reach, visible);
                batched.consider(&mut actual, from, reach, visible);
            }
            assert_eq!(
                actual, expected,
                "count={count}, from={from:?}, reach={reach}"
            );
        }
    }
}

#[test]
fn dense_and_quiet_cells_count_broad_phase_queries() {
    let players: Vec<_> = (0..20)
        .map(|id| {
            (
                EntityId(id + 1),
                [f64::from(id % 5) * 12.0, 1.0, f64::from(id / 5) * 12.0],
            )
        })
        .collect();
    let queries: Vec<_> = (0..1000)
        .map(|id| {
            (
                [
                    f64::from(id % 32) * 2.0 + 0.5,
                    1.0,
                    f64::from(id / 32) * 2.0 + 0.5,
                ],
                35.0,
            )
        })
        .collect();
    let reference = TargetGrid::for_consumers(&players, queries.len());
    let mut batched = TargetGrid::for_consumers(&players, queries.len());
    batched.batch(queries.iter().copied());
    assert_eq!(batched.batch_count(), 16);
    for &(from, reach) in &queries {
        assert_eq!(
            batched.nearest_within(from, reach, |_| true),
            reference.nearest_within(from, reach, |_| true),
        );
    }
    let mut quiet = TargetGrid::for_consumers(&[], queries.len());
    quiet.batch(queries.iter().copied());
    assert_eq!(quiet.batch_count(), 0);
    assert!(
        queries
            .iter()
            .all(|&(from, reach)| quiet.nearest_within(from, reach, |_| true).is_none())
    );
    println!("dense: 1000 exact selections, 16 shared broad phases; quiet: 0 broad phases");
}
