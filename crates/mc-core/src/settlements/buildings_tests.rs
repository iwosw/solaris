use super::*;
use crate::boundary::{BlockPos, ChangeSet};
use crate::settlements::allocator::tests::{block, test_state, villager};

fn town(state: &mut TickState) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "builders".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap()
}
fn bounds() -> ScriptWorldBounds {
    ScriptWorldBounds::new([3, 200, 3], [7, 204, 7]).unwrap()
}
fn station(state: &mut TickState, at: [i32; 3], name: &str) {
    let id = block(state, name, &[]);
    state.world.set_block(at[0], at[1], at[2], id);
    state.world.fold_overlay();
}
#[test]
fn certification_reports_missing_and_never_changes_record_on_failure() {
    let mut state = test_state();
    let id = town(&mut state);
    let before = state.settlements.get(id).unwrap().clone();
    let error = certify(&mut state, id, BuildingKind::House, bounds()).unwrap_err();
    assert!(error.missing.iter().any(|r| r.description == "beds"));
    assert!(error.missing.iter().any(|r| r.description == "roof"));
    assert_eq!(state.settlements.get(id).unwrap(), &before);
}
#[test]
fn workshop_certifies_upgrades_and_caps_distinct_slots() {
    let mut state = test_state();
    let id = town(&mut state);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    let building = certify(
        &mut state,
        id,
        BuildingKind::Workshop(Profession::Farmer),
        bounds(),
    )
    .unwrap();
    assert_eq!(
        state.settlements.get(id).unwrap().buildings[&building].work_points,
        vec![[4, 201, 4]]
    );
    assert!(upgrade(&mut state, building).is_err());
    station(&mut state, [6, 201, 4], "minecraft:composter");
    upgrade(&mut state, building).unwrap();
    assert_eq!(
        state.settlements.get(id).unwrap().buildings[&building].level,
        2
    );
    station(&mut state, [4, 201, 6], "minecraft:composter");
    upgrade(&mut state, building).unwrap();
    assert_eq!(
        state.settlements.get(id).unwrap().buildings[&building].level,
        3
    );
    assert!(upgrade(&mut state, building).is_err());
    assert_eq!(job_slots(BuildingKind::Workshop(Profession::Farmer), 3), 3);
}
#[test]
fn idle_bound_residents_auto_assign_only_certified_slots_unbound_stays_vanilla() {
    let mut state = test_state();
    state.day_time = 2000;
    let id = town(&mut state);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    station(&mut state, [12, 201, 12], "minecraft:lectern");
    let first = villager(&mut state, [3.5, 201.0, 4.5]);
    let second = villager(&mut state, [3.5, 201.0, 6.5]);
    for slot in [first, second] {
        let entity = state.entities.body(slot).id;
        crate::settlements::bind(&mut state, id, entity).unwrap();
    }
    let outsider = villager(&mut state, [12.5, 201.0, 11.5]);
    certify(
        &mut state,
        id,
        BuildingKind::Workshop(Profession::Farmer),
        bounds(),
    )
    .unwrap();
    for _ in 0..300 {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(&mut state, &mut changes);
        crate::villager::brain::tick(&mut state, &mut changes);
        state.tick += 1;
    }
    let residents = [first, second].map(|slot| state.entities.row(slot).villager.as_ref().unwrap());
    assert_eq!(
        residents
            .iter()
            .filter(|v| v.job_site == Some(BlockPos { x: 4, y: 201, z: 4 }))
            .count(),
        1
    );
    assert_eq!(residents.iter().filter(|v| v.job_site.is_none()).count(), 1);
    assert!(residents.iter().all(|v| v.job_site
        != Some(BlockPos {
            x: 12,
            y: 201,
            z: 12
        })));
    assert_eq!(
        state
            .entities
            .row(outsider)
            .villager
            .as_ref()
            .unwrap()
            .job_site,
        Some(BlockPos {
            x: 12,
            y: 201,
            z: 12
        })
    );
}
#[test]
fn roofed_enclosed_house_with_real_bed_certifies() {
    let mut state = test_state();
    let id = town(&mut state);
    let stone = state.registry.states.stone;
    for x in 3..=7 {
        for z in 3..=7 {
            state.world.set_block(x, 204, z, stone);
            if x == 3 || x == 7 || z == 3 || z == 7 {
                for y in 201..204 {
                    state.world.set_block(x, y, z, stone);
                }
            }
        }
    }
    let door = block(&state, "minecraft:oak_door", &[("half", "lower")]);
    let upper = block(&state, "minecraft:oak_door", &[("half", "upper")]);
    state.world.set_block(3, 201, 5, door);
    state.world.set_block(3, 202, 5, upper);
    crate::settlements::allocator::tests::bed(&mut state, 5, 5);
    state.world.fold_overlay();
    assert!(certify(&mut state, id, BuildingKind::House, bounds()).is_ok());
}

