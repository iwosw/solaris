//! Job-memory regression: `job_site` retention across distant sleep,
//! site-chunk unload/reload, and multi-day return to work.
use super::*;

#[test]
fn job_survives_distant_sleep_unload_across_three_days() {
    // Bed head (6,201,6) in chunk (0,0), composter (20,201,10) in chunk (1,0):
    // ~14.6 blocks apart, so the villager must commute across the boundary.
    let mut state = control_state();
    let want = rustc_hash::FxHashMap::from_iter([
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
        (crate::boundary::ChunkPos { x: 1, z: 0 }, 0),
    ]);
    state.world.ensure_blocking(&want);
    state.tickets.insert(
        "job-chunk".into(),
        (crate::boundary::ChunkPos { x: 1, z: 0 }, 0),
    );
    state.refresh_active_chunks(state.tick);
    // Flat floor in the job chunk mirrors the (0,0) fixture floor so the
    // cross-chunk commute is walkable.
    for x in 16..32 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..206 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    state.day_time = 2000;
    let _home = bed(&mut state, 6, 6);
    let job = crate::boundary::BlockPos {
        x: 20,
        y: 201,
        z: 10,
    };
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(job.x, job.y, job.z, composter);
    state.world.fold_overlay();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);

    // Real acquisition: allocator claim plus brain profession takeover.
    let mut acquired = false;
    for _ in 0..600 {
        step(&mut state);
        let v = state.entities.row(slot).villager.as_ref().unwrap();
        if v.job_site == Some(job)
            && v.data.profession == crate::villager::VillagerProfession::Farmer
        {
            acquired = true;
            break;
        }
    }
    assert!(
        acquired,
        "villager must claim the composter and take Farmer through the real acquisition path"
    );

    let job_centre = [
        f64::from(job.x) + 0.5,
        f64::from(job.y),
        f64::from(job.z) + 0.5,
    ];
    let reach = crate::settlements::allocator::JOB_SITE_REACH_SQUARED;
    for night in 0..3 {
        // Rest on a distinct day; the villager walks home and sleeps far away.
        state.day_time = i64::from(night) * 24_000 + 13_000;
        let mut slept = false;
        for _ in 0..900 {
            step(&mut state);
            let v = state.entities.row(slot).villager.as_ref().unwrap();
            assert_eq!(
                v.job_site,
                Some(job),
                "night {night}: job site must survive validation while the villager sleeps away"
            );
            assert_eq!(
                v.data.profession,
                crate::villager::VillagerProfession::Farmer,
                "night {night}: untraded profession must survive with its site"
            );
            if v.sleeping {
                slept = true;
                let here = state.entities.pos(slot);
                assert!(
                    crate::settlements::allocator::distance(here, job_centre) > reach,
                    "night {night}: sleep must happen far from the workstation"
                );
                break;
            }
        }
        assert!(slept, "night {night}: villager must actually fall asleep");

        // Unload only the site chunk mid-sleep: absence is unknown, not
        // destroyed, so the sleeping claim must survive.
        let flushed: Vec<_> = state
            .world
            .chunks()
            .map(|chunk| (chunk.pos, state.world.storage_revision(chunk.pos)))
            .collect();
        state.world.mark_flushed(&flushed);
        let keep =
            rustc_hash::FxHashMap::from_iter([(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]);
        let evicted = state.world.evict_outside(&keep);
        assert!(
            evicted.contains(&crate::boundary::ChunkPos { x: 1, z: 0 }),
            "night {night}: site chunk must actually unload while sleeping"
        );
        for _ in 0..60 {
            step(&mut state);
            let v = state.entities.row(slot).villager.as_ref().unwrap();
            assert_eq!(
                v.job_site,
                Some(job),
                "night {night}: unloaded station must not drop the sleeping claim"
            );
            assert_eq!(
                v.data.profession,
                crate::villager::VillagerProfession::Farmer,
                "night {night}: profession must survive the unloaded site"
            );
        }

        // Re-admit the site chunk. `ensure_blocking` regenerates from the
        // terrain generator, so re-apply the floor plus composter to simulate
        // the persisted stored chunk content a real load would return.
        state.world.ensure_blocking(&want);
        for x in 16..32 {
            for z in 0..16 {
                state
                    .world
                    .set_block(x, 200, z, state.registry.states.stone);
                for y in 201..206 {
                    state.world.set_block(x, y, z, state.registry.states.air);
                }
            }
        }
        state.world.set_block(job.x, job.y, job.z, composter);
        state.world.fold_overlay();
        // Fixture lifecycle matches the runtime tick: resident demand alone
        // does not reactivate; refresh the membership gate or the villager
        // stays frozen once it crosses into the reloaded chunk.
        state.refresh_active_chunks(state.tick + 1);

        // Next distinct day: wake, keep the site, physically return to work.
        state.day_time = i64::from(night + 1) * 24_000 + 3000;
        let mut arrived_at: Option<u64> = None;
        for _ in 0..1200 {
            step(&mut state);
            let v = state.entities.row(slot).villager.as_ref().unwrap();
            assert_eq!(
                v.job_site,
                Some(job),
                "day {night}: job site must survive the return to work"
            );
            assert_eq!(
                v.data.profession,
                crate::villager::VillagerProfession::Farmer,
                "day {night}: profession must survive the return to work"
            );
            if arrived_at.is_none()
                && crate::settlements::allocator::distance(state.entities.pos(slot), job_centre)
                    <= reach
            {
                arrived_at = Some(state.tick);
            }
            if arrived_at.is_some_and(|tick| state.tick >= tick + 40) {
                break;
            }
        }
        if arrived_at.is_none() {
            let v = state.entities.row(slot).villager.as_ref().unwrap();
            panic!(
                "day {night}: villager must physically reach its station each workday \
                (pos={:?} nav_target={:?} nav_len={} last_work={} day_time={} home={:?} active01={}/{})",
                state.entities.pos(slot),
                v.navigation_target,
                v.navigation_path.len(),
                v.last_work,
                state.day_time,
                v.home,
                state.world.is_active(0, 0),
                state.world.is_active(1, 0),
            );
        }

        let arrived = arrived_at.unwrap();
        let v = state.entities.row(slot).villager.as_ref().unwrap();

        assert!(!v.sleeping, "day {night}: villager must wake for work");
        assert!(
            v.last_work >= i64::try_from(arrived).unwrap_or(i64::MAX),
            "day {night}: work at the station must stamp last_work (arrived tick {arrived}, last_work {})",
            v.last_work
        );
    }
}

