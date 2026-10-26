//! Event-driven validation versus an independent full-population validator.
//! More than 128 residents prevent the round-robin backstop from hiding lost
//! events; both paths consume the same row/world pushes before validation.
use super::tests::{bed, block, test_state, villager};
use super::{Poi, PoiKind};
use crate::boundary::{BlockPos, ChunkPos, EntityId};
use crate::tick::TickState;
use crate::villager::{VillagerProfession as Profession, VillagerState};

const OVERWORLD: &str = "minecraft:overworld";
const NETHER: &str = "minecraft:the_nether";

/// More than 128 residents, so the round-robin probes (3 at 260) cannot
/// accidentally validate every churned resident.
const ORACLE_RESIDENTS: usize = 260;

/// Load extra chunks, flatten POI chunks to the `test_state` floor profile,
/// and widen activity to cover them. Chunk (2,0) is loaded but *not*
/// flattened so it stays clean for the evict/reload leg.
fn load_oracle_chunks(state: &mut TickState) {
    let chunks = [(1, 0), (0, 1), (1, 1), (2, 0)];
    let want = rustc_hash::FxHashMap::from_iter(
        [(0, 0), (1, 0), (0, 1), (1, 1), (2, 0)]
            .into_iter()
            .map(|(x, z)| (ChunkPos { x, z }, 0)),
    );
    state.world.ensure_blocking(&want);
    let stone = state.registry.states.stone;
    let air = state.registry.states.air;
    for (cx, cz) in chunks[..3].iter().copied() {
        for lx in 0..16 {
            for lz in 0..16 {
                let (x, z) = (cx * 16 + lx, cz * 16 + lz);
                state.world.set_block(x, 200, z, stone);
                for y in 201..206 {
                    state.world.set_block(x, y, z, air);
                }
            }
        }
    }
    state.world.fold_overlay();
    state
        .tickets
        .insert("oracle-fixture".into(), (ChunkPos { x: 0, z: 0 }, 2));
    let tick = state.tick;
    state.refresh_active_chunks(tick);
}

struct OracleWorld {
    state: TickState,
    ids: Vec<EntityId>,
    beds: Vec<BlockPos>,
    jobs: Vec<BlockPos>,
    bells: Vec<BlockPos>,
    cross_foot: BlockPos,
    air: u32,
    prefix_mobs: Vec<EntityId>,
}