#[test]
fn every_vanilla_workshop_maps_only_its_matching_station() {
    let mut state = test_state();
    let id = town(&mut state);
    for (kind, station_name) in [
        ("armorer", "blast_furnace"),
        ("butcher", "smoker"),
        ("cartographer", "cartography_table"),
        ("cleric", "brewing_stand"),
        ("farmer", "composter"),
        ("fisherman", "barrel"),
        ("fletcher", "fletching_table"),
        ("leatherworker", "cauldron"),
        ("librarian", "lectern"),
        ("mason", "stonecutter"),
        ("shepherd", "loom"),
        ("toolsmith", "smithing_table"),
        ("weaponsmith", "grindstone"),
    ] {
        let kind = BuildingKind::parse(kind).unwrap();
        station(
            &mut state,
            [4, 201, 4],
            &format!("minecraft:{station_name}"),
        );
        let points = inspect(&state, kind, 1, bounds()).unwrap();
        assert_eq!(points, vec![[4, 201, 4]]);
        let record = state.settlements.get_mut(id).unwrap();
        record.buildings.clear();
        let building = certify(&mut state, id, kind, bounds()).unwrap();
        assert_eq!(
            job_profession(
                &state.settlements,
                Some(id),
                BlockPos { x: 4, y: 201, z: 4 }
            ),
            kind.profession()
        );
        assert_eq!(
            state.settlements.get(id).unwrap().buildings[&building].kind,
            kind
        );
    }
}

#[test]
fn recertification_expands_same_building_before_upgrade_without_duplicate_id() {
    let mut state = test_state();
    let id = town(&mut state);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    let small = ScriptWorldBounds::new([3, 200, 3], [5, 204, 5]).unwrap();
    let building = certify(
        &mut state,
        id,
        BuildingKind::Workshop(Profession::Farmer),
        small,
    )
    .unwrap();
    assert!(upgrade(&mut state, building).is_err());
    station(&mut state, [6, 201, 4], "minecraft:composter");
    assert_eq!(
        certify(
            &mut state,
            id,
            BuildingKind::Workshop(Profession::Farmer),
            bounds()
        )
        .unwrap(),
        building
    );
    upgrade(&mut state, building).unwrap();
    assert_eq!(state.settlements.get(id).unwrap().buildings.len(), 1);
    assert_eq!(
        state.settlements.get(id).unwrap().buildings[&building].level,
        2
    );
}

#[test]
fn storage_and_extraction_certificates_supply_real_work_points() {
    let mut state = test_state();
    let id = town(&mut state);
    station(&mut state, [4, 201, 4], "minecraft:chest");
    certify(&mut state, id, BuildingKind::Storage, bounds()).unwrap();
    assert_eq!(storage_points(&state, id), vec![[4, 201, 4]]);
    let mine_bounds = ScriptWorldBounds::new([9, 199, 3], [13, 204, 7]).unwrap();
    station(&mut state, [10, 201, 4], "minecraft:smithing_table");
    let building = certify(&mut state, id, BuildingKind::Mine, mine_bounds).unwrap();
    assert_eq!(
        job_profession(
            &state.settlements,
            Some(id),
            BlockPos {
                x: 10,
                y: 201,
                z: 4
            }
        ),
        Some(Profession::Miner)
    );
    let v = VillagerState {
        settlement: Some(id),
        job_site: Some(BlockPos {
            x: 10,
            y: 201,
            z: 4,
        }),
        job_site_dimension: state.dimension.name().into(),
        ..VillagerState::default()
    };
    assert_eq!(building_for_job(&state, &v).unwrap().id, building);
}

