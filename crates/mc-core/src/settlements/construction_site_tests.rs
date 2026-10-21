use super::tests::{fixture, fund};
use super::*;

/// Three-by-three chunks around the fixture's own chunk: stone floor at
/// y=200, then solid stone through the whole terrain-following band except
/// the fixture chest, so a site exists only where a test carves one.
fn solid_plateau(state: &mut TickState) {
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter((-1..=1).flat_map(|x| {
            (-1..=1).map(move |z| (crate::boundary::ChunkPos { x, z }, 0))
        })));
    for x in -16i32..32 {
        for z in -16..32 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..=216 {
                if (x, y, z) != (1, 201, 1) {
                    state.world.set_block(x, y, z, state.registry.states.stone);
                }
            }
        }
    }
}

fn clear(state: &mut TickState, origin: [i32; 3], size: [u8; 3]) {
    for x in origin[0]..origin[0] + i32::from(size[0]) {
        for z in origin[2]..origin[2] + i32::from(size[2]) {
            for y in 201..=216 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
}

fn storage_size(state: &TickState) -> [u8; 3] {
    blueprints::get(&state.registry, "storage")
        .unwrap()
        .footprint(Rotation::from_quarter_turns(0))
}

/// Origin whose footprint centre sits at `(dx, dz)` from the bell.
fn centred(anchor: [i32; 3], size: [u8; 3], dx: i32, dz: i32) -> [i32; 3] {
    [
        anchor[0] + dx - i32::from(size[0] / 2),
        200,
        anchor[2] + dz - i32::from(size[2] / 2),
    ]
}

/// Chebyshev distance of the footprint centre from the anchor.
fn distance(anchor: [i32; 3], size: [u8; 3], origin: [i32; 3]) -> i32 {
    (origin[0] + i32::from(size[0] / 2) - anchor[0])
        .abs()
        .max((origin[2] + i32::from(size[2] / 2) - anchor[2]).abs())
}

/// The truth `site_project` must match: the free site with the smallest
/// distance, over every loaded column of the settlement sphere.
fn nearest_free_distance(state: &TickState, id: SettlementId, size: [u8; 3]) -> Option<i32> {
    let record = state.settlements.get(id).unwrap();
    let mut best = None;
    for x in -16i32..32 {
        for z in -16..32 {
            let Some(ground) = surface_y(state, record.anchor[1], x, z) else {
                continue;
            };
            let origin = [x, ground, z];
            if site_free(state, record, origin, size) {
                let found = distance(record.anchor, size, origin);
                best = Some(best.map_or(found, |current: i32| current.min(found)));
            }
        }
    }
    best
}

#[test]
fn ring_order_visits_every_cell_once_nearest_ring_first() {
    let radius = 6;
    let mut seen = std::collections::BTreeSet::new();
    let mut previous = 0;
    for n in 0..(2 * radius + 1u32).pow(2) {
        let (dx, dz) = ring_offset(n);
        let ring = dx.abs().max(dz.abs());
        assert!(ring >= previous, "ring {ring} after {previous} at {n}");
        previous = ring;
        assert!(seen.insert((dx, dz)), "cell ({dx}, {dz}) repeated at {n}");
    }
    assert_eq!(seen.len(), 13 * 13);
}

/// Seed 87122 claim QA: a storage ordered at the anchor was sited 110 blocks
/// away, and 21 earlier orders were refused. Two free pockets exist; the order
/// takes the one nearer the bell, though both lie off the bell's own row.
#[test]
fn owner_order_takes_the_free_site_nearest_the_anchor() {
    let (mut state, id, _) = fixture();
    fund(&mut state, "storage", 1);
    solid_plateau(&mut state);
    let size = storage_size(&state);
    let anchor = state.settlements.get(id).unwrap().anchor;
    let far = centred(anchor, size, 12, -14);
    let near = centred(anchor, size, -6, 7);
    clear(&mut state, far, size);
    clear(&mut state, near, size);
    state.world.fold_overlay();
    request_build(&mut state, id, "storage").unwrap();
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(project.origin, near, "not the pocket at {far:?}");
}

/// A cluttered village: a pillar every third column leaves no footprint-sized
/// gap near the bell, and the only clearing lies off the bell's row. One owner
/// order must find it, and it must be the nearest free site there is.
#[test]
fn owner_order_in_a_cluttered_village_finds_the_nearest_site_at_once() {
    let (mut state, id, _) = fixture();
    fund(&mut state, "storage", 1);
    solid_plateau(&mut state);
    let size = storage_size(&state);
    let anchor = state.settlements.get(id).unwrap().anchor;
    for x in -16i32..32 {
        for z in -16i32..32 {
            if x.rem_euclid(3) != 0 && z.rem_euclid(3) != 0 && (x, z) != (1, 1) {
                for y in 201..=216 {
                    state.world.set_block(x, y, z, state.registry.states.air);
                }
            }
        }
    }
    let clearing = centred(anchor, size, 14, 13);
    clear(&mut state, clearing, size);
    state.world.fold_overlay();
    let nearest = nearest_free_distance(&state, id, size).expect("fixture has a free site");
    request_build(&mut state, id, "storage").unwrap();
    let project = state.settlements.get(id).unwrap().project.as_ref().unwrap();
    assert_eq!(distance(anchor, size, project.origin), nearest);
    assert!(
        distance(anchor, size, project.origin) <= 14,
        "{:?}",
        project.origin
    );
}
