//! Eight-column arithmetic with the scalar evaluator's exact operation order.
//! Coordinates remain fp64 until each octave's lattice has been selected.
use super::*;

impl NormalNoise {
    pub(crate) fn sample_batch(&self, positions: [[f64; 3]; 8]) -> [f32; 8] {
        let first = self.first.sample_batch(positions);
        let second = self
            .second
            .sample_batch(positions.map(|p| p.map(|v| v * 1.0181268882175227)));
        std::array::from_fn(|lane| (first[lane] + second[lane]) * self.value_factor as f32)
    }
}

impl PerlinNoise {
    fn sample_batch(&self, positions: [[f64; 3]; 8]) -> [f32; 8] {
        let mut input = self.input;
        let mut weight = self.value as f32;
        let mut sum = [0.0; 8];
        for (level, amplitude) in self.levels.iter().zip(&self.amplitudes) {
            if let Some(level) = level {
                let samples = level.sample_batch(positions.map(|p| p.map(|v| wrap(v * input))));
                for lane in 0..8 {
                    sum[lane] += *amplitude as f32 * samples[lane] * weight;
                }
            }
            input *= 2.0;
            weight /= 2.0;
        }
        sum
    }
}

impl ImprovedNoise {
    // Keep permutation lookups separate from floating arithmetic so LLVM can
    // vectorize the eight independent lanes without gathering lattice cells
    // through fp32. No target dispatch, reassociation, or fused multiply-add.
    fn sample_batch(&self, positions: [[f64; 3]; 8]) -> [f32; 8] {
        let mut fractions = [[0.0_f32; 8]; 3];
        let mut hashes = [[0_i32; 8]; 8];
        for (lane, position) in positions.iter().enumerate() {
            let p = [
                position[0] + self.xo,
                position[1] + self.yo,
                position[2] + self.zo,
            ];
            let cell = p.map(floor_i32);
            for axis in 0..3 {
                fractions[axis][lane] = (p[axis] - f64::from(cell[axis])) as f32;
            }
            let x0 = self.p(cell[0]);
            let x1 = self.p(cell[0] + 1);
            let a = self.p(x0 + cell[1]);
            let b = self.p(x0 + cell[1] + 1);
            let c = self.p(x1 + cell[1]);
            let d = self.p(x1 + cell[1] + 1);
            for (corner, hash) in hashes.iter_mut().enumerate() {
                let xy = [a, c, b, d][corner & 3];
                hash[lane] = self.p(xy + cell[2] + (corner >> 2) as i32);
            }
        }
        let [x, y, z] = fractions;
        let fx = x.map(fade);
        let fy = y.map(fade);
        let fz = z.map(fade);
        let dots: [[f32; 8]; 8] = std::array::from_fn(|corner| {
            std::array::from_fn(|lane| {
                grad(
                    hashes[corner][lane],
                    x[lane] - (corner & 1) as f32,
                    (y[lane] - 0.0) - ((corner >> 1) & 1) as f32,
                    z[lane] - (corner >> 2) as f32,
                )
            })
        });
        std::array::from_fn(|lane| {
            let lower = lerp(
                fy[lane],
                lerp(fx[lane], dots[0][lane], dots[1][lane]),
                lerp(fx[lane], dots[2][lane], dots[3][lane]),
            );
            let upper = lerp(
                fy[lane],
                lerp(fx[lane], dots[4][lane], dots[5][lane]),
                lerp(fx[lane], dots[6][lane], dots[7][lane]),
            );
            lerp(fz[lane], lower, upper)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_noise_matches_scalar_bits() {
        for seed in [712816, 5617830, 42] {
            let noise = NoiseFactory::new(seed).normal("minecraft:temperature", -3, &[1.5, 1.0]);
            for origin in [
                -30_000_000.0,
                -16_777_216.0,
                -0.5,
                0.0,
                16_777_216.0,
                29_999_992.0,
            ] {
                for y in [-64.25, -0.0, 0.5, 63.75, 319.0] {
                    let positions =
                        std::array::from_fn(|lane| [origin + lane as f64 * 0.25, y, -origin]);
                    let actual = noise.sample_batch(positions);
                    for (lane, [x, y, z]) in positions.into_iter().enumerate() {
                        assert_eq!(
                            actual[lane].to_bits(),
                            noise.sample(x, y, z).to_bits(),
                            "seed={seed} ({x},{y},{z})"
                        );
                    }
                }
            }
        }
    }
}
