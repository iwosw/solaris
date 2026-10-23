use crate::vanilla_climate::noise::{ImprovedNoise, NoiseFactory};

/// The legacy octave initialization used by 26.1.2 `BlendedNoise`, even with
/// the overworld's modern xoroshiro positional random source.
pub(super) struct BlendedNoise {
    pub(super) min: [ImprovedNoise; 16],
    pub(super) max: [ImprovedNoise; 16],
    pub(super) main: [ImprovedNoise; 8],
    pub(super) xz: f64,
    pub(super) y: f64,
    pub(super) xz_factor: f64,
    pub(super) y_factor: f64,
    pub(super) smear: f64,
}

impl BlendedNoise {
    pub(super) fn new(
        factory: &NoiseFactory,
        xz: f64,
        y: f64,
        xz_factor: f64,
        y_factor: f64,
        smear: f64,
    ) -> Self {
        let mut random = factory.random("minecraft:terrain");
        Self {
            min: std::array::from_fn(|_| ImprovedNoise::new(&mut random)),
            max: std::array::from_fn(|_| ImprovedNoise::new(&mut random)),
            main: std::array::from_fn(|_| ImprovedNoise::new(&mut random)),
            xz: 684.412 * xz,
            y: 684.412 * y,
            xz_factor,
            y_factor,
            smear,
        }
    }

    pub(super) fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
        let x = f64::from(x) * self.xz;
        let y = f64::from(y) * self.y;
        let z = f64::from(z) * self.xz;
        let mx = x / self.xz_factor;
        let my = y / self.y_factor;
        let mz = z / self.xz_factor;
        let smear = self.y * self.smear;
        let main_smear = smear / self.y_factor;
        let mut main = 0.0;
        let mut scale = 1.0;
        for octave in &self.main {
            main += octave.sample_with_y_scale(
                wrap(mx * scale),
                wrap(my * scale),
                wrap(mz * scale),
                main_smear * scale,
                my * scale,
            ) / scale as f32;
            scale /= 2.0;
        }
        let alpha = (main / 10.0 + 1.0) / 2.0;
        let mut min = 0.0;
        let mut max = 0.0;
        scale = 1.0;
        for i in 0..16 {
            let sx = wrap(x * scale);
            let sy = wrap(y * scale);
            let sz = wrap(z * scale);
            if alpha < 1.0 {
                min += self.min[i].sample_with_y_scale(sx, sy, sz, smear * scale, y * scale)
                    / scale as f32;
            }
            if alpha > 0.0 {
                max += self.max[i].sample_with_y_scale(sx, sy, sz, smear * scale, y * scale)
                    / scale as f32;
            }
            scale /= 2.0;
        }
        let alpha = alpha.clamp(0.0, 1.0);
        (min / 512.0 + alpha * (max / 512.0 - min / 512.0)) / 128.0
    }
}

fn wrap(value: f64) -> f64 {
    value - (value / 33_554_432.0 + 0.5).floor() * 33_554_432.0
}
