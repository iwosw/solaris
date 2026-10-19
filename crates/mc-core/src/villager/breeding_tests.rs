//! A typical player village, driven only through the allocator + brain tick:
//! nobody is hand-placed into courtship and nobody is handed food. Food comes
//! from what the farmers harvest, exactly as in vanilla.
use super::*;

fn population(state: &TickState) -> usize {
    state
        .entities
        .slots()
        .filter(|&slot| state.entities.row(slot).villager.is_some())
        .count()
}

/// Four bound villagers (two farmers, two unemployed), six beds, a certified
/// level-2 farmer workshop with two composters beside mature wheat/carrots.
fn typical_village(state: &mut TickState) {
    state.day_time = 3000;
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "village".into(),
            "owner".into(),
            [8, 201, 8],
            crate::settlements::commands::DEFAULT_RADIUS,
        )
        .unwrap();
    for (x, z) in [(3, 2), (7, 2), (3, 13), (7, 13), (11, 13), (11, 2)] {
        bed(state, x, z);
    }
    let composter = block(state, "minecraft:composter", &[]);
    state.world.set_block(12, 201, 8, composter);
    state.world.set_block(12, 201, 9, composter);
    let farmland = block(state, "minecraft:farmland", &[]);
    let wheat = block(state, "minecraft:wheat", &[("age", "7")]);
    let carrots = block(state, "minecraft:carrots", &[("age", "7")]);
    for x in 13..16 {
        for z in 5..12 {
            state.world.set_block(x, 200, z, farmland);
            state
                .world
                .set_block(x, 201, z, if x == 13 { carrots } else { wheat });
        }
    }
    state.world.fold_overlay();
    let workshop = crate::settlements::buildings::certify(
        state,
        settlement,
        crate::settlements::buildings::BuildingKind::Workshop(Profession::Farmer),
        mc_script::ScriptWorldBounds::new([10, 200, 7], [13, 203, 10]).unwrap(),
    )
    .expect("the farm has a loaded, floored workshop with composters");
    crate::settlements::buildings::upgrade(state, workshop)
        .expect("level 2 supplies a distinct slot for each farmer");
    assert_eq!(
        state.settlements.get(settlement).unwrap().buildings[&workshop]
            .work_points
            .len(),
        2,
    );
    for (index, offset) in [(0, 0.0), (1, 1.0), (2, 2.0), (3, 3.0)] {
        let slot = villager(state, [8.5 + offset, 201.0, 8.5]);
        let id = state.entities.body(slot).id;
        crate::settlements::bind(state, settlement, id).unwrap();
        if index < 2 {
            state
                .entities
                .row_mut(slot)
                .villager
                .as_mut()
                .unwrap()
                .data
                .profession = Profession::Farmer;
        }
    }
}

#[test]
fn bound_village_farmers_feed_the_village_and_it_has_births() {
    let mut state = test_state();
    typical_village(&mut state);
    let before = population(&state);
    let mut reduced_cadence = false;
    for _ in 0..6_000 {
        step(&mut state);
        reduced_cadence |= state
            .villager_pois
            .resident_data
            .iter()
            .any(|brain| brain.cadence.period > 1);
        if population(&state) > before {
            assert!(
                reduced_cadence,
                "no player is near: the village runs on a reduced brain cadence"
            );
            return;
        }
    }
    let residents: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| {
            let v = state.entities.row(slot).villager.as_ref()?;
            Some((
                state.entities.body(slot).id,
                state.entities.pos(slot),
                v.data.profession,
                v.job_site,
                v.home,
                v.activity,
                &v.inventory,
                (v.food_level, v.age, v.partner, v.courtship_until),
            ))
        })
        .collect();
    panic!(
        "no birth in 6000 working-hour ticks of a four-villager village with six beds: {residents:?}"
    );
}

#[test]
fn working_farmer_harvests_replants_and_bakes_bread() {
    let mut state = test_state();
    state.day_time = 3000;
    let farmland = block(&state, "minecraft:farmland", &[]);
    let wheat = block(&state, "minecraft:wheat", &[("age", "7")]);
    let young = block(&state, "minecraft:wheat", &[("age", "0")]);
    for x in 9..12 {
        state.world.set_block(x, 200, 8, farmland);
        state.world.set_block(x, 201, 8, wheat);
    }
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.data.profession = Profession::Farmer;
    v.activity = Activity::Work;
    let mut changes = ChangeSet::default();
    for tick in [7, 47, 87] {
        state.tick = tick;
        let mut v = state.entities.row_mut(slot).villager.take().unwrap();
        crate::villager::harvest::work(&mut state, slot, &mut v, &mut changes);
        state.entities.row_mut(slot).villager = Some(v);
    }
    let bread = item(&state, "minecraft:bread");
    assert_eq!(food_count(&state, slot, bread), 1);
    assert_eq!(food_count(&state, slot, item(&state, "minecraft:wheat")), 0);
    assert_eq!(changes.blocks.len(), 3);
    for x in 9..12 {
        assert_eq!(state.world.block_at(x, 201, 8), Some(young));
    }
}

#[test]
fn farmer_with_partial_room_leaves_the_crop_standing() {
    let mut state = test_state();
    state.day_time = 3000;
    let farmland = block(&state, "minecraft:farmland", &[]);
    let carrots = block(&state, "minecraft:carrots", &[("age", "7")]);
    state.world.set_block(9, 200, 8, farmland);
    state.world.set_block(9, 201, 8, carrots);
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let carrot = item(&state, "minecraft:carrot");
    let potato = item(&state, "minecraft:potato");
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.data.profession = Profession::Farmer;
    v.activity = Activity::Work;
    v.inventory = std::array::from_fn(|_| Some(ItemStack::new(potato, 64)));
    v.inventory[0] = Some(ItemStack::new(carrot, 62));
    let mut changes = ChangeSet::default();
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    crate::villager::harvest::work(&mut state, slot, &mut v, &mut changes);
    state.entities.row_mut(slot).villager = Some(v);
    assert_eq!(food_count(&state, slot, carrot), 62);
    assert_eq!(state.world.block_at(9, 201, 8), Some(carrots));
    assert!(changes.blocks.is_empty());
}

#[test]
fn bound_farmer_waits_for_a_certified_job_before_harvesting() {
    let mut state = test_state();
    state.day_time = 3000;
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "farm".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    let farmland = block(&state, "minecraft:farmland", &[]);
    let carrots = block(&state, "minecraft:carrots", &[("age", "7")]);
    state.world.set_block(9, 200, 8, farmland);
    state.world.set_block(9, 201, 8, carrots);
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(&mut state, settlement, entity).unwrap();
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    v.data.profession = Profession::Farmer;
    v.activity = Activity::Work;
    let mut changes = ChangeSet::default();
    crate::villager::harvest::work(&mut state, slot, &mut v, &mut changes);
    state.entities.row_mut(slot).villager = Some(v);
    assert_eq!(
        state.world.block_at(9, 201, 8),
        Some(carrots),
        "saved profession alone is not a settlement production slot"
    );
    assert!(changes.blocks.is_empty());
    assert_eq!(
        food_count(&state, slot, item(&state, "minecraft:carrot")),
        0
    );
}