/// Deterministic 260-resident world: 120 unique beds plus 40 conflicting
/// duplicates (larger id shares the smaller id's bed), 40 workstations, 8
/// shared bells, one cross-chunk bed, and a clean (2,0) chunk holding
/// unclaimed residents for the evict leg.
fn build_oracle(prefix_count: usize) -> OracleWorld {
    let mut state = test_state();
    load_oracle_chunks(&mut state);
    let air = state.registry.states.air;
    let cow = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let prefix_mobs = (0..prefix_count)
        .map(|_| state.spawn_mob(cow, [5_000.5, 201.0, 5_000.5], 20.0))
        .collect();

    let mut beds = Vec::new();
    'grid: for &(cx, cz) in [(0, 0), (1, 0), (0, 1), (1, 1)].iter() {
        for lz in [2, 4, 6, 8, 10, 12] {
            for hx in [2, 4, 6, 8, 10, 12, 14] {
                if beds.len() >= 120 {
                    break 'grid;
                }
                beds.push(bed(&mut state, cx * 16 + hx, cz * 16 + lz));
            }
        }
    }
    // Cross-chunk bed: foot (15,201,8) in (0,0), head (16,201,8) in (1,0).
    // Grid leaves both cells free (row z=8 uses local x 1..14; local 15 free).
    let cross_head = bed(&mut state, 16, 8);
    let cross_foot = BlockPos {
        x: 15,
        y: 201,
        z: 8,
    };

    let composter = block(&state, "minecraft:composter", &[]);
    let mut jobs = Vec::new();
    for (i, &(cx, cz)) in [(0, 0), (1, 0), (0, 1), (1, 1)].iter().enumerate() {
        for k in 0..10 {
            let (x, z) = (cx * 16, cz * 16 + (i * 10 + k) as i32 % 16);
            state.world.set_block(x, 201, z, composter);
            jobs.push(BlockPos { x, y: 201, z });
        }
    }
    let bell = block(&state, "minecraft:bell", &[]);
    let mut bells = Vec::new();
    for (i, &(cx, cz)) in [(0, 0), (1, 0), (0, 1), (1, 1)].iter().enumerate() {
        for k in 0..2 {
            let (x, z) = (cx * 16 + 8 + k, cz * 16);
            state.world.set_block(x, 201 + i as i32 % 2, z, bell);
            bells.push(BlockPos {
                x,
                y: 201 + i as i32 % 2,
                z,
            });
        }
    }
    state.world.fold_overlay();

    // Five-chunk round robin; every 5th resident lands in clean chunk (2,0).
    let areas = [(0, 0), (1, 0), (0, 1), (1, 1), (2, 0)];
    let mut ids = Vec::with_capacity(ORACLE_RESIDENTS);
    for i in 0..ORACLE_RESIDENTS {
        let (cx, cz) = areas[i % areas.len()];
        let slot = villager(
            &mut state,
            [
                f64::from(cx * 16 + 8) + 0.5,
                201.0,
                f64::from(cz * 16 + 8) + 0.5,
            ],
        );
        ids.push(state.entities.body(slot).id);
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.age = if i % 5 == 4 { -100 } else { 0 };
        let adult = v.age >= 0;
        if i < 120 {
            v.home = Some(beds[i]);
            v.home_dimension = OVERWORLD.into();
        } else if i < 160 {
            v.home = Some(beds[i - 120]);
            v.home_dimension = OVERWORLD.into();
        }
        if adult && i % 3 == 0 {
            let job = jobs[(i / 3) % jobs.len()];
            v.job_site = Some(job);
            v.job_site_dimension = OVERWORLD.into();
            v.data.profession = Profession::Farmer;
            if i % 9 == 0 {
                v.xp = 1; // traded: keeps profession when the site changes
            }
        } else if i % 11 == 0 {
            v.data.profession = Profession::Nitwit;
        }
        if i % 2 == 0 {
            let meeting = bells[(i / 2) % bells.len()];
            v.meeting_point = Some(meeting);
            v.meeting_point_dimension = OVERWORLD.into();
        }
    }
    // Cross-chunk conflict: index 10 (smaller id) and unclaimed 210 share it.
    for &i in [10, 210].iter() {
        let slot = state.entities.slot_of(ids[i]).unwrap();
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.home = Some(cross_head);
        v.home_dimension = OVERWORLD.into();
    }
    OracleWorld {
        state,
        ids,
        beds,
        jobs,
        bells,
        cross_foot,
        air,
        prefix_mobs,
    }
}

/// Event path prefix: consume world/entity pushes, refresh, validate dirty.
fn event_validate(state: &mut TickState) {
    let mut cache = std::mem::take(&mut state.villager_pois);
    cache.changed.clear();
    state.entities.swap_brain_changes(&mut cache.changed);
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(state);
    super::validate_changed_claims(state, &mut cache);
    state.villager_pois = cache;
}

/// Full oracle: same prefix, then validate *every* active resident to a fixed
/// point with the same [`super::validate_pois`] primitive, publishing through
/// the same [`super::PoiCache::update_claims`] so releases settle same-tick.
fn full_validate_all(state: &mut TickState, cache: &mut super::PoiCache) {
    for _ in 0..32 {
        let mut residents: Vec<EntityId> = state
            .entities
            .slots()
            .filter(|&slot| super::active(state, slot))
            .map(|slot| state.entities.body(slot).id)
            .collect();
        residents.sort();
        let mut settled = true;
        for id in residents {
            let Some(slot) = state.entities.slot_of(id) else {
                continue;
            };
            if !super::active(state, slot) {
                continue;
            }
            let old = state.entities.row(slot).villager.clone().unwrap();
            let mut v = old.clone();
            super::validate_pois(state, cache, id, &mut v);
            let old_claims = [old.home, old.job_site, old.meeting_point];
            let new_claims = [v.home, v.job_site, v.meeting_point];
            if old_claims != new_claims {
                cache.update_claims(id, old_claims, new_claims);
                settled = false;
            }
            // Geometry memos and foreign-dimension path clears are part of the
            // validated row even when the durable claim tuple is unchanged.
            if v.home != old.home
                || v.job_site != old.job_site
                || v.meeting_point != old.meeting_point
                || v.data.profession != old.data.profession
                || v.offers.len() != old.offers.len()
                || v.restocks_today != old.restocks_today
                || v.navigation_path != old.navigation_path
                || v.navigation_target != old.navigation_target
                || v.navigation_chunks != old.navigation_chunks
                || v.navigation_repath_at != old.navigation_repath_at
                || v.poi_validation.sites != old.poi_validation.sites
            {
                settled = false;
            }
            state.entities.row_mut_unmarked(slot).villager = Some(v);
            let after = state.entities.row(slot).villager.as_ref().unwrap();
            if (after.home, after.job_site, after.meeting_point)
                != (old.home, old.job_site, old.meeting_point)
                || after.data.profession != old.data.profession
                || after.offers.len() != old.offers.len()
                || after.restocks_today != old.restocks_today
            {
                state.entities.row_mut(slot);
            }
        }
        if settled {
            return;
        }
    }
    panic!("full claim validator did not converge");
}

