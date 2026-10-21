use super::*;
use crate::settlements::allocator::tests::{block, test_state};

fn town(state: &mut TickState) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "zones".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap()
}
fn zone(min: [i32; 3], max: [i32; 3]) -> ScriptWorldBounds {
    ScriptWorldBounds::new(min, max).unwrap()
}
fn put(state: &mut TickState, pos: [i32; 3], name: &str) {
    let id = block(state, name, &[]);
    state.world.set_block(pos[0], pos[1], pos[2], id);
    state.world.fold_overlay();
}

#[test]
fn lumber_zone_certifies_from_a_grove_with_no_placed_workstation() {
    let mut state = test_state();
    let id = town(&mut state);
    let bounds = zone([2, 201, 2], [8, 206, 8]);
    let before = state.settlements.get(id).unwrap().clone();
    let error = certify_zone(
        &mut state,
        id,
        BuildingKind::LumberCamp,
        bounds,
        [5, 201, 5],
    )
    .unwrap_err();
    assert!(error.missing.iter().any(|r| r.description == "zone logs"));
    assert_eq!(
        state.settlements.get(id).unwrap(),
        &before,
        "refusal changes nothing"
    );
    for y in 201..205 {
        put(&mut state, [3, y, 3], "minecraft:oak_log");
    }
    let building = certify_zone(
        &mut state,
        id,
        BuildingKind::LumberCamp,
        bounds,
        [5, 201, 5],
    )
    .unwrap();
    let record = state.settlements.get(id).unwrap();
    let certified = &record.buildings[&building];
    assert_eq!(
        (certified.kind, certified.level),
        (BuildingKind::LumberCamp, 1)
    );
    assert_eq!(certified.work_points, vec![[5, 201, 5]]);
    validate_record_buildings(record).unwrap();
    let overlapping = zone([6, 201, 6], [10, 205, 10]);
    assert!(
        certify_zone(
            &mut state,
            id,
            BuildingKind::LumberCamp,
            overlapping,
            [9, 201, 9]
        )
        .is_err(),
        "overlapping zone"
    );
}

#[test]
fn mine_zone_needs_stone_depth_and_an_open_entrance() {
    let mut state = test_state();
    let id = town(&mut state);
    let narrow = zone([3, 198, 3], [5, 204, 5]);
    let error = certify_zone(&mut state, id, BuildingKind::Mine, narrow, [4, 201, 4]).unwrap_err();
    assert!(error.missing.iter().any(|r| r.description == "zone stone"));
    assert!(error.missing.iter().any(|r| r.description == "zone width"));
    let bounds = zone([3, 198, 3], [9, 204, 9]);
    for x in 3..=9 {
        for z in 3..=9 {
            for y in 198..200 {
                put(&mut state, [x, y, z], "minecraft:stone");
            }
        }
    }
    assert!(
        certify_zone(&mut state, id, BuildingKind::Mine, bounds, [6, 199, 6]).is_err(),
        "entrance must be open and two blocks above the zone floor"
    );
    let building = certify_zone(&mut state, id, BuildingKind::Mine, bounds, [6, 201, 6]).unwrap();
    let record = state.settlements.get(id).unwrap();
    assert_eq!(record.buildings[&building].work_points, vec![[6, 201, 6]]);
    validate_record_buildings(record).unwrap();
    assert!(certify_zone(&mut state, id, BuildingKind::House, bounds, [6, 201, 6]).is_err());
}

#[test]
fn upgrading_a_natural_zone_with_real_stations_ends_its_natural_status() {
    let mut state = test_state();
    let id = town(&mut state);
    let bounds = zone([3, 198, 3], [9, 204, 9]);
    for x in 3..=9 {
        for z in 3..=9 {
            for y in 198..200 {
                put(&mut state, [x, y, z], "minecraft:stone");
            }
        }
    }
    let building = certify_zone(&mut state, id, BuildingKind::Mine, bounds, [6, 201, 6]).unwrap();
    assert!(state.settlements.get(id).unwrap().buildings[&building].natural_zone);
    assert!(
        upgrade(&mut state, building).is_err(),
        "bare terrain has no stations"
    );
    for x in [4, 5] {
        put(&mut state, [x, 201, 4], "minecraft:crafting_table");
    }
    upgrade(&mut state, building).unwrap();
    let record = state.settlements.get(id).unwrap();
    let upgraded = &record.buildings[&building];
    assert_eq!((upgraded.level, upgraded.natural_zone), (2, false));
    assert_eq!(upgraded.work_points, vec![[4, 201, 4], [5, 201, 4]]);
    validate_record_buildings(record).unwrap();
}
