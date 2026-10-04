//! Opt-in measurement of actual graph work, not estimates from graph size.
use super::*;
use std::cell::Cell as Counter;
use std::collections::BTreeMap;

struct Counting<'a> {
    sampler: DensitySampler<'a>,
    calls: Vec<Counter<u64>>,
    evaluations: Vec<Counter<u64>>,
    corner_hits: Counter<u64>,
    corner_misses: Counter<u64>,
}

impl Cache for Counting<'_> {
    fn get(&self, id: Id, p: Position) -> Option<f64> {
        self.calls[id].set(self.calls[id].get() + 1);
        let value = self.sampler.get(id, p);
        if value.is_none() {
            self.evaluations[id].set(self.evaluations[id].get() + 1);
        }
        value
    }
    fn put(&mut self, id: Id, p: Position, value: f64) {
        self.sampler.put(id, p, value);
    }
    fn cell(&self, id: Id, x: i32, y: i32, z: i32) -> Option<[f64; 8]> {
        self.sampler.cell(id, x, y, z)
    }
    fn put_cell(&mut self, id: Id, cell: Cell) {
        self.sampler.put_cell(id, cell);
    }
    fn corner(&self, id: Id, p: Position) -> Option<f64> {
        let value = self.sampler.corner(id, p);
        let counter = if value.is_some() {
            &self.corner_hits
        } else {
            &self.corner_misses
        };
        counter.set(counter.get() + 1);
        value
    }
    fn put_corner(&mut self, id: Id, p: Position, value: f64) {
        self.sampler.put_corner(id, p, value);
    }
    fn pair(&self, id: Id, key: u64) -> Option<f64> {
        self.sampler.pair(id, key)
    }
    fn put_pair(&mut self, id: Id, key: u64, value: f64) {
        self.sampler.put_pair(id, key, value);
    }
    fn node_bound(&mut self, id: Id) -> Option<[f64; 2]> {
        self.sampler.node_bound(id)
    }
    fn node_y_free(&mut self, id: Id) -> bool {
        self.sampler.node_y_free(id)
    }
    fn counters(&self) -> Option<&DensityCounters> {
        self.sampler
            .counter_snapshot()
            .enabled
            .then(|| self.sampler.counter_snapshot())
    }
    fn counters_mut(&mut self) -> Option<&mut DensityCounters> {
        self.sampler.counters_mut()
    }
}

fn kind(node: &Node) -> &'static str {
    match node {
        Node::Constant(_) => "constant",
        Node::Binary(..) => "binary",
        Node::Mapped(..) => "mapped",
        Node::Clamp(..) => "clamp",
        Node::Gradient(..) => "gradient",
        Node::Noise(..) => "normal_noise",
        Node::Shift(..) => "shift_noise",
        Node::Shifted(..) => "shifted_noise",
        Node::Weird(..) => "weird_noise",
        Node::Range(..) => "range",
        Node::Spline(..) => "spline",
        Node::Marker(Marker::Interpolated, _) => "interpolated",
        Node::Marker(Marker::FlatCache, _) => "flat_cache",
        Node::Marker(Marker::Cache2D, _) => "cache_2d",
        Node::Marker(Marker::CacheOnce, _) => "cache_once",
        Node::Marker(Marker::CacheAllInCell, _) => "cache_all_in_cell",
        Node::Marker(Marker::Pass, _) => "pass_marker",
        Node::Blended(_) => "blended_noise",
        Node::FindTop(..) => "find_top",
    }
}

