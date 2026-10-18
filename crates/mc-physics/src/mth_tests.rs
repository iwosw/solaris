use super::{atan2, ceil_i32, cos, floor_i32, length2, length3, sin, sqrt};
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

#[test]
fn integer_rounding_matches_floor_and_ceil_casts() {
    for value in [
        f64::NEG_INFINITY,
        f64::MIN,
        f64::from(i32::MIN) - 0.5,
        f64::from(i32::MIN),
        -30_000_000.25,
        -16.001,
        -16.0,
        -0.001,
        -0.0,
        0.0,
        0.001,
        15.999,
        16.0,
        30_000_000.25,
        f64::from(i32::MAX) - 0.5,
        f64::from(i32::MAX) + 0.5,
        f64::MAX,
        f64::INFINITY,
        f64::NAN,
    ] {
        assert_eq!(floor_i32(value), value.floor() as i32, "{value}");
        assert_eq!(ceil_i32(value), value.ceil() as i32, "{value}");
    }
    let mut bits = 0x89ab_cdef_0123_4567_u64;
    for _ in 0..100_000 {
        bits ^= bits << 13;
        bits ^= bits >> 7;
        bits ^= bits << 17;
        let value = f64::from_bits(bits);
        assert_eq!(floor_i32(value), value.floor() as i32, "{bits:x}");
        assert_eq!(ceil_i32(value), value.ceil() as i32, "{bits:x}");
    }
}

#[test]
fn table_trig_preserves_direction_across_turn_boundaries() {
    for step in -4096..=4096 {
        let angle = f64::from(step) * TAU / 1024.0;
        assert!((f64::from(sin(angle)) - angle.sin()).abs() < 0.0001);
        assert!((f64::from(cos(angle)) - angle.cos()).abs() < 0.0001);
    }
    assert_eq!(sin(0.0), 0.0);
    assert_eq!(cos(0.0), 1.0);
}

#[test]
fn atan2_preserves_quadrants_and_heading() {
    for y in [-100.0, -3.0, -0.25, 0.25, 3.0, 100.0] {
        for x in [-100.0, -3.0, -0.25, 0.25, 3.0, 100.0] {
            assert!((atan2(y, x) - y.atan2(x)).abs() < 0.00003);
        }
    }
}

#[test]
fn atan2_handles_overflowing_finite_axes() {
    assert_eq!(atan2(0.0, f64::MAX), 0.0);
    assert_eq!(atan2(0.0, -f64::MAX), PI);
    assert_eq!(atan2(f64::MAX, 0.0), FRAC_PI_2);
    assert_eq!(atan2(-f64::MAX, 0.0), -FRAC_PI_2);
}

#[test]
fn atan2_handles_overflowing_finite_quadrants() {
    for (y, x, expected) in [
        (f64::MAX, f64::MAX, FRAC_PI_4),
        (f64::MAX, -f64::MAX, PI - FRAC_PI_4),
        (-f64::MAX, -f64::MAX, -PI + FRAC_PI_4),
        (-f64::MAX, f64::MAX, -FRAC_PI_4),
    ] {
        assert!((atan2(y, x) - expected).abs() < 0.00003);
    }
    assert!((atan2(1.0e300, 2.0e300) - 1.0_f64.atan2(2.0)).abs() < 0.00003);
}

#[test]
fn atan2_infinite_axes_do_not_index_out_of_the_table() {
    for (y, x) in [
        (f64::INFINITY, f64::INFINITY),
        (f64::NEG_INFINITY, f64::INFINITY),
        (f64::INFINITY, 0.0),
        (0.0, f64::NEG_INFINITY),
    ] {
        assert_eq!(atan2(y, x), y.atan2(x));
    }
}

#[test]
fn lengths_preserve_geometry_and_unrepresentable_overflow() {
    assert_eq!(length2(-3.0, 4.0), 5.0);
    assert_eq!(length3(3.0, -4.0, 12.0), 13.0);
    assert_eq!(length2(f64::MAX, 0.0), f64::INFINITY);
    assert_eq!(length3(0.0, f64::MAX, 0.0), f64::INFINITY);
}

#[test]
fn float_sqrt_preserves_numeric_domain() {
    assert_eq!(sqrt(9.0), 3.0);
    assert_eq!(sqrt(0.0), 0.0);
    assert_eq!(sqrt(f32::INFINITY), f32::INFINITY);
    assert!(sqrt(-1.0).is_nan());
}