fn oracle_validate(state: &mut TickState) {
    let mut cache = std::mem::take(&mut state.villager_pois);
    cache.changed.clear();
    state.entities.swap_brain_changes(&mut cache.changed);
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(state);
    full_validate_all(state, &mut cache);
    state.villager_pois = cache;
}

#[derive(Debug, PartialEq)]
struct ClaimSnapshot {
    home: Option<BlockPos>,
    home_dimension: String,
    job: Option<BlockPos>,
    job_dimension: String,
    meeting: Option<BlockPos>,
    meeting_dimension: String,
    profession: Profession,
    offers: usize,
    restocks: i32,
    path: Vec<BlockPos>,
    target: Option<BlockPos>,
    repath_at: u64,
}

fn snapshot(v: &VillagerState) -> ClaimSnapshot {
    ClaimSnapshot {
        home: v.home,
        home_dimension: v.home_dimension.clone(),
        job: v.job_site,
        job_dimension: v.job_site_dimension.clone(),
        meeting: v.meeting_point,
        meeting_dimension: v.meeting_point_dimension.clone(),
        profession: v.data.profession,
        offers: v.offers.len(),
        restocks: v.restocks_today,
        path: v.navigation_path.clone(),
        target: v.navigation_target,
        repath_at: v.navigation_repath_at,
    }
}

/// Validator-relevant rows must match id-for-id; active homes/jobs must be
/// singly owned (bells are intentionally shareable).
fn compare_oracle(event: &TickState, oracle: &TickState, label: &str) {
    let mut event_ids: Vec<EntityId> = event
        .entities
        .slots()
        .map(|slot| event.entities.body(slot).id)
        .collect();
    event_ids.sort();
    let mut oracle_ids: Vec<EntityId> = oracle
        .entities
        .slots()
        .map(|slot| oracle.entities.body(slot).id)
        .collect();
    oracle_ids.sort();
    assert_eq!(
        event_ids, oracle_ids,
        "entity population diverged ({label})"
    );
    for id in event_ids {
        let a = event.entities.slot_of_any(id).unwrap();
        let b = oracle.entities.slot_of_any(id).unwrap();
        let (Some(va), Some(vb)) = (
            event.entities.row(a).villager.as_ref(),
            oracle.entities.row(b).villager.as_ref(),
        ) else {
            assert!(
                event.entities.row(a).villager.is_none()
                    && oracle.entities.row(b).villager.is_none()
            );
            continue;
        };
        assert_eq!(
            snapshot(va),
            snapshot(vb),
            "validator state diverged for {id:?} ({label})"
        );
        assert_eq!(
            va.poi_validation.sites, vb.poi_validation.sites,
            "geometry memo diverged for {id:?} ({label})"
        );
    }
    assert_eq!(
        event.villager_pois.homes, oracle.villager_pois.homes,
        "home claim index diverged ({label})"
    );
    assert_eq!(
        event.villager_pois.jobs, oracle.villager_pois.jobs,
        "job claim index diverged ({label})"
    );
    for state in [event, oracle] {
        let mut homes = rustc_hash::FxHashSet::default();
        let mut jobs = rustc_hash::FxHashSet::default();
        for slot in state.entities.slots() {
            if !super::active(state, slot) {
                continue;
            }
            let v = state.entities.row(slot).villager.as_ref().unwrap();
            if let Some(home) = v.home {
                assert!(homes.insert(home), "duplicate home {home:?} ({label})");
            }
            if let Some(job) = v.job_site {
                assert!(jobs.insert(job), "duplicate job {job:?} ({label})");
            }
        }
    }
}