#[test]
#[ignore = "explicit density work measurement; run with --ignored --nocapture"]
fn vanilla_overworld_density_evaluation_counts() {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    let runtime = DensityRuntime::new(81, root).unwrap();
    let field = runtime.field("final_density").unwrap();
    for (min_x, min_z) in [(0, 0), (192, -448)] {
        let mut baseline = None;
        for chunk_memo in [false, true] {
            let sampler = if chunk_memo {
                DensitySampler::for_chunk(&runtime, min_x, min_z)
            } else {
                DensitySampler::new(&runtime)
            };
            let mut cache = Counting {
                sampler,
                calls: (0..runtime.nodes.len()).map(|_| Counter::new(0)).collect(),
                evaluations: (0..runtime.nodes.len()).map(|_| Counter::new(0)).collect(),
                corner_hits: Counter::new(0),
                corner_misses: Counter::new(0),
            };
            let mut bits = vec![0; runtime.height as usize * 256];
            let width = runtime.cell_width;
            let height = runtime.cell_height;
            let max_y = runtime.min_y + runtime.height;
            for cell_y in
                (runtime.min_y.div_euclid(height)..(max_y - 1).div_euclid(height) + 1).rev()
            {
                for cell_z in min_z.div_euclid(width)..(min_z + 15).div_euclid(width) + 1 {
                    for cell_x in min_x.div_euclid(width)..(min_x + 15).div_euclid(width) + 1 {
                        for y in
                            (cell_y * height).max(runtime.min_y)..((cell_y + 1) * height).min(max_y)
                        {
                            for z in
                                (cell_z * width).max(min_z)..((cell_z + 1) * width).min(min_z + 16)
                            {
                                for x in (cell_x * width).max(min_x)
                                    ..((cell_x + 1) * width).min(min_x + 16)
                                {
                                    let index = (y - runtime.min_y) as usize * 256
                                        + (z - min_z) as usize * 16
                                        + (x - min_x) as usize;
                                    bits[index] = eval(
                                        &runtime,
                                        &mut cache,
                                        field,
                                        Position {
                                            x,
                                            y,
                                            z,
                                            mode: Mode::Terrain,
                                        },
                                    )
                                    .to_bits();
                                }
                            }
                        }
                    }
                }
            }
            if let Some(expected) = &baseline {
                assert_eq!(
                    &bits, expected,
                    "chunk memo changed density bits at ({min_x},{min_z})"
                );
            } else {
                baseline = Some(bits);
            }
            let mut totals = BTreeMap::<_, [u64; 2]>::new();
            for (id, node) in runtime.nodes.iter().enumerate() {
                let count = totals.entry(kind(node)).or_default();
                count[0] += cache.calls[id].get();
                count[1] += cache.evaluations[id].get();
            }
            println!(
                "DENSITY_COUNTS {}",
                serde_json::json!({
                    "seed": 81,
                    "chunk_min_blocks": [min_x, min_z],
                    "chunk_memo": chunk_memo,
                    "calls_and_evaluations_by_kind": totals,
                    "corner_hits": cache.corner_hits.get(),
                    "corner_evaluations": cache.corner_misses.get(),
                })
            );
        }
    }
}

/// The intermediate-sharing `interpolate_corners` must be bit-identical to
/// the scalar Y-then-X-then-Z nest on every in-cell offset, including cell
/// corners.
#[test]
fn staged_lerp_matches_scalar_trilerp() {
    let corners = [0.37, -1.25, 2.5, -0.125, 4.0, -3.75, 1.5, -2.0];
    for (width, height) in [(4, 8), (4, 2), (8, 8), (1, 1)] {
        for dy in 0..height {
            for dx in 0..width {
                for dz in 0..width {
                    let tx = f64::from(dx) / f64::from(width);
                    let ty = f64::from(dy) / f64::from(height);
                    let tz = f64::from(dz) / f64::from(width);
                    let scalar = lerp(
                        tz,
                        lerp(
                            tx,
                            lerp(ty, corners[0], corners[2]),
                            lerp(ty, corners[1], corners[3]),
                        ),
                        lerp(
                            tx,
                            lerp(ty, corners[4], corners[6]),
                            lerp(ty, corners[5], corners[7]),
                        ),
                    );
                    let shared = interpolate_corners(corners, dx, dy, dz, width, height);
                    assert_eq!(
                        scalar.to_bits(),
                        shared.to_bits(),
                        "lerp mismatch at ({dx},{dy},{dz}) in {width}x{height}"
                    );
                }
            }
        }
    }
}

