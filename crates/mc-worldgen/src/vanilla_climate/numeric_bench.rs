//! Candidate arithmetic experiment. Coordinates remain f64 through lattice splitting.
use super::noise::{ImprovedNoise, NoiseFactory, NormalNoise};
use std::hint::black_box;
use std::ops::{Add, Mul, Sub};
use std::time::Instant;

trait Real: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> {
    fn from_f64(x: f64) -> Self;
    fn to_f64(self) -> f64;
    fn scale(self, coefficient: i8) -> Self;
}
impl Real for f64 {
    fn from_f64(x: f64) -> Self {
        x
    }
    fn to_f64(self) -> f64 {
        self
    }
    fn scale(self, coefficient: i8) -> Self {
        self * f64::from(coefficient)
    }
}
impl Real for f32 {
    fn from_f64(x: f64) -> Self {
        x as Self
    }
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    fn scale(self, coefficient: i8) -> Self {
        self * f32::from(coefficient)
    }
}
// Q11.20: enough headroom for the normal-noise octave accumulation.
#[derive(Clone, Copy)]
struct Fixed(i32);
impl Real for Fixed {
    fn from_f64(x: f64) -> Self {
        Self((x * 1048576.0).round() as i32)
    }
    fn to_f64(self) -> f64 {
        f64::from(self.0) / 1048576.0
    }
    fn scale(self, coefficient: i8) -> Self {
        Self(self.0 * i32::from(coefficient))
    }
}
impl Add for Fixed {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}
impl Sub for Fixed {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}
impl Mul for Fixed {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Self(((i64::from(self.0) * i64::from(rhs.0)) >> 20) as i32)
    }
}
fn fade<R: Real>(v: R) -> R {
    v * v * v * (v * (v * R::from_f64(6.0) - R::from_f64(15.0)) + R::from_f64(10.0))
}
fn lerp<R: Real>(t: R, a: R, b: R) -> R {
    a + t * (b - a)
}
fn grad<R: Real>(hash: i32, x: R, y: R, z: R) -> R {
    const G: [[i8; 3]; 16] = [
        [1, 1, 0],
        [-1, 1, 0],
        [1, -1, 0],
        [-1, -1, 0],
        [1, 0, 1],
        [-1, 0, 1],
        [1, 0, -1],
        [-1, 0, -1],
        [0, 1, 1],
        [0, -1, 1],
        [0, 1, -1],
        [0, -1, -1],
        [1, 1, 0],
        [0, -1, 1],
        [-1, 1, 0],
        [0, -1, -1],
    ];
    let g = G[(hash & 15) as usize];
    x.scale(g[0]) + y.scale(g[1]) + z.scale(g[2])
}
// N=1 is scalar; N=8 exposes neighbouring columns to LLVM's loop/SLP vectorizer.
#[inline(never)]
fn improved<R: Real, const N: usize>(noise: &ImprovedNoise, xyz: [[f64; 3]; N]) -> [R; N] {
    let mut hashes = [[0; 8]; N];
    let mut fractions = [[R::from_f64(0.0); 3]; N];
    for lane in 0..N {
        let v = [
            xyz[lane][0] + noise.xo,
            xyz[lane][1] + noise.yo,
            xyz[lane][2] + noise.zo,
        ];
        let cell = v.map(|x| x.floor() as i32);
        fractions[lane] = std::array::from_fn(|axis| R::from_f64(v[axis] - f64::from(cell[axis])));
        let p = |i: i32| i32::from(noise.p[(i & 255) as usize]);
        for (corner, hash) in hashes[lane].iter_mut().enumerate() {
            *hash = p(
                p(p(cell[0] + (corner & 1) as i32) + cell[1] + ((corner >> 1) & 1) as i32)
                    + cell[2]
                    + (corner >> 2) as i32,
            );
        }
    }
    let mut out = [R::from_f64(0.0); N];
    for lane in 0..N {
        let [x, y, z] = fractions[lane];
        let one = R::from_f64(1.0);
        let dots: [R; 8] = std::array::from_fn(|corner| {
            grad(
                hashes[lane][corner],
                x - if corner & 1 != 0 {
                    one
                } else {
                    R::from_f64(0.0)
                },
                y - if corner & 2 != 0 {
                    one
                } else {
                    R::from_f64(0.0)
                },
                z - if corner & 4 != 0 {
                    one
                } else {
                    R::from_f64(0.0)
                },
            )
        });
        let (fx, fy, fz) = (fade(x), fade(y), fade(z));
        out[lane] = lerp(
            fz,
            lerp(fy, lerp(fx, dots[0], dots[1]), lerp(fx, dots[2], dots[3])),
            lerp(fy, lerp(fx, dots[4], dots[5]), lerp(fx, dots[6], dots[7])),
        );
    }
    out
}
fn normal<R: Real, const N: usize>(noise: &NormalNoise, xyz: [[f64; 3]; N]) -> [R; N] {
    let mut sums = [[R::from_f64(0.0); N]; 2];
    for (set, perlin) in [&noise.first, &noise.second].into_iter().enumerate() {
        let factor = if set == 0 { 1.0 } else { 1.0181268882175227 };
        let mut input = perlin.input;
        let mut weight = R::from_f64(perlin.value);
        for (level, amplitude) in perlin.levels.iter().zip(&perlin.amplitudes) {
            if let Some(level) = level {
                let points = xyz.map(|v| v.map(|v| super::noise::wrap((v * factor) * input)));
                let values = improved::<R, N>(level, points);
                for lane in 0..N {
                    sums[set][lane] =
                        sums[set][lane] + R::from_f64(*amplitude) * values[lane] * weight;
                }
            }
            input *= 2.0;
            weight = weight * R::from_f64(0.5);
        }
    }
    std::array::from_fn(|lane| (sums[0][lane] + sums[1][lane]) * R::from_f64(noise.value_factor))
}
fn bench<R: Real, const N: usize>(noise: &NormalNoise, batches: usize) -> (f64, f64) {
    let start = Instant::now();
    let mut checksum = 0.0;
    for i in 0..batches {
        let points = std::array::from_fn(|lane| {
            let column = i * N + lane;
            [
                (column % 1024) as f64 * 0.25,
                64.0,
                (column / 1024) as f64 * 0.25,
            ]
        });
        for value in black_box(normal::<R, N>(noise, black_box(points))) {
            checksum += value.to_f64();
        }
    }
    (
        start.elapsed().as_secs_f64() * 1e9 / (batches * N) as f64,
        checksum,
    )
}
#[test]
#[ignore = "release numeric experiment; run in the serialized perf lane"]
fn numeric_candidates() {
    let noise = NoiseFactory::new(712816).normal(
        "minecraft:temperature",
        -10,
        &[1.5, 0.0, 1.0, 0.0, 0.0, 0.0],
    );
    for run in 0..3 {
        let start = Instant::now();
        let mut sum = 0.0;
        for i in 0..131072 {
            sum += black_box(noise.sample(
                black_box((i % 1024) as f64 * 0.25),
                64.0,
                (i / 1024) as f64 * 0.25,
            ));
        }
        println!(
            "NUMERIC run={run} type=production lanes=1 ns={} checksum={sum}",
            start.elapsed().as_secs_f64() * 1e9 / 131072.0
        );
        for (name, lanes, result) in [
            ("f64", 1, bench::<f64, 1>(&noise, 131072)),
            ("f32", 1, bench::<f32, 1>(&noise, 131072)),
            ("fixed", 1, bench::<Fixed, 1>(&noise, 131072)),
            ("f64", 8, bench::<f64, 8>(&noise, 16384)),
            ("f32", 8, bench::<f32, 8>(&noise, 16384)),
            ("fixed", 8, bench::<Fixed, 8>(&noise, 16384)),
        ] {
            println!(
                "NUMERIC run={run} type={name} lanes={lanes} ns={} checksum={}",
                result.0, result.1
            );
        }
    }
}

#[test]
fn normal_noise_preserves_neighbouring_world_border_columns() {
    for seed in [712816, 5617830, 42] {
        let noise = NoiseFactory::new(seed).normal("minecraft:temperature", -3, &[1.5, 1.0]);
        for origin in [-30_000_000, -16_777_216, 16_777_216, 29_999_992] {
            let points = std::array::from_fn(|lane| {
                [f64::from(origin + lane as i32), 64.0, f64::from(-origin)]
            });
            let reference = normal::<f64, 8>(&noise, points);
            let mut bits = [0; 8];
            for (lane, [x, y, z]) in points.into_iter().enumerate() {
                let actual = noise.sample(x, y, z);
                bits[lane] = actual.to_bits();
                assert!(
                    (f64::from(actual) - reference[lane]).abs() < 1.0e-5,
                    "seed={seed} x={x}: {actual} vs {}",
                    reference[lane]
                );
            }
            assert!(
                bits.windows(2).all(|pair| pair[0] != pair[1]),
                "adjacent integer columns collapsed at {origin}"
            );
        }
    }
}