/// Deterministic churn applied identically to both worlds: bed/bell/job
/// edits, a cross-chunk foot break, conflicting hydration onto smaller-id
/// beds, adulthood, death/removal, wandering/conversion, foreign dimensions,
/// an active-set flip, and an evict/reload leg.
fn apply_churn(world: &mut OracleWorld) {
    let OracleWorld {
        state,
        ids,
        beds,
        jobs,
        bells,
        cross_foot,
        air,
        ..
    } = world;
    let air = *air;
    let cross_foot = *cross_foot;
    // Destroy every 7th bed head plus the cross-chunk foot.
    for (i, &head) in beds.iter().enumerate() {
        if i % 7 == 6 {
            state.world.set_block(head.x, head.y, head.z, air);
        }
    }
    state
        .world
        .set_block(cross_foot.x, cross_foot.y, cross_foot.z, air);
    // Bell and workstation edits; three composters become lecterns so traded
    // farmers keep their profession while untraded ones lose it.
    for (i, &bell) in bells.iter().enumerate() {
        if i % 4 == 1 {
            state.world.set_block(bell.x, bell.y, bell.z, air);
        }
    }
    let lectern = block(state, "minecraft:lectern", &[]);
    for (i, &job) in jobs.iter().enumerate() {
        if i % 8 == 2 {
            state.world.set_block(job.x, job.y, job.z, air);
        } else if i % 8 == 3 {
            state.world.set_block(job.x, job.y, job.z, lectern);
        }
    }
    state.world.fold_overlay();

    // Conflicting hydration: unclaimed 200..210 pile onto smaller-id beds.
    for k in 0..10 {
        let id = ids[200 + k];
        let slot = state.entities.slot_of(id).unwrap();
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.home = Some(beds[k]);
        v.home_dimension = OVERWORLD.into();
    }
    // Adulthood: first ten babies grow up.
    let mut grown = 0;
    for &id in ids.iter() {
        if grown >= 10 {
            break;
        }
        let slot = state.entities.slot_of(id).unwrap();
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        if v.age < 0 {
            v.age = 0;
            grown += 1;
        }
    }
    // Foreign dimensions clear on next validation (plus navigation).
    for i in [100, 101, 102] {
        let slot = state.entities.slot_of(ids[i]).unwrap();
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.home_dimension = NETHER.into();
    }
    // Wandering and conversion leave the active set.
    for i in [110, 111] {
        let slot = state.entities.slot_of(ids[i]).unwrap();
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .wandering = true;
    }
    for i in [112, 113] {
        let slot = state.entities.slot_of(ids[i]).unwrap();
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .conversion_time = Some(10);
    }
    // Death first, removals last (slots shift; always re-lookup by id).
    for i in [240, 241, 242, 243, 244] {
        let slot = state.entities.slot_of(ids[i]).unwrap();
        state.entities.body_mut(slot).alive = false;
    }
    for i in [250, 251, 252, 253, 254] {
        let slot = state.entities.slot_of_any(ids[i]).unwrap();
        state.remove_entity(slot);
    }
    // Active-set flip: chunk (1,0) goes dark for round one.
    let mut active: rustc_hash::FxHashSet<ChunkPos> = state.world.active_positions().collect();
    assert!(active.remove(&ChunkPos { x: 1, z: 0 }));
    let tick = state.tick;
    state.world.set_active_chunks(active, tick);
    // Evict/reload leg on clean chunk (2,0): eviction needs two pushes to
    // seed `previous_want` before the drop is observed.
    let full = rustc_hash::FxHashMap::from_iter(
        [(0, 0), (1, 0), (0, 1), (1, 1), (2, 0)]
            .into_iter()
            .map(|(x, z)| (ChunkPos { x, z }, 0)),
    );
    let without = rustc_hash::FxHashMap::from_iter(
        [(0, 0), (1, 0), (0, 1), (1, 1)]
            .into_iter()
            .map(|(x, z)| (ChunkPos { x, z }, 0)),
    );
    state.world.evict_outside(&full);
    state.world.evict_outside(&without);
    let back = rustc_hash::FxHashMap::from_iter([(ChunkPos { x: 2, z: 0 }, 0)]);
    state.world.ensure_blocking(&back);
    let mut active: rustc_hash::FxHashSet<ChunkPos> = state.world.active_positions().collect();
    active.insert(ChunkPos { x: 2, z: 0 });
    let tick = state.tick;
    state.world.set_active_chunks(active, tick);
}