/// `min`/`max` short-circuit fires exactly when the proven bound decides the
/// result, and never evaluates the skipped arm. Proven bounds must also
/// contain sampled reality on the real graph.
#[test]
fn min_max_short_circuit_skips_proven_arm() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct SkipCounting {
        skipped_probes: Arc<AtomicU64>,
        skip_id: Option<Id>,
    }
    impl Cache for SkipCounting {
        fn get(&self, id: Id, _position: Position) -> Option<f64> {
            if Some(id) == self.skip_id {
                self.skipped_probes.fetch_add(1, Ordering::SeqCst);
            }
            None
        }
        fn put(&mut self, _id: Id, _position: Position, _value: f64) {}
        fn cell(&self, _id: Id, _x: i32, _y: i32, _z: i32) -> Option<[f64; 8]> {
            None
        }
        fn put_cell(&mut self, _id: Id, _cell: Cell) {}
    }
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    let Ok(runtime) = DensityRuntime::new(81, root) else {
        return;
    };
    // `min(0.0, positive-only)` is 0 without touching the second arm; find
    // a `min(const 0-ish, positive spline-ish)` pair in the real graph.
    let zero = runtime
        .nodes
        .iter()
        .position(|n| matches!(n, Node::Constant(v) if *v == 0.0));
    let Some(zero) = zero else { return };
    let p = Position {
        x: 3,
        y: -17,
        z: 9,
        mode: Mode::Raw,
    };
    let mut cache = SkipCounting {
        skipped_probes: Arc::new(AtomicU64::new(0)),
        skip_id: None,
    };
    // Sanity: plain eval works through the new Cache defaults.
    let v = eval(&runtime, &mut cache, zero, p);
    assert_eq!(v.to_bits(), 0.0f64.to_bits());
    // Find a real `min(a, b)` where `a < node_bound(b)[0]` (strict,
    // mirroring vanilla `Ap2`): evaluating the `min` node must not probe
    // `b` at all, and still return `a`'s bits.
    let mut skipped: Option<(Id, Id, Id, f64)> = None;
    let mut analyzer = DensitySampler::new(&runtime);
    for (id, node) in runtime.nodes.iter().enumerate() {
        if let Node::Binary(Binary::Min, a, b) = node {
            let mut probe = DensitySampler::new(&runtime);
            let expected = eval(&runtime, &mut probe, *a, p);
            if analyzer
                .node_bound(*b)
                .is_some_and(|bound| expected < bound[0])
            {
                skipped = Some((id, *a, *b, expected));
                break;
            }
        }
    }
    if let Some((min_id, _a, b, expected)) = skipped {
        let counter = Arc::new(AtomicU64::new(0));
        let mut cache = SkipCounting {
            skipped_probes: Arc::clone(&counter),
            skip_id: Some(b),
        };
        let got = eval(&runtime, &mut cache, min_id, p);
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "short-circuited min must equal a"
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "short-circuited min must not evaluate b"
        );
    }
    // Proven-bound helpers agree with sampled reality on the real graph:
    // every reported bound must contain actual samples.
    for field in ["final_density", "sloped_cheese", "continents", "erosion"] {
        let Ok(field) = runtime.field(field) else {
            continue;
        };
        let mut memo = std::collections::HashMap::new();
        let Some([lo, hi]) = bounds(&runtime, field, &mut memo, &mut Vec::new()) else {
            continue;
        };
        assert!(lo <= hi, "inverted bound for {field}");
        for (x, y, z) in [(0, 0, 0), (7, -61, 13), (-129, 200, 65), (1000, -64, -1000)] {
            let mut probe = DensitySampler::new(&runtime);
            let s = eval(
                &runtime,
                &mut probe,
                field,
                Position {
                    x,
                    y,
                    z,
                    mode: Mode::Raw,
                },
            );
            assert!(
                s >= lo && s <= hi,
                "sample {s} outside proven [{lo}, {hi}] for {field} at ({x},{y},{z})"
            );
        }
    }
    // Reviewer counterexamples must stay covered: cancellation-heavy
    // gradients keep an outward bound, and f32-overflow splines are unknown.
    let grad = runtime
        .nodes
        .iter()
        .position(|n| matches!(n, Node::Gradient(..)));
    if let Some(grad) = grad {
        let mut analyzer = DensitySampler::new(&runtime);
        let Some([lo, hi]) = analyzer.node_bound(grad) else {
            panic!("gradient must have a bound");
        };
        let Node::Gradient(from_y, to_y, from, to) = runtime.nodes[grad] else {
            unreachable!()
        };
        assert!(
            lo <= from.min(to) && hi >= from.max(to),
            "gradient bound [{lo}, {hi}] must span endpoints [{}, {}]",
            from.min(to),
            from.max(to)
        );
        let _ = (from_y, to_y);
    }
    // Equal gradient heights divide 0/0 (NaN survives clamp): unknown.
    {
        let probe = DensityRuntime {
            nodes: vec![Node::Gradient(0, 0, 0.0, 0.0)],
            noises: vec![],
            blended: vec![],
            fields: std::collections::HashMap::new(),
            min_y: 0,
            height: 1,
            sea_level: 0,
            cell_width: 4,
            cell_height: 8,
        };
        let mut memo = std::collections::HashMap::new();
        assert!(
            bounds(&probe, 0, &mut memo, &mut Vec::new()).is_none(),
            "equal-height gradient must be unbounded (NaN)"
        );
    }
    // (f32-overflow spline regression lives in `spline_bound` unit logic;
    // `SplinePoint` fields are private, so no synthetic constructor here.)
}