#[test]
fn solid_wall_requires_real_height_and_continuity() {
    let mut state = test_state();
    let id = town(&mut state);
    let selected = ScriptWorldBounds::new([3, 201, 3], [7, 203, 3]).unwrap();
    assert!(certify(&mut state, id, BuildingKind::Wall, selected).is_err());
    for x in 3..=7 {
        for y in 201..=203 {
            state.world.set_block(x, y, 3, state.registry.states.stone);
        }
    }
    state.world.fold_overlay();
    let wall = certify(&mut state, id, BuildingKind::Wall, selected).unwrap();
    assert_eq!(
        state.settlements.get(id).unwrap().buildings[&wall].kind,
        BuildingKind::Wall
    );
}

#[test]
fn upgrade_wakes_idle_residents_and_fills_new_slot() {
    let mut state = test_state();
    state.day_time = 2000;
    let id = town(&mut state);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    station(&mut state, [6, 201, 4], "minecraft:composter");
    let building = certify(
        &mut state,
        id,
        BuildingKind::Workshop(Profession::Farmer),
        bounds(),
    )
    .unwrap();
    let residents = [
        villager(&mut state, [3.5, 201.0, 4.5]),
        villager(&mut state, [6.5, 201.0, 5.5]),
    ];
    for slot in residents {
        let entity = state.entities.body(slot).id;
        crate::settlements::bind(&mut state, id, entity).unwrap();
    }
    let advance = |state: &mut TickState| {
        for _ in 0..80 {
            state.tick_budget.reset(state.tick);
            let mut changes = ChangeSet::default();
            crate::settlements::allocator::tick(state, &mut changes);
            state.tick += 1;
        }
    };
    advance(&mut state);
    assert_eq!(
        residents
            .iter()
            .filter(|&&slot| state
                .entities
                .row(slot)
                .villager
                .as_ref()
                .unwrap()
                .job_site
                .is_some())
            .count(),
        1
    );
    upgrade(&mut state, building).unwrap();
    advance(&mut state);
    let jobs = residents.map(|slot| state.entities.row(slot).villager.as_ref().unwrap().job_site);
    assert!(jobs.iter().all(Option::is_some));
    assert_ne!(jobs[0], jobs[1]);
}

#[test]
fn allocator_and_brain_assign_custom_profession_not_vanilla_workstation_look() {
    for (kind, station_name, profession) in [
        (
            BuildingKind::Mine,
            "minecraft:smithing_table",
            Profession::Miner,
        ),
        (
            BuildingKind::LumberCamp,
            "minecraft:fletching_table",
            Profession::Lumberjack,
        ),
    ] {
        let mut state = test_state();
        state.day_time = 2000;
        let id = town(&mut state);
        station(&mut state, [4, 201, 4], station_name);
        certify(&mut state, id, kind, bounds()).unwrap();
        let slot = villager(&mut state, [3.5, 201.0, 4.5]);
        let entity = state.entities.body(slot).id;
        crate::settlements::bind(&mut state, id, entity).unwrap();
        for _ in 0..40 {
            state.tick_budget.reset(state.tick);
            let mut changes = ChangeSet::default();
            crate::settlements::allocator::tick(&mut state, &mut changes);
            crate::villager::brain::tick(&mut state, &mut changes);
            state.tick += 1;
        }
        let v = state.entities.row(slot).villager.as_ref().unwrap();
        assert_eq!(v.job_site, Some(BlockPos { x: 4, y: 201, z: 4 }));
        assert_eq!(v.data.profession, profession);
    }
}

#[test]
fn persisted_storage_rejects_missing_container_points() {
    let mut state = test_state();
    let settlement = town(&mut state);
    station(&mut state, [4, 201, 4], "minecraft:chest");
    let id = certify(&mut state, settlement, BuildingKind::Storage, bounds()).unwrap();
    let mut record = state.settlements.get(settlement).unwrap().clone();
    record.buildings.get_mut(&id).unwrap().work_points.clear();
    assert!(
        validate_record_buildings(&record).is_err(),
        "a saved certificate must retain its required container point count",
    );
}

