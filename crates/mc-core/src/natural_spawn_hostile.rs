//! Java-style hostile attempts: one per eligible chunk per tick, including
//! unsuccessful vertical samples. Habitat checks do not retry failed candidates.
use super::*;

pub(super) fn run(
    state: &mut TickState,
    players: &[[f64; 3]],
    rng: &mut Rng,
    changes: &mut ChangeSet,
) {
    let category = MobCategory::Monster;
    let cap = category.max_instances_per_chunk() as usize;
    let global_cap = cap * state.spawn_census.eligible.len() / 289;
    // Stable player/chunk order without a second allocated chunk list. Shared
    // chunks are visited once, even when observers' spawning regions overlap.
    for (observer, player) in players.iter().enumerate() {
        let cell = chunk_of(*player);
        for x in cell.0 - 8..=cell.0 + 8 {
            for z in cell.1 - 8..=cell.1 + 8 {
                let room = global_cap.saturating_sub(state.spawn_census.global[0]);
                if room == 0 {
                    return;
                }
                if !state.spawn_census.eligible.contains(&(x, z))
                    || players[..observer]
                        .iter()
                        .any(|p| near_chunks(chunk_of(*p), (x, z)))
                    || !players.iter().any(|p| {
                        (p[0] - f64::from(x * 16 + 8)).powi(2)
                            + (p[2] - f64::from(z * 16 + 8)).powi(2)
                            < SPAWN_MAX * SPAWN_MAX
                    })
                {
                    continue;
                }
                let column = [x * 16 + rng.below(16) as i32, z * 16 + rng.below(16) as i32];
                spawn_group_in_column(state, players, column, category, room, rng, changes);
            }
        }
    }
}

pub(super) fn height(state: &TickState, x: i32, z: i32, rng: &mut Rng) -> Option<i32> {
    let geometry = state.dimension.geometry();
    // Include solid blocks and unsupported air in the sample space, rather than
    // reservoir-sampling only successful floors (which floods exposed surfaces).
    let max_y = if state.dimension == crate::dimensions::Dimension::Nether {
        128
    } else {
        geometry.max_y()
    };
    let mut top = max_y - 1;
    while state
        .registry
        .materials
        .classify(state.world.block_at(x, top, z)?)
        == BlockMaterial::Air
    {
        top -= 1;
        if top < geometry.min_y() {
            return None;
        }
    }
    let ceiling = (top + 1).min(max_y - 1);
    Some(geometry.min_y() + rng.below((ceiling - geometry.min_y() + 1) as u64) as i32)
}

fn slime_chunk(seed: i64, x: i32, z: i32) -> bool {
    let seed = seed
        .wrapping_add(i64::from(x.wrapping_mul(x).wrapping_mul(0x4c1906)))
        .wrapping_add(i64::from(x.wrapping_mul(0x5ac0db)))
        .wrapping_add(i64::from(z.wrapping_mul(z)).wrapping_mul(0x4307a7))
        .wrapping_add(i64::from(z.wrapping_mul(0x5f24f)))
        ^ 0x3ad8025f;
    crate::inventory::enchanting::LegacyRandom::new(seed).next_int(10) == 0
}

pub(super) fn slime_allowed(
    state: &TickState,
    pos: [i32; 3],
    brightness: u8,
    rng: &mut Rng,
) -> bool {
    let [x, y, z] = pos;
    if y < 40 && slime_chunk(state.world.seed(), x.div_euclid(16), z.div_euclid(16)) {
        return rng.below(10) == 0;
    }
    if !(51..=69).contains(&y)
        || !state.world.biome_at(x, y, z).is_some_and(|biome| {
            matches!(
                biome.as_str(),
                "minecraft:swamp" | "minecraft:mangrove_swamp"
            )
        })
    {
        return false;
    }
    const MOON: [f64; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];
    let moon = MOON[state.day_time.div_euclid(24_000).rem_euclid(8) as usize];
    rng.below(2) == 0 && rng.unit() < moon && u64::from(brightness) <= rng.below(8)
}