#[test]
fn event_validation_matches_full_oracle_after_churn_and_reload() {
    let mut event = build_oracle(0);
    let mut oracle = build_oracle(0);
    assert_eq!(event.ids, oracle.ids, "fixture ids must be deterministic");
    assert_eq!(
        event.beds, oracle.beds,
        "fixture POIs must be deterministic"
    );
    // Settle initial duplicates identically before churning.
    event_validate(&mut event.state);
    oracle_validate(&mut oracle.state);
    compare_oracle(&event.state, &oracle.state, "hydrated");

    apply_churn(&mut event);
    apply_churn(&mut oracle);

    // Round one: (1,0) dark, (2,0) evicted and reloaded.
    event_validate(&mut event.state);
    oracle_validate(&mut oracle.state);
    compare_oracle(&event.state, &oracle.state, "churned");

    // Round two: (1,0) reloaded; every survivor validates again.
    for world in [&mut event, &mut oracle] {
        let mut active: rustc_hash::FxHashSet<ChunkPos> =
            world.state.world.active_positions().collect();
        active.insert(ChunkPos { x: 1, z: 0 });
        let tick = world.state.tick;
        world.state.world.set_active_chunks(active, tick);
    }
    event_validate(&mut event.state);
    oracle_validate(&mut oracle.state);
    compare_oracle(&event.state, &oracle.state, "reloaded");
    for tick in 1..=32 {
        event.state.tick = tick;
        oracle.state.tick = tick;
        event_validate(&mut event.state);
        oracle_validate(&mut oracle.state);
        compare_oracle(&event.state, &oracle.state, "settled ticks");
    }
}

/// Real sleep/wake events must dirty an owner outside the RR probe slice,
/// without a resident row mutation or a POI block revision.
#[test]
fn occupied_bed_blocks_claim_through_event_gate() {
    use crate::boundary::{ConnectionId, PlayerInput};
    let mut state = test_state();
    let connection = ConnectionId(1);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection,
            uuid: uuid::Uuid::from_u128(1),
            name: "AllocatorSleeper".into(),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(connection).unwrap();
    let home = bed(&mut state, 8, 8);
    state.world.fold_overlay();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(
        slot,
        [
            f64::from(home.x) + 0.5,
            f64::from(home.y),
            f64::from(home.z) - 1.5,
        ],
    );
    let mut owner = None;
    for i in 0..ORACLE_RESIDENTS {
        let slot = villager(&mut state, [6.5, 201.0, 8.5]);
        if i == 200 {
            state.entities.row_mut(slot).villager.as_mut().unwrap().home = Some(home);
            owner = Some(state.entities.body(slot).id);
        }
    }
    let owner = owner.unwrap();
    event_validate(&mut state);
    state.day_time = 13_000;
    assert!(crate::sleep::use_bed(&mut state, player, connection, home));
    assert!(state.sleepers.contains_key(&player));
    event_validate(&mut state);
    let slot = state.entities.slot_of(owner).unwrap();
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().home,
        None
    );
    assert!(state.villager_pois.occupied_beds.contains(&home));
    crate::sleep::wake(&mut state, player);
    event_validate(&mut state);
    assert!(!state.villager_pois.occupied_beds.contains(&home));
    assert!(!state.villager_pois.claimed(
        owner,
        Poi {
            pos: home,
            kind: PoiKind::Home
        }
    ));
}

