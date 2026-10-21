//! A built blueprint is functional, not decorative: the intended resident can use it.
use super::tests::build;
use super::*;
use crate::boundary::ChangeSet;
use crate::settlements::SettlementId;
use crate::settlements::allocator::tests::{test_state, villager};
use crate::settlements::buildings::job_slots;
use crate::tick::TickState;

/// An open interior cell at floor level inside the certification box to stand villagers in.
fn standing_cell(state: &TickState, bounds: ScriptWorldBounds) -> [f64; 3] {
    let y = bounds.min[1] + 1;
    for x in bounds.min[0]..=bounds.max[0] {
        for z in bounds.min[2]..=bounds.max[2] {
            let open = |y| {
                state
                    .world
                    .block_at(x, y, z)
                    .is_some_and(|id| state.registry.collision_boxes[id as usize].is_empty())
            };
            if open(y) && open(y + 1) {
                return [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
            }
        }
    }
    panic!("no open cell inside {bounds:?}");
}
fn settle(state: &mut TickState, settlement: SettlementId, bounds: ScriptWorldBounds, n: usize) {
    for _ in 0..n {
        let at = standing_cell(state, bounds);
        let slot = villager(state, at);
        let entity = state.entities.body(slot).id;
        crate::settlements::bind(state, settlement, entity).unwrap();
    }
}
fn run(state: &mut TickState, ticks: u32) {
    for _ in 0..ticks {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(state, &mut changes);
        crate::villager::brain::tick(state, &mut changes);
        state.tick += 1;
    }
}
fn residents(state: &TickState) -> Vec<&crate::villager::VillagerState> {
    state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).villager.as_deref())
        .collect()
}

#[test]
fn every_house_level_gives_each_resident_a_bed() {
    let mut state = test_state();
    state.day_time = 2000;
    let blueprint = get(&state.registry.clone(), "house").unwrap();
    for levels in 1..=blueprint.levels.len() {
        let mut state = test_state();
        state.day_time = 2000;
        let (settlement, id) = build(&mut state, "house", blueprint, levels);
        let bounds = state.settlements.get(settlement).unwrap().buildings[&id].bounds;
        settle(&mut state, settlement, bounds, levels);
        run(&mut state, 300);
        let homes: std::collections::BTreeSet<[i32; 3]> = residents(&state)
            .iter()
            .filter_map(|v| v.home.map(|h| [h.x, h.y, h.z]))
            .collect();
        assert_eq!(
            homes.len(),
            levels,
            "house level {levels}: one distinct bed each"
        );
        assert!(
            homes
                .iter()
                .all(|h| (0..3).all(|i| h[i] >= bounds.min[i] && h[i] <= bounds.max[i]))
        );
    }
}

#[test]
fn staffed_buildings_hand_out_exactly_their_job_slots() {
    let mut failures = Vec::new();
    for (id, profession) in [
        ("farm", crate::villager::VillagerProfession::Farmer),
        ("mason", crate::villager::VillagerProfession::Builder),
        ("mine", crate::villager::VillagerProfession::Miner),
        (
            "lumber_camp",
            crate::villager::VillagerProfession::Lumberjack,
        ),
    ] {
        let registry = test_state().registry.clone();
        let blueprint = get(&registry, id).unwrap();
        for levels in 1..=blueprint.levels.len() {
            let mut state = test_state();
            state.day_time = 2000;
            let (settlement, building) = build(&mut state, id, blueprint, levels);
            let slots = job_slots(blueprint.kind, levels as u8);
            let bounds = state.settlements.get(settlement).unwrap().buildings[&building].bounds;
            settle(&mut state, settlement, bounds, slots + 1);
            run(&mut state, 400);
            let employed = residents(&state)
                .iter()
                .filter(|v| v.job_site.is_some() && v.data.profession == profession)
                .count();
            if employed != slots {
                failures.push(format!(
                    "{id} level {levels}: {employed} of {slots} slots staffed"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn barracks_levels_take_two_soldiers_per_level() {
    let registry = test_state().registry.clone();
    let blueprint = get(&registry, "barracks").unwrap();
    for levels in 1..=blueprint.levels.len() {
        let mut state = test_state();
        let (settlement, building) = build(&mut state, "barracks", blueprint, levels);
        let bounds = state.settlements.get(settlement).unwrap().buildings[&building].bounds;
        settle(&mut state, settlement, bounds, 2 * levels + 1);
        for n in 0..2 * levels {
            crate::settlements::army::recruit(&mut state, settlement)
                .unwrap_or_else(|e| panic!("level {levels} recruit {n}: {e}"));
        }
        assert!(
            crate::settlements::army::recruit(&mut state, settlement).is_err(),
            "level {levels}: beds are the limit"
        );
    }
}

#[test]
fn storage_levels_expose_their_containers_as_settlement_stock() {
    let registry = test_state().registry.clone();
    let blueprint = get(&registry, "storage").unwrap();
    for levels in 1..=blueprint.levels.len() {
        let mut state = test_state();
        let (settlement, _) = build(&mut state, "storage", blueprint, levels);
        let points = crate::settlements::buildings::storage_points(&state, settlement);
        assert!(
            points.len() >= 3 * levels,
            "level {levels}: {} containers",
            points.len()
        );
    }
}