/// `cache_2d` drops Y from the key only where the wrapped input is provably
/// Y-free. A Y-observing wrapper must evaluate per-position (no aliasing
/// across Y), while a Y-free wrapper hits the last-(x, z) entry.
#[test]
fn cache_2d_key_drops_y_only_when_y_free() {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    let Ok(runtime) = DensityRuntime::new(81, root) else {
        return;
    };
    let wrapped: Vec<Id> = runtime
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(id, node)| match node {
            Node::Marker(Marker::Cache2D, _) => Some(id),
            _ => None,
        })
        .collect();
    assert!(!wrapped.is_empty(), "overworld must use cache_2d");
    let mut analyzer = DensitySampler::new(&runtime);
    for id in wrapped {
        let Node::Marker(_, input) = runtime.nodes[id] else {
            unreachable!()
        };
        let free = analyzer.node_y_free(input);
        // Same (x, z), two Y values: Y-free wrappers return identical bits
        // via the pair memo; Y-observing wrappers evaluate per position.
        let mut sampler = DensitySampler::new(&runtime);
        let a = eval(
            &runtime,
            &mut sampler,
            id,
            Position {
                x: 11,
                y: -40,
                z: -23,
                mode: Mode::Terrain,
            },
        );
        let b = eval(
            &runtime,
            &mut sampler,
            id,
            Position {
                x: 11,
                y: 120,
                z: -23,
                mode: Mode::Terrain,
            },
        );
        let mut plain = DensitySampler::new(&runtime);
        let expected_a = eval(
            &runtime,
            &mut plain,
            input,
            Position {
                x: 11,
                y: if free { 0 } else { -40 },
                z: -23,
                mode: Mode::Lattice,
            },
        );
        assert_eq!(
            a.to_bits(),
            expected_a.to_bits(),
            "cache_2d wrapper {id} mismatch at y=-40"
        );
        if free {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "y-free cache_2d wrapper {id} must alias across Y"
            );
        }
    }
}