/// Structural workload: satisfied residents never enter the search indexes,
/// an unrelated empty-chunk flip adds nothing to `updates`, and a
/// resident-chunk flip adds only that chunk's residents (never all ids).
#[test]
fn event_driven_workload_stays_structural() {
    let mut state = test_state();
    let want = rustc_hash::FxHashMap::from_iter(
        [(0, 0), (1, 0)]
            .into_iter()
            .map(|(x, z)| (ChunkPos { x, z }, 0)),
    );
    state.world.ensure_blocking(&want);
    let stone = state.registry.states.stone;
    let air = state.registry.states.air;
    for lx in 0..16 {
        for lz in 0..16 {
            for (cx, cz) in [(1, 0)] {
                let (x, z) = (cx * 16 + lx, cz * 16 + lz);
                state.world.set_block(x, 200, z, stone);
                for y in 201..206 {
                    state.world.set_block(x, y, z, air);
                }
            }
        }
    }
    state.world.fold_overlay();
    state
        .tickets
        .insert("workload-fixture".into(), (ChunkPos { x: 0, z: 0 }, 1));
    let tick = state.tick;
    state.refresh_active_chunks(tick);

    // Sixty fully satisfied adults: unique bed + composter + shared bell.
    let composter = block(&state, "minecraft:composter", &[]);
    let bell_block = block(&state, "minecraft:bell", &[]);
    state.world.set_block(8, 202, 0, bell_block);
    state.world.fold_overlay();
    let meeting = BlockPos { x: 8, y: 202, z: 0 };
    let mut beds = Vec::new();
    for row in 0..10 {
        for hx in [2, 4, 6, 8, 10, 12] {
            beds.push(bed(&mut state, hx, 2 + row));
            if beds.len() >= 60 {
                break;
            }
        }
    }
    let mut jobs = Vec::new();
    for k in 0..60 {
        let (x, z) = (16 + (k % 15), 1 + (k / 15));
        state.world.set_block(x, 201, z, composter);
        jobs.push(BlockPos { x, y: 201, z });
    }
    state.world.fold_overlay();
    let mut ids = Vec::new();
    for i in 0..60 {
        let (cx, cz) = if i < 30 { (0, 0) } else { (1, 0) };
        let slot = villager(
            &mut state,
            [
                f64::from(cx * 16 + 8) + 0.5,
                201.0,
                f64::from(cz * 16 + 8) + 0.5,
            ],
        );
        ids.push(state.entities.body(slot).id);
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.age = 0;
        v.home = Some(beds[i]);
        v.home_dimension = OVERWORLD.into();
        v.job_site = Some(jobs[i]);
        v.job_site_dimension = OVERWORLD.into();
        v.data.profession = Profession::Farmer;
        v.meeting_point = Some(meeting);
        v.meeting_point_dimension = OVERWORLD.into();
    }
    event_validate(&mut state);

    // Satisfied residents hold no search demand in any ready structure.
    let cache = &state.villager_pois;
    assert!(
        cache.search_needs.is_empty(),
        "satisfied residents must not enter search needs"
    );
    assert!(
        cache.search_due.is_empty(),
        "satisfied residents must not enter search due"
    );
    assert!(
        cache.search_ready.is_empty(),
        "satisfied residents must not enter the ready queue"
    );
    assert_eq!(cache.ready_demand, 0);
    for id in &ids {
        assert!(!cache.search_needs.contains_key(id));
        assert!(!cache.search_ready_order.contains_key(id));
    }

    // Unrelated empty-chunk flip: touching (5,5) activity adds no residents.
    let mut active: rustc_hash::FxHashSet<ChunkPos> = state.world.active_positions().collect();
    active.insert(ChunkPos { x: 5, z: 5 });
    let tick = state.tick;
    state.world.set_active_chunks(active, tick);
    let mut cache = std::mem::take(&mut state.villager_pois);
    cache.changed.clear();
    state.entities.swap_brain_changes(&mut cache.changed);
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(&state);
    assert!(
        cache.updates.is_empty(),
        "empty-chunk flip must not queue resident updates"
    );
    state.villager_pois = cache;
    let mut active: rustc_hash::FxHashSet<ChunkPos> = state.world.active_positions().collect();
    active.remove(&ChunkPos { x: 5, z: 5 });
    let tick = state.tick;
    state.world.set_active_chunks(active, tick);

    // Resident-chunk flip: darkening (1,0) queues only its own residents.
    let mut cache = std::mem::take(&mut state.villager_pois);
    cache.changed.clear();
    state.entities.swap_brain_changes(&mut cache.changed);
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(&state);
    cache.updates.clear();
    state.villager_pois = cache;
    let mut active: rustc_hash::FxHashSet<ChunkPos> = state.world.active_positions().collect();
    assert!(active.remove(&ChunkPos { x: 1, z: 0 }));
    let tick = state.tick;
    state.world.set_active_chunks(active, tick);
    let mut cache = std::mem::take(&mut state.villager_pois);
    cache.changed.clear();
    state.entities.swap_brain_changes(&mut cache.changed);
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(&state);
    assert!(
        cache.updates.len() < ids.len(),
        "chunk flip must not queue every resident"
    );
    assert_eq!(cache.updates.len(), 30);
    let updated: rustc_hash::FxHashSet<EntityId> = cache.updates.iter().copied().collect();
    assert_eq!(updated.len(), 30, "flipped chunk queues each resident once");
    for id in &ids[30..] {
        assert!(updated.contains(id), "flipped chunk resident missing");
    }
    for id in &ids[..30] {
        assert!(!updated.contains(id), "unrelated resident queued");
    }
    state.villager_pois = cache;
}

