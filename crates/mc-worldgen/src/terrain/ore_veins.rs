//! Bounded analogue of vanilla OreFeature's overlapping ellipsoids.
//!
//! Configured `size` is the number/extent of ellipsoids, not a promised block
//! count. Small features can legitimately place no blocks. Anchor randomness
//! remains coordinate-derived, rather than vanilla's chunk decoration stream.
//! Mirrors the 26.1.2 OreFeature `spreadXY=size/8`, `spreadY=2` and
//! `r=((sin(pi*step)+1)*nextDouble()*size/16+1)/2` block-center tests.

use crate::vanilla_features::{LegacyRandom, RandomSource};

use super::{MAX_ORE_VEIN_SIZE, ORE_VEIN_RADIUS};

const SIDE: usize = (ORE_VEIN_RADIUS * 2 + 1) as usize;
const VOLUME: usize = SIDE * SIDE * SIDE;

pub(super) fn ore_offsets(
    offsets: &mut [[i8; 3]; MAX_ORE_VEIN_SIZE],
    vein_hash: u64,
    size: usize,
) -> usize {
    let size = size.min(MAX_ORE_VEIN_SIZE);
    if size == 0 {
        return 0;
    }
    let mut random = LegacyRandom::new(vein_hash as i64);
    let angle = f64::from(random.next_float()) * std::f64::consts::PI;
    let dx = angle.sin() * size as f64 / 8.0;
    let dz = angle.cos() * size as f64 / 8.0;
    let start_y = f64::from(random.next_int_bounded(3) - 2);
    let end_y = f64::from(random.next_int_bounded(3) - 2);
    let mut visited = [false; VOLUME];
    let mut count = 0;
    for step in 0..size {
        let t = step as f64 / size as f64;
        let center = [
            dx * (1.0 - 2.0 * t),
            start_y + (end_y - start_y) * t,
            dz * (1.0 - 2.0 * t),
        ];
        let radius = ((std::f64::consts::PI * t).sin() + 1.0) * random.next_double() * size as f64
            / 32.0
            + 0.5;
        // The fixed halo/storage caps also bound oversized custom features.
        // All embedded overworld ore sizes (3..=20) fit the spatial halo.
        let min = center.map(|value| (value - radius).floor() as i32);
        let max = center.map(|value| (value + radius).floor() as i32);
        for x in min[0].max(-ORE_VEIN_RADIUS)..=max[0].min(ORE_VEIN_RADIUS) {
            let nx = (f64::from(x) + 0.5 - center[0]) / radius;
            let xx = nx * nx;
            if xx >= 1.0 {
                continue;
            }
            for y in min[1].max(-ORE_VEIN_RADIUS)..=max[1].min(ORE_VEIN_RADIUS) {
                let ny = (f64::from(y) + 0.5 - center[1]) / radius;
                let xy = xx + ny * ny;
                if xy >= 1.0 {
                    continue;
                }
                for z in min[2].max(-ORE_VEIN_RADIUS)..=max[2].min(ORE_VEIN_RADIUS) {
                    let nz = (f64::from(z) + 0.5 - center[2]) / radius;
                    if xy + nz * nz >= 1.0 {
                        continue;
                    }
                    let index = (x + ORE_VEIN_RADIUS) as usize
                        + SIDE * (y + ORE_VEIN_RADIUS) as usize
                        + SIDE * SIDE * (z + ORE_VEIN_RADIUS) as usize;
                    if visited[index] {
                        continue;
                    }
                    visited[index] = true;
                    offsets[count] = [x as i8, y as i8, z as i8];
                    count += 1;
                    if count == MAX_ORE_VEIN_SIZE {
                        return count;
                    }
                }
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_size_is_not_a_guaranteed_block_count() {
        let mut offsets = [[0; 3]; MAX_ORE_VEIN_SIZE];
        for (size, band) in [
            (3, 0.1..0.8),
            (4, 1.5..2.5),
            (9, 5.0..8.0),
            (17, 20.0..28.0),
            (20, 28.0..45.0),
        ] {
            let total = (0..4096)
                .map(|seed| {
                    ore_offsets(
                        &mut offsets,
                        super::super::feature_hash(seed, 0, 0, 0, 0),
                        size,
                    )
                })
                .sum::<usize>();
            let average = total as f64 / 4096.0;
            println!("ore geometry size={size} blocks/attempt={average:.4}");
            assert!(
                band.contains(&average),
                "size {size}: {average} blocks/attempt"
            );
        }
    }

    #[test]
    fn geometry_is_deterministic_unique_and_bounded() {
        for size in [0, 3, 4, 9, 17, 20, 64, usize::MAX] {
            let mut first = [[0; 3]; MAX_ORE_VEIN_SIZE];
            let mut second = first;
            let count = ore_offsets(&mut first, 0x51, size);
            assert_eq!(count, ore_offsets(&mut second, 0x51, size));
            assert_eq!(first, second);
            for (index, offset) in first[..count].iter().enumerate() {
                assert!(!first[..index].contains(offset));
                assert!(
                    offset
                        .iter()
                        .all(|value| i32::from(*value).abs() <= ORE_VEIN_RADIUS)
                );
            }
        }
    }
}
