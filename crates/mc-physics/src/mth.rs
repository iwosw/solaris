//! Shared table-based entity math derived from Java Edition 26.1.2 `Mth`.
//!
//! Tables are the raw bits dumped from the unmodified 26.1.2 client class;
//! neither startup nor the hot path reconstructs them with host libm.

#[path = "mth_tables.rs"]
mod tables;

pub const DEG_TO_RAD: f32 = std::f32::consts::PI / 180.0;
pub const RAD_TO_DEG: f32 = 57.295776;
const SIN_SCALE: f64 = 10430.378350470453;
const FRAC_BIAS: f64 = f64::from_bits(4_805_340_802_404_319_232);

/// Equivalent to `value.floor() as i32`, including Rust's saturating cast and
/// NaN-to-zero behavior, without a libm call on baseline x86-64.
#[inline]
#[must_use]
pub fn floor_i32(value: f64) -> i32 {
    let integer = value as i32;
    integer.saturating_sub(i32::from(value < f64::from(integer)))
}

/// Equivalent to `value.ceil() as i32` without a libm call.
#[inline]
#[must_use]
pub fn ceil_i32(value: f64) -> i32 {
    let integer = value as i32;
    integer.saturating_add(i32::from(value > f64::from(integer)))
}

#[inline]
#[must_use]
pub fn sin(angle: f64) -> f32 {
    let index = ((angle * SIN_SCALE) as i64 & 0xffff) as usize;
    f32::from_bits(tables::SIN[index])
}

#[inline]
#[must_use]
pub fn cos(angle: f64) -> f32 {
    // Java adds the quarter-turn before its saturating double-to-long cast.
    let index = ((angle * SIN_SCALE + 16384.0) as i64 & 0xffff) as usize;
    f32::from_bits(tables::SIN[index])
}

#[inline]
#[must_use]
pub fn sqrt(value: f32) -> f32 {
    f64::from(value).sqrt() as f32
}

#[inline]
#[must_use]
pub fn length_squared2(x: f64, y: f64) -> f64 {
    x * x + y * y
}

#[inline]
#[must_use]
pub fn length2(x: f64, y: f64) -> f64 {
    length_squared2(x, y).sqrt()
}

#[inline]
#[must_use]
pub fn length_squared3(x: f64, y: f64, z: f64) -> f64 {
    x * x + y * y + z * z
}

#[inline]
#[must_use]
pub fn length3(x: f64, y: f64, z: f64) -> f64 {
    length_squared3(x, y, z).sqrt()
}

#[inline]
#[must_use]
pub fn atan2(mut y: f64, mut x: f64) -> f64 {
    let mut squared = x * x + y * y;
    if !squared.is_finite() {
        if squared.is_nan() {
            return f64::NAN;
        }
        if !(x.is_finite() && y.is_finite()) {
            // An infinite axis would index the 257-entry table out of bounds
            // (vanilla throws). Keep host semantics for this degenerate case.
            return y.atan2(x);
        }
        // Deliberate fix: vanilla's inverse-root correction turns an
        // overflowed finite norm into -infinity, then 0 * -infinity into
        // NaN. Rescale only this overflow case; the ordinary path and
        // non-finite argument semantics remain byte-for-byte arithmetic.
        let scale = x.abs().max(y.abs());
        x /= scale;
        y /= scale;
        squared = x * x + y * y;
    }
    let negative_y = y < 0.0;
    if negative_y {
        y = -y;
    }
    let negative_x = x < 0.0;
    if negative_x {
        x = -x;
    }
    let steep = y > x;
    if steep {
        std::mem::swap(&mut x, &mut y);
    }
    let inverse = fast_inverse_sqrt(squared);
    x *= inverse;
    y *= inverse;
    let biased = FRAC_BIAS + y;
    // Java narrows the raw long bits to a signed int, not a numeric float cast.
    let index = (biased.to_bits() as i32) as usize;
    let phi = f64::from_bits(tables::ASIN_TAB[index]);
    let cosine = f64::from_bits(tables::COS_TAB[index]);
    let sine = biased - FRAC_BIAS;
    let delta = y * cosine - x * sine;
    let correction = (6.0 + delta * delta) * delta * 0.16666666666666666;
    let mut angle = phi + correction;
    if steep {
        angle = std::f64::consts::FRAC_PI_2 - angle;
    }
    if negative_x {
        angle = std::f64::consts::PI - angle;
    }
    if negative_y {
        angle = -angle;
    }
    angle
}

#[inline]
fn fast_inverse_sqrt(mut value: f64) -> f64 {
    let half = 0.5 * value;
    let bits = 6_910_469_410_427_058_090_i64.wrapping_sub((value.to_bits() as i64) >> 1);
    value = f64::from_bits(bits as u64);
    value * (1.5 - half * value * value)
}

#[cfg(test)]
#[path = "mth_tests.rs"]
mod tests;