#[test]
fn job_survives_site_unload_reload_but_not_destruction() {
    // Spread-memories fixture mirrors `poi_validation_fixture`: bed head in
    // chunk 1, bell in chunk 2, composter in chunk 3.
    let mut state = test_state();
    let want = rustc_hash::FxHashMap::from_iter(
        (0..=4).map(|x| (crate::boundary::ChunkPos { x, z: 0 }, 0)),
    );
    state.world.ensure_blocking(&want);
    let home = bed(&mut state, 16, 8);
    let meeting = crate::boundary::BlockPos {
        x: 32,
        y: 201,
        z: 8,
    };
    let job = crate::boundary::BlockPos {
        x: 48,
        y: 201,
        z: 8,
    };
    let bell = block(&state, "minecraft:bell", &[]);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(meeting.x, meeting.y, meeting.z, bell);
    state.world.set_block(job.x, job.y, job.z, composter);
    state.world.fold_overlay();
    let mut cache = crate::settlements::allocator::PoiCache::default();
    cache.refresh(&state);
    let owner = crate::boundary::EntityId(-1);
    let mut v = VillagerState {
        home: Some(home),
        meeting_point: Some(meeting),
        job_site: Some(job),
        data: crate::villager::VillagerData {
            profession: Profession::Farmer,
            ..Default::default()
        },
        ..Default::default()
    };
    crate::settlements::allocator::validate_pois(&state, &cache, owner, &mut v);
    assert_eq!(
        (v.home, v.meeting_point, v.job_site),
        (Some(home), Some(meeting), Some(job)),
        "baseline memories must validate while loaded"
    );

    // Evict only the workstation chunk: absence is unknown, not destroyed.
    let flushed: Vec<_> = state
        .world
        .chunks()
        .map(|chunk| (chunk.pos, state.world.storage_revision(chunk.pos)))
        .collect();
    state.world.mark_flushed(&flushed);
    let keep = rustc_hash::FxHashMap::from_iter(
        [0, 1, 2, 4].map(|x| (crate::boundary::ChunkPos { x, z: 0 }, 0)),
    );
    let evicted = state.world.evict_outside(&keep);
    assert!(
        evicted.contains(&crate::boundary::ChunkPos { x: 3, z: 0 }),
        "workstation chunk must actually unload for this regression"
    );
    crate::settlements::allocator::validate_pois(&state, &cache, owner, &mut v);
    assert_eq!(
        (v.home, v.meeting_point, v.job_site),
        (Some(home), Some(meeting), Some(job)),
        "unloaded station is unknown geometry, not evidence of destruction"
    );
    crate::settlements::allocator::validate_pois(&state, &cache, owner, &mut v);
    assert_eq!(
        (v.home, v.meeting_point, v.job_site),
        (Some(home), Some(meeting), Some(job)),
        "repeated passes over the absent chunk must not erode the memory"
    );

    // Reload, then re-apply the composter before re-admission: `ensure`
    // regenerates from the terrain generator, so this simulates the persisted
    // stored station a real load would return.
    state.world.ensure_blocking(&want);
    state.world.set_block(job.x, job.y, job.z, composter);
    state.world.fold_overlay();
    crate::settlements::allocator::validate_pois(&state, &cache, owner, &mut v);
    assert_eq!(
        (v.home, v.meeting_point, v.job_site),
        (Some(home), Some(meeting), Some(job)),
        "reloaded station must keep the original claim"
    );

    // Negative control: destroying the station still drops an untraded claim.
    state
        .world
        .set_block(job.x, job.y, job.z, state.registry.states.air);
    state.world.fold_overlay();
    crate::settlements::allocator::validate_pois(&state, &cache, owner, &mut v);
    assert_eq!(v.job_site, None, "destroyed station must release the claim");
    assert_eq!(
        v.data.profession,
        crate::villager::VillagerProfession::None,
        "untraded profession goes with its destroyed site"
    );

    // A traded villager loses the site but keeps its profession.
    state.world.set_block(job.x, job.y, job.z, composter);
    state.world.fold_overlay();
    v.job_site = Some(job);
    v.data.profession = crate::villager::VillagerProfession::Farmer;
    v.xp = 1;
    crate::settlements::allocator::validate_pois(&state, &cache, owner, &mut v);
    assert_eq!(
        v.job_site,
        Some(job),
        "traded villager keeps its matching live site"
    );
    state
        .world
        .set_block(job.x, job.y, job.z, state.registry.states.air);
    state.world.fold_overlay();
    crate::settlements::allocator::validate_pois(&state, &cache, owner, &mut v);
    assert_eq!(
        v.job_site, None,
        "destroyed station must release even a traded claim"
    );
    assert_eq!(
        v.data.profession,
        crate::villager::VillagerProfession::Farmer,
        "traded profession survives its destroyed site"
    );
}