#[test]
fn existing_world_releases_uncertified_jobs_keeps_professions_offers_and_reassigns() {
    let mut state = test_state();
    state.day_time = 2000;
    let settlement = town(&mut state);
    let mut residents = Vec::new();
    // Restore a pre-certification world's durable bound profession/job columns.
    for (at, name, profession) in [
        ([4, 201, 4], "minecraft:composter", Profession::Farmer),
        ([10, 201, 10], "minecraft:lectern", Profession::Librarian),
    ] {
        station(&mut state, at, name);
        let slot = villager(
            &mut state,
            [f64::from(at[0]) - 0.5, 201.0, f64::from(at[2]) + 0.5],
        );
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.settlement = Some(settlement);
        v.job_site = Some(BlockPos {
            x: at[0],
            y: at[1],
            z: at[2],
        });
        v.job_site_dimension = state.dimension.name().into();
        v.data.profession = profession;
        crate::villager::trades::initialize_offers(v, &state.registry);
        assert!(!v.offers.is_empty());
        assert_eq!(v.xp, 0, "fixture must cover untraded professions");
        residents.push((slot, v.job_site, profession, v.offers.clone()));
    }
    let advance = |state: &mut TickState| {
        for _ in 0..40 {
            state.tick_budget.reset(state.tick);
            let mut changes = ChangeSet::default();
            crate::settlements::allocator::tick(state, &mut changes);
            crate::villager::brain::tick(state, &mut changes);
            state.tick += 1;
        }
    };
    advance(&mut state);
    for (slot, _, profession, offers) in &residents {
        let v = state.entities.row(*slot).villager.as_ref().unwrap();
        assert!(v.job_site.is_none(), "uncertified jobs must be released");
        assert_eq!(
            v.data.profession, *profession,
            "retain the saved profession"
        );
        assert_eq!(&v.offers, offers, "retain the saved untraded offers");
    }
    certify(
        &mut state,
        settlement,
        BuildingKind::Workshop(Profession::Farmer),
        bounds(),
    )
    .unwrap();
    certify(
        &mut state,
        settlement,
        BuildingKind::Workshop(Profession::Librarian),
        ScriptWorldBounds::new([9, 200, 9], [13, 204, 13]).unwrap(),
    )
    .unwrap();
    advance(&mut state);
    for (slot, job, profession, offers) in &residents {
        let v = state.entities.row(*slot).villager.as_ref().unwrap();
        assert_eq!(v.job_site, *job, "certification restores reachable slots");
        assert_eq!(v.data.profession, *profession);
        assert_eq!(&v.offers, offers);
    }
}

#[test]
fn first_certificate_releases_uncertified_job_without_erasing_profession() {
    let mut state = test_state();
    let settlement = town(&mut state);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    let slot = villager(&mut state, [3.5, 201.0, 4.5]);
    let owner = state.entities.body(slot).id;
    let mut resident = state.entities.row(slot).villager.clone().unwrap();
    resident.settlement = Some(settlement);
    resident.job_site = Some(BlockPos { x: 4, y: 201, z: 4 });
    resident.job_site_dimension = state.dimension.name().into();
    resident.data.profession = Profession::Farmer;
    state.entities.row_mut(slot).villager = Some(resident.clone());
    station(&mut state, [6, 201, 6], "minecraft:chest");
    certify(&mut state, settlement, BuildingKind::Storage, bounds()).unwrap();
    let mut cache = crate::settlements::allocator::PoiCache::default();
    cache.refresh(&state);
    for _ in 0..3 {
        crate::settlements::allocator::validate_pois(&state, &cache, owner, &mut resident);
        assert!(
            resident.job_site.is_none(),
            "storage certification does not authorize this workstation"
        );
        assert_eq!(
            resident.data.profession,
            Profession::Farmer,
            "certificate absence releases the claim, not the profession"
        );
    }
}

#[test]
fn idle_bound_resident_requires_certification_to_acquire_workstation_profession() {
    let mut state = test_state();
    state.day_time = 2000;
    let settlement = town(&mut state);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    let slot = villager(&mut state, [3.5, 201.0, 4.5]);
    let owner = state.entities.body(slot).id;
    crate::settlements::bind(&mut state, settlement, owner).unwrap();
    for _ in 0..40 {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(&mut state, &mut changes);
        crate::villager::brain::tick(&mut state, &mut changes);
        state.tick += 1;
    }
    let resident = state.entities.row(slot).villager.as_ref().unwrap();
    assert!(resident.job_site.is_none());
    assert_eq!(resident.data.profession, Profession::None);
    certify(
        &mut state,
        settlement,
        BuildingKind::Workshop(Profession::Farmer),
        bounds(),
    )
    .unwrap();
    for _ in 0..40 {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(&mut state, &mut changes);
        crate::villager::brain::tick(&mut state, &mut changes);
        state.tick += 1;
    }
    let resident = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(resident.job_site, Some(BlockPos { x: 4, y: 201, z: 4 }));
    assert_eq!(resident.data.profession, Profession::Farmer);
}
