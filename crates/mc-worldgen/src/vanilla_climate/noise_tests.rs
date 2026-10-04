use super::*;

#[test]
fn integer_floor_matches_runtime_boundaries_and_ieee_values() {
    let check = |v: f64| {
        assert_eq!(
            floor_i32(v),
            v.floor() as i32,
            "floor at {:#018x}",
            v.to_bits()
        );
    };
    for v in [
        0.0,
        -0.0,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        -f64::MAX,
        f64::NAN,
        f64::from_bits(0xfff8_0000_0000_1234),
        f64::from_bits(0x7ff0_0000_0000_0001),
    ] {
        check(v);
    }
    for boundary in [
        -2147483649.0_f64,
        -2147483648.0,
        -16777216.0,
        -256.0,
        -1.0,
        1.0,
        256.0,
        16777216.0,
        2147483647.0,
        2147483648.0,
    ] {
        for bits in [
            boundary.to_bits() - 1,
            boundary.to_bits(),
            boundary.to_bits() + 1,
        ] {
            check(f64::from_bits(bits));
        }
    }
    let mut random = Xoroshiro::seeded(0x46_4c_4f_4f_52);
    for _ in 0..8192 {
        check(f64::from_bits(random.next_long()));
    }
}

fn reference_wrap(v: f64) -> f64 {
    v - (v / 33554432.0 + 0.5).floor() * 33554432.0
}

#[test]
fn wrap_preserves_half_period_rounding_and_ieee_bits() {
    let check = |v: f64| {
        assert_eq!(
            wrap(v).to_bits(),
            reference_wrap(v).to_bits(),
            "wrap at {:#018x}",
            v.to_bits()
        );
    };
    for v in [
        0.0,
        -0.0,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        -f64::MAX,
        f64::from_bits(0x7ff8_0000_0000_1234),
        f64::from_bits(0xfff8_0000_0000_1234),
        f64::from_bits(0x7ff0_0000_0000_0001),
    ] {
        check(v);
    }
    for boundary in [
        -50331648.0_f64,
        -33554432.0,
        -16777216.0,
        16777216.0,
        33554432.0,
        50331648.0,
    ] {
        for bits in [
            boundary.to_bits() - 1,
            boundary.to_bits(),
            boundary.to_bits() + 1,
        ] {
            check(f64::from_bits(bits));
        }
    }
    let mut random = Xoroshiro::seeded(0x57_52_41_50);
    for _ in 0..8192 {
        check(f64::from_bits(random.next_long()));
    }
}

fn reference_grad(hash: i32, x: f64, y: f64, z: f64) -> f64 {
    const G: [[i32; 3]; 16] = [
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
    f64::from(g[0]) * x + f64::from(g[1]) * y + f64::from(g[2]) * z
}

#[test]
fn gradients_preserve_signed_zero_and_ieee_operation_order() {
    let values = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for hash in 0..16 {
        for x in values {
            for y in values {
                for z in values {
                    assert_eq!(
                        grad(hash, x, y, z).to_bits(),
                        reference_grad(hash, x, y, z).to_bits(),
                        "gradient {hash} at ({x:?}, {y:?}, {z:?})"
                    );
                }
            }
        }
    }
}

// Independent snapshot of the original floating-point evaluation order.
fn reference_sample(
    noise: &ImprovedNoise,
    x: f64,
    y: f64,
    z: f64,
    y_scale: f64,
    y_max: f64,
) -> f64 {
    let x = x + noise.xo;
    let y = y + noise.yo;
    let z = z + noise.zo;
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let iz = z.floor() as i32;
    let xr = x - f64::from(ix);
    let yr = y - f64::from(iy);
    let zr = z - f64::from(iz);
    let delta = if y_scale != 0.0 {
        let limited = if y_max >= 0.0 && y_max < yr {
            y_max
        } else {
            yr
        };
        (limited / y_scale + 1.0000000116860974e-7).floor() * y_scale
    } else {
        0.0
    };
    let gy = yr - delta;
    let p = |n: i32| i32::from(noise.p[(n & 255) as usize]);
    let x0 = p(ix);
    let x1 = p(ix + 1);
    let a = p(x0 + iy);
    let b = p(x0 + iy + 1);
    let c = p(x1 + iy);
    let d = p(x1 + iy + 1);
    let fade = |v: f64| v * v * v * (v * (v * 6.0 - 15.0) + 10.0);
    let lerp = |t: f64, a: f64, b: f64| a + t * (b - a);
    let fx = fade(xr);
    let fy = fade(yr);
    let fz = fade(zr);
    let lower = lerp(
        fy,
        lerp(
            fx,
            reference_grad(p(a + iz), xr, gy, zr),
            reference_grad(p(c + iz), xr - 1.0, gy, zr),
        ),
        lerp(
            fx,
            reference_grad(p(b + iz), xr, gy - 1.0, zr),
            reference_grad(p(d + iz), xr - 1.0, gy - 1.0, zr),
        ),
    );
    let upper = lerp(
        fy,
        lerp(
            fx,
            reference_grad(p(a + iz + 1), xr, gy, zr - 1.0),
            reference_grad(p(c + iz + 1), xr - 1.0, gy, zr - 1.0),
        ),
        lerp(
            fx,
            reference_grad(p(b + iz + 1), xr, gy - 1.0, zr - 1.0),
            reference_grad(p(d + iz + 1), xr - 1.0, gy - 1.0, zr - 1.0),
        ),
    );
    lerp(fz, lower, upper)
}

#[test]
fn seeded_noise_matches_original_evaluation_bits() {
    for seed in [0, 1, 0x91_23_45_67_89_ab_cd_ef, u64::MAX] {
        let noise = ImprovedNoise::new(&mut Xoroshiro::seeded(seed));
        let mut random = Xoroshiro::seeded(seed ^ 0x4e_4f_49_53_45);
        let check = |x: f64, y: f64, z: f64| {
            for (y_scale, y_max) in [
                (0.0, 0.0),
                (-0.0, 0.0),
                (0.125, -1.0),
                (0.125, 0.0),
                (0.125, 0.25),
                (0.125, 1.0),
            ] {
                assert_eq!(
                    noise
                        .sample_with_y_scale(wrap(x), wrap(y), wrap(z), y_scale, y_max)
                        .to_bits(),
                    reference_sample(
                        &noise,
                        reference_wrap(x),
                        reference_wrap(y),
                        reference_wrap(z),
                        y_scale,
                        y_max
                    )
                    .to_bits(),
                    "seed {seed}, point ({x:?}, {y:?}, {z:?}), y scale/max ({y_scale}, {y_max})"
                );
            }
        };
        for coordinate in [
            -50331648.0_f64,
            -16777216.0,
            -256.0,
            -1.0,
            1.0,
            256.0,
            16777216.0,
            50331648.0,
        ] {
            for bits in [
                coordinate.to_bits() - 1,
                coordinate.to_bits(),
                coordinate.to_bits() + 1,
            ] {
                let v = f64::from_bits(bits);
                check(v, -v, v * 0.5);
            }
        }
        check(0.0, -0.0, 0.0);
        check(-noise.xo, -noise.yo, -noise.zo);
        for _ in 0..512 {
            let x = (random.next_double() - 0.5) * 60000000.0;
            let y = (random.next_double() - 0.5) * 768.0;
            let z = (random.next_double() - 0.5) * 60000000.0;
            check(x, y, z);
        }
    }
}
