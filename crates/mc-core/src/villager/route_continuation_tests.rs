use super::*;

fn distant_state() -> TickState {
    let mut state = test_state();
    let want = rustc_hash::FxHashMap::from_iter(
        (0..4).map(|x| (crate::boundary::ChunkPos { x, z: 0 }, 0)),
    );
    state.world.ensure_blocking(&want);
    for x in 0..64 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..206 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    state.world.fold_overlay();
    for (name, x) in [
        ("distant-fixture-1", 1),
        ("distant-fixture-2", 2),
        ("distant-fixture-3", 3),
    ] {
        state
            .tickets
            .insert(name.into(), (crate::boundary::ChunkPos { x, z: 0 }, 0));
    }
    state.refresh_active_chunks(0);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state.villager_pois.schedule_window = 200;
    state
}

fn distant_claim_fixture() -> (TickState, EntitySlot, BlockPos) {
    let mut state = distant_state();
    state.day_time = 3000;
    let job = BlockPos {
        x: 57,
        y: 201,
        z: 8,
    };
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(job.x, job.y, job.z, composter);
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let mut sequence = state.settlement_sequence;
    let town = state
        .settlements
        .create(
            &mut sequence,
            "town".into(),
            "owner".into(),
            [32, 201, 8],
            64,
        )
        .unwrap();
    state.settlement_sequence = sequence;
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(town);
    // Isolate discovery from the allocator's staggered twenty-tick admission.
    state
        .villager_pois
        .search_at
        .insert(state.entities.body(slot).id, 0);
    (state, slot, job)
}

#[test]
fn distant_settlement_job_is_claimed_beyond_vanilla_reach() {
    let (mut state, slot, job) = distant_claim_fixture();
    for _ in 0..10 {
        step(&mut state);
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(
        v.job_site,
        Some(job),
        "bound villager must discover a reachable job inside its settlement radius even beyond 48 blocks"
    );
    assert_eq!(v.navigation_target, Some(job));
}

#[test]
fn distant_claim_keeps_walking_without_forty_tick_abandon() {
    let (mut state, slot, job) = distant_claim_fixture();
    for _ in 0..10 {
        step(&mut state);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(job),
        "setup: distant job must be claimed first"
    );
    for _ in 0..600 {
        step(&mut state);
        if state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .data
            .profession
            == Profession::Farmer
            && distance(state.entities.pos(slot), centre(job))
                <= crate::settlements::allocator::JOB_SITE_REACH_SQUARED
        {
            break;
        }
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(
        v.job_site,
        Some(job),
        "must not abandon the reachable claim"
    );
    assert_eq!(
        v.data.profession,
        Profession::Farmer,
        "arrival at the composter must employ the villager"
    );
    assert!(
        crate::settlements::allocator::distance(state.entities.pos(slot), centre(job))
            <= crate::settlements::allocator::JOB_SITE_REACH_SQUARED,
        "must arrive within workstation reach, not merely make progress"
    );
}

#[test]
fn due_replan_search_failure_keeps_valid_progress_route() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let target = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let mut v = VillagerState::default();
    let mut cache = PoiCache::default();
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert!(!v.navigation_path.is_empty(), "setup must produce a route");
    let walking = v.navigation_path.clone();
    set_node_allowance(&mut state, 1);
    state.tick = v.navigation_repath_at;
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert_eq!(v.navigation_target, Some(target));
    assert_eq!(
        v.navigation_path, walking,
        "a failed periodic replan must not clear a valid progress route"
    );
    assert!(
        v.navigation_repath_at > state.tick,
        "live routes must not force an overdue brain wake every tick"
    );
}

#[test]
fn pushed_off_route_replans_from_pushed_position_at_due_check() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let target = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let mut v = VillagerState::default();
    let mut cache = PoiCache::default();
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert!(!v.navigation_path.is_empty(), "setup must produce a route");
    let stale = v.navigation_path.clone();
    // Simulate a push/shove: body displaced sideways, world unchanged.
    let pushed = [8.5, 201.0, 12.5];
    state.entities.set_pos(slot, pushed);
    state.tick = v.navigation_repath_at;
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert_eq!(v.navigation_target, Some(target));
    assert!(
        v.navigation_path != stale,
        "a pushed-off-route villager must replan at the due check instead of keeping the stale route"
    );
    let next = centre(
        *v.navigation_path
            .last()
            .expect("replanned route must be nonempty"),
    );
    assert!(
        distance(pushed, next) <= 4.0,
        "new route must detour from the actual pushed position, not the stale origin"
    );
    assert!(
        v.navigation_repath_at > state.tick,
        "replanned routes must renew the brain wake deadline"
    );
}

#[test]
fn unchanged_waypoint_distance_is_stalled_not_progress() {
    let mut state = test_state();
    let start = [8.5, 201.0, 8.5];
    let slot = villager(&mut state, start);
    let target = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let next = BlockPos { x: 8, y: 201, z: 9 };
    // A valid, longer route remains committed while crowding prevents movement.
    let mut v = VillagerState {
        navigation_path: vec![
            BlockPos {
                x: 10,
                y: 201,
                z: 8,
            },
            BlockPos { x: 9, y: 201, z: 8 },
            BlockPos { x: 9, y: 201, z: 9 },
            next,
        ],
        navigation_target: Some(target),
        navigation_repath_at: 40,
        navigation_progress_len: 4,
        navigation_progress_next: Some(next),
        navigation_progress_dist_sq: distance(start, centre(next)),
        ..Default::default()
    };
    capture_route(&state, &mut v, start, true);
    let old = v.navigation_path.clone();
    state.tick = v.navigation_repath_at;
    let mut cache = PoiCache::default();
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert_ne!(
        v.navigation_path, old,
        "equal distance is not progress; replan the stalled route"
    );
}

#[test]
fn changed_target_zero_step_route_drops_old_path() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let mut v = VillagerState::default();
    let mut cache = PoiCache::default();
    move_to(
        &mut state,
        &mut cache,
        slot,
        &mut v,
        Some(BlockPos {
            x: 11,
            y: 201,
            z: 8,
        }),
        false,
    );
    assert!(!v.navigation_path.is_empty());
    let nearby = at(state.entities.pos(slot));
    move_to(&mut state, &mut cache, slot, &mut v, Some(nearby), false);
    assert_eq!(v.navigation_target, Some(nearby));
    assert!(
        v.navigation_path.is_empty(),
        "arrival at a new target cannot restore the previous target's path"
    );
}

#[test]
fn changed_geometry_never_restores_budget_denied_route() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let target = BlockPos {
        x: 11,
        y: 201,
        z: 8,
    };
    let mut v = VillagerState::default();
    let mut cache = PoiCache::default();
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    let blocked = *v.navigation_path.last().unwrap();
    state
        .world
        .set_block(blocked.x, blocked.y, blocked.z, state.registry.states.stone);
    assert!(route_changed(&state, &v));
    set_node_allowance(&mut state, 1);
    move_to(&mut state, &mut cache, slot, &mut v, Some(target), false);
    assert!(
        v.navigation_path.is_empty(),
        "budget exhaustion cannot restore a route invalidated by an obstacle"
    );
}