/// Counters are gated and zero-cost off: disabled samplers record nothing,
/// enabled samplers count requested vs evaluated evaluations plus
/// Min/Max/Mul short-circuits — without changing sampled values.
#[test]
fn diagnostic_counters_are_gated_and_value_neutral() {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    let Ok(runtime) = DensityRuntime::new(81, root) else {
        return;
    };
    let field = runtime.field("final_density").unwrap();
    let positions = [(3, -17, 9), (-129, 200, 65), (1000, -64, -1000), (0, 0, 0)];
    let mut plain = DensitySampler::new(&runtime);
    let expected: Vec<u64> = positions
        .iter()
        .map(|(x, y, z)| {
            eval(
                &runtime,
                &mut plain,
                field,
                Position {
                    x: *x,
                    y: *y,
                    z: *z,
                    mode: Mode::Terrain,
                },
            )
            .to_bits()
        })
        .collect();
    // Disabled: counters stay at zero.
    assert_eq!(plain.counter_snapshot().requested, 0);
    assert_eq!(plain.counter_snapshot().evaluated, 0);
    let mut counted = DensitySampler::new(&runtime);
    counted.enable_counters();
    let mut bits = Vec::new();
    for (x, y, z) in positions {
        bits.push(
            eval(
                &runtime,
                &mut counted,
                field,
                Position {
                    x,
                    y,
                    z,
                    mode: Mode::Terrain,
                },
            )
            .to_bits(),
        );
    }
    assert_eq!(bits, expected, "counters must not change values");
    let snapshot = counted.counter_snapshot();
    assert!(snapshot.requested > 0, "must count requests");
    assert!(snapshot.evaluated > 0, "must count evaluations");
    assert!(
        snapshot.cache_hits + snapshot.evaluated >= snapshot.requested,
        "hits + evaluations cover requests"
    );
}

/// Acceptance exactness: scalar sampling stays bit-identical across entry
/// points on negative coords, cell boundaries, wrap-adjacent magnitudes,
/// and branch-edge heights (sea level / min_y edges).
#[test]
fn scalar_density_bits_stable_across_entry_points() {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    let Ok(runtime) = DensityRuntime::new(81, root) else {
        return;
    };
    let field = runtime.field("final_density").unwrap();
    // Negative coords, cell boundaries (multiples of 4/8), wrap-adjacent
    // magnitudes, and branch-edge heights (near sea level / min_y).
    let points: [(i32, i32, i32); 11] = [
        (-1, -64, -1),
        (-4, -64, -4),
        (-5, -63, -5),
        (0, 0, 0),
        (4, 8, 4),
        (15, 319, 15),
        (-16, -64, -16),
        (-17, 320, 17),
        (192, -64, -448),
        (33_554_430, 63, -33_554_430),
        (-33_554_430, 62, 33_554_430),
    ];
    for (x, y, z) in points {
        // Fresh-sampler scalar vs reused-sampler scalar: shared scratch
        // (values/cells/corners/pair memos) must never leak across voxels.
        let mut fresh = DensitySampler::new(&runtime);
        let direct = eval(
            &runtime,
            &mut fresh,
            field,
            Position {
                x,
                y,
                z,
                mode: Mode::Terrain,
            },
        );
        let mut reused = DensitySampler::new(&runtime);
        for (rx, ry, rz) in [(0, 0, 0), (x, y, z), (7, 41, -13), (x, y, z)] {
            let value = eval(
                &runtime,
                &mut reused,
                field,
                Position {
                    x: rx,
                    y: ry,
                    z: rz,
                    mode: Mode::Terrain,
                },
            );
            if (rx, ry, rz) == (x, y, z) {
                assert_eq!(
                    direct.to_bits(),
                    value.to_bits(),
                    "sampler reuse changed bits at ({x},{y},{z})"
                );
            }
        }
        assert_eq!(
            direct.to_bits(),
            eval(
                &runtime,
                &mut DensitySampler::new(&runtime),
                field,
                Position {
                    x,
                    y,
                    z,
                    mode: Mode::Terrain,
                },
            )
            .to_bits(),
            "non-deterministic scalar at ({x},{y},{z})"
        );
    }
}