#[test]
fn lower_id_claim_dirties_existing_owner_without_a_block_revision() {
    let mut state = test_state();
    let home = bed(&mut state, 8, 8);
    state.world.fold_overlay();
    let ids: Vec<_> = (0..ORACLE_RESIDENTS)
        .map(|_| {
            let slot = villager(&mut state, [6.5, 201.0, 8.5]);
            state.entities.body(slot).id
        })
        .collect();
    let previous = state.entities.slot_of(ids[200]).unwrap();
    state
        .entities
        .row_mut(previous)
        .villager
        .as_mut()
        .unwrap()
        .home = Some(home);
    event_validate(&mut state);
    let winner = state.entities.slot_of(ids[50]).unwrap();
    state
        .entities
        .row_mut(winner)
        .villager
        .as_mut()
        .unwrap()
        .home = Some(home);
    event_validate(&mut state);
    assert_eq!(
        state.entities.row(winner).villager.as_ref().unwrap().home,
        Some(home)
    );
    assert_eq!(
        state.entities.row(previous).villager.as_ref().unwrap().home,
        None
    );
    assert_eq!(state.villager_pois.homes[&home].len(), 1);
    assert!(state.villager_pois.homes[&home].contains(&ids[50]));
}

#[test]
fn mixed_removal_slot_fixups_match_full_roster_rebuild_every_tick() {
    let mut event = build_oracle(32);
    let mut oracle = build_oracle(32);
    event_validate(&mut event.state);
    oracle_validate(&mut oracle.state);
    compare_oracle(&event.state, &oracle.state, "mixed hydration");
    for tick in 0..32 {
        for world in [&mut event, &mut oracle] {
            world.state.tick = tick as u64 + 1;
            let id = world.prefix_mobs[tick];
            let slot = world.state.entities.slot_of(id).unwrap();
            world.state.remove_entity(slot);
        }
        // Reference deliberately rebuilds dense slots and admission ranks.
        oracle.state.villager_pois.resident_revision = None;
        event_validate(&mut event.state);
        oracle_validate(&mut oracle.state);
        assert_eq!(event.state.villager_pois.roster_rebuilds, 0);
        assert_eq!(oracle.state.villager_pois.roster_rebuilds, 1);
        compare_oracle(&event.state, &oracle.state, "mixed slot fixup");
        let cached = &event.state.villager_pois;
        let reference = &oracle.state.villager_pois;
        for &id in &event.ids {
            let index = cached.resident_index[&id];
            assert_eq!(event.state.entities.body(cached.residents[index]).id, id);
            assert!(
                cached.resident_data[index]
                    == reference.resident_data[reference.resident_index[&id]],
                "cached brain differs after mixed slot fixup for {id:?}"
            );
        }
    }
}