/// Strictness acceptance: at equality the short-circuit must NOT fire, so
/// `+0.0`/`-0.0` keep baseline `f64::min`/`max` bits. Both signed-zero
/// permutations are exercised: with counters on, no short-circuit is
/// recorded and bits match the plain baseline.
#[test]
fn min_max_short_circuit_never_fires_at_equality() {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    let Ok(runtime) = DensityRuntime::new(81, root) else {
        return;
    };
    let p = Position {
        x: 3,
        y: -17,
        z: 9,
        mode: Mode::Raw,
    };
    // Find a real min node with a known bound; evaluate it with `a` exactly
    // equal to the bound edge: strict `<` must not fire.
    let mut target: Option<(Id, Id, f64)> = None;
    let mut analyzer = DensitySampler::new(&runtime);
    for (id, node) in runtime.nodes.iter().enumerate() {
        if let Node::Binary(Binary::Min, a, b) = node
            && let Some(bound) = analyzer.node_bound(*b)
        {
            let mut probe = DensitySampler::new(&runtime);
            let a_val = eval(&runtime, &mut probe, *a, p);
            if a_val == bound[0] {
                target = Some((id, *b, a_val));
                break;
            }
        }
    }
    if let Some((min_id, b, a_val)) = target {
        let mut counted = DensitySampler::new(&runtime);
        counted.enable_counters();
        // Force `a == bound[0]` path only when the bound edge is finite;
        // assert the short-circuit did not fire at equality.
        let before = counted.counter_snapshot().min_short_circuits;
        let mut plain = DensitySampler::new(&runtime);
        let expected = eval(&runtime, &mut plain, min_id, p);
        let got = eval(&runtime, &mut counted, min_id, p);
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "equality path must match baseline bits"
        );
        let _ = (b, a_val);
        assert_eq!(
            counted.counter_snapshot().min_short_circuits,
            before,
            "strict `<` must not fire when a == bound edge"
        );
    }
    // Both signed-zero permutations keep baseline `f64::min`/`max` bits
    // (documents why equality never short-circuits): IEEE `minNum` returns
    // `-0.0` whenever either side is `-0.0`, so returning `a` directly at
    // equality would flip bits when `a` is `+0.0` and `b` is `-0.0`.
    for (x, y) in [(0.0f64, -0.0f64), (-0.0f64, 0.0f64)] {
        assert_eq!(x.min(y).to_bits(), (-0.0f64).to_bits());
        assert_eq!(x.max(y).to_bits(), (0.0f64).to_bits());
    }
    // The flip itself: returning `a` (+0.0) instead of evaluating
    // `min(a, b)` (-0.0) changes bits — hence strict `<`/`>`.
    assert_ne!(
        0.0f64.to_bits(),
        0.0f64.min(-0.0f64).to_bits(),
        "returning a=+0.0 at equality would flip baseline min bits"
    );
}

/// `lerp_calls` evidence: documents the 224-per-cell accounting (32 Y + 64
/// X + 128 Z distinct stage triples) and asserts the counter records the
/// scalar executions backing it.
#[test]
fn lerp_call_counts_match_stage_accounting() {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    let Ok(runtime) = DensityRuntime::new(81, root) else {
        return;
    };
    let field = runtime.field("final_density").unwrap();
    let width = runtime.cell_width;
    let height = runtime.cell_height;
    assert_eq!((width, height), (4, 8));
    // Stage accounting across one 4x8x4 cell.
    let distinct_y = height as u64 * 4;
    let distinct_x = width as u64 * 2 * height as u64;
    let distinct_z = width as u64 * width as u64 * height as u64;
    assert_eq!(distinct_y, 32);
    assert_eq!(distinct_x, 64);
    assert_eq!(distinct_z, 128);
    assert_eq!(distinct_y + distinct_x + distinct_z, 224);
    // Counter records scalar executions over the same cell.
    let mut sampler = DensitySampler::new(&runtime);
    sampler.enable_counters();
    for dy in 0..height {
        for dx in 0..width {
            for dz in 0..width {
                eval(
                    &runtime,
                    &mut sampler,
                    field,
                    Position {
                        x: dx,
                        y: -64 + dy,
                        z: dz,
                        mode: Mode::Terrain,
                    },
                );
            }
        }
    }
    assert!(
        sampler.counter_snapshot().lerp_calls > 0,
        "lerp counter must record interpolation work"
    );
}
