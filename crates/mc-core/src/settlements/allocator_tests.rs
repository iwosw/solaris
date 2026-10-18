//! Native allocator regression gates: the S1 housing/POI behavior that the
//! allocator now owns as sole authority (replacing the brain-local gates).
use super::*;
use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::registry::CoreRegistry;
use crate::tick::TickState;
use crate::villager::{VillagerProfession as Profession, VillagerState};
use std::sync::Arc;

pub(super) fn test_state() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(
        mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks)),
    );
    let mut world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 201, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let want = rustc_hash::FxHashMap::from_iter([(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]);
    world.ensure_blocking(&want);
    for x in 0..16 {
        for z in 0..16 {
            world.set_block(x, 200, z, registry.states.stone);
            for y in 201..206 {
                world.set_block(x, y, z, registry.states.air);
            }
        }
    }
    world.fold_overlay();
    let mut state = TickState::new(registry, world, 2, 512);
    state.tickets.insert(
        "allocator-fixture".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.refresh_active_chunks(0);
    state
}
pub(super) fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(name).unwrap();
    let default = state.registry.blocks.block(&id).unwrap().default;
    let mut full = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (key, value) in props {
        if let Some(entry) = full.iter_mut().find(|(k, _)| k == key) {
            entry.1 = (*value).to_owned();
        }
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &full)
        .unwrap()
        .0
}
pub(super) fn villager(state: &mut TickState, pos: [f64; 3]) -> EntitySlot {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).villager = Some(Box::new(VillagerState::default()));
    slot
}
pub(super) fn bed(state: &mut TickState, x: i32, z: i32) -> BlockPos {
    let foot = block(
        state,
        "minecraft:red_bed",
        &[("part", "foot"), ("facing", "east")],
    );
    let head = block(
        state,
        "minecraft:red_bed",
        &[("part", "head"), ("facing", "east")],
    );
    state.world.set_block(x - 1, 201, z, foot);
    state.world.set_block(x, 201, z, head);
    BlockPos { x, y: 201, z }
}
fn step(state: &mut TickState) -> ChangeSet {
    state.tick_budget.reset(state.tick);
    let mut changes = ChangeSet::default();
    tick(state, &mut changes);
    crate::villager::brain::tick(state, &mut changes);
    state.tick += 1;
    changes
}

#[test]
fn allocator_assigns_home_job_and_meeting_point_as_sole_authority() {
    let mut state = test_state();
    state.day_time = 2000;
    let home = bed(&mut state, 8, 8);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    let bell = block(&state, "minecraft:bell", &[]);
    state.world.set_block(8, 201, 10, bell);
    state.world.fold_overlay();
    let slot = villager(&mut state, [2.5, 201.0, 2.5]);
    for _ in 0..60 {
        step(&mut state);
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.home, Some(home));
    assert_eq!(v.job_site, Some(BlockPos { x: 4, y: 201, z: 4 }));
    assert_eq!(
        v.meeting_point,
        Some(BlockPos {
            x: 8,
            y: 201,
            z: 10
        })
    );
    assert_eq!(v.data.profession, Profession::Farmer);
    assert_eq!(v.home_dimension, "minecraft:overworld");
    assert_eq!(v.job_site_dimension, "minecraft:overworld");
    assert_eq!(v.meeting_point_dimension, "minecraft:overworld");
}

#[test]
fn allocator_never_loads_chunks_for_unloaded_claims() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let before = state.world.chunks().count();
    for _ in 0..40 {
        step(&mut state);
    }
    assert_eq!(state.world.chunks().count(), before);
    // An unloaded local cell is not evidence that a retained claim vanished.
    let unloaded = BlockPos {
        x: 1600,
        y: 201,
        z: 1600,
    };
    state.entities.row_mut(slot).villager.as_mut().unwrap().home = Some(unloaded);
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    let id = state.entities.body(slot).id;
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    validate_pois(&state, &cache, id, &mut v);
    state.entities.row_mut(slot).villager = Some(v);
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().home,
        Some(unloaded)
    );
    assert_eq!(state.world.chunks().count(), before);
}

#[test]
fn allocator_revalidates_destroyed_claims_and_releases_traded_profession() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let home = BlockPos { x: 9, y: 201, z: 8 };
    let mut v = VillagerState::default();
    v.data.profession = Profession::Farmer;
    v.job_site = Some(home);
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    validate_pois(&state, &cache, state.entities.body(slot).id, &mut v);
    assert_eq!(v.job_site, None);
    assert_eq!(v.data.profession, Profession::None);
    v.data.profession = Profession::Farmer;
    v.job_site = Some(home);
    v.xp = 1;
    validate_pois(&state, &cache, state.entities.body(slot).id, &mut v);
    assert_eq!(v.job_site, None);
    assert_eq!(v.data.profession, Profession::Farmer);
}

#[test]
fn allocator_keeps_every_body_executing_under_budget_pressure() {
    let mut state = test_state();
    state.day_time = 3000;
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(8, 201, 8, composter);
    state.world.fold_overlay();
    let first = villager(&mut state, [7.5, 201.0, 8.5]);
    let second = villager(&mut state, [9.5, 201.0, 8.5]);
    // Adult bodies with nonzero age tick their life clock every brain pass.
    for slot in [first, second] {
        state.entities.row_mut(slot).villager.as_mut().unwrap().age = 100;
    }
    // Exhaust the whole shared allowance before the pass: no admission, but
    // both bodies still run their brain phase every tick.
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::Decision);
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::PathNode);
    let mut changes = ChangeSet::default();
    tick(&mut state, &mut changes);
    crate::villager::brain::tick(&mut state, &mut changes);
    for slot in [first, second] {
        let v = state.entities.row(slot).villager.as_ref().unwrap();
        assert!(v.job_site.is_none());
        assert_eq!(
            v.age, 99,
            "every active body executes its life clock each tick, even with no allowance"
        );
    }
}

#[test]
fn allocator_shares_one_pool_across_dimensions_without_local_reset() {
    let mut state = test_state();
    let mut sibling = test_state();
    sibling.dimension = crate::dimensions::Dimension::Nether;
    state.tick_budget.reset(11);
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::Decision);
    // Swap the globals into a sibling world: the sibling pass in the same
    // global tick observes the same exhausted pool (no refill on entry).
    crate::dimensions::swap_globals(&mut state, &mut sibling);
    assert!(
        !sibling
            .tick_budget
            .try_charge(sibling.dimension, Consumer::Allocator, Kind::Decision, 1),
        "a second dimension in the same global tick must not refill the pool"
    );
    crate::dimensions::swap_globals(&mut state, &mut sibling);
    assert!(
        !state
            .tick_budget
            .try_charge(state.dimension, Consumer::Allocator, Kind::Decision, 1)
    );
    state.tick_budget.reset(11);
    assert!(
        !state
            .tick_budget
            .try_charge(state.dimension, Consumer::Allocator, Kind::Decision, 1)
    );
    state.tick_budget.reset(12);
    assert!(
        state
            .tick_budget
            .remaining(crate::tick_budget::Kind::Decision)
            > 0
    );
}

#[test]
fn allocator_route_refusal_retries_on_next_pass() {
    let mut state = test_state();
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(10, 201, 8, composter);
    state.world.fold_overlay();
    let slot = villager(&mut state, [2.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    state.villager_pois.search_at.insert(id, state.tick);
    state.tick_budget.reset(state.tick);
    state
        .tick_budget
        .exhaust_for_test(crate::tick_budget::Kind::PathNode);
    let mut changes = ChangeSet::default();
    tick(&mut state, &mut changes);
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .job_site
            .is_none()
    );
    assert!(state.villager_pois.search_deferred);
    let deadline = state.villager_pois.search_at[&id];
    // Next global tick refills the pool; the refused route is retried.
    state.tick += 1;
    state.tick_budget.reset(state.tick);
    tick(&mut state, &mut changes);
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(BlockPos {
            x: 10,
            y: 201,
            z: 8
        })
    );
    assert!(state.villager_pois.search_at[&id] > deadline);
}

#[test]
fn allocator_rebuilds_member_counts_from_live_columns() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let mut sequence = state.settlement_sequence;
    let id = state
        .settlements
        .create(
            &mut sequence,
            "town".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    state.settlement_sequence = sequence;
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(id);
    let mut changes = ChangeSet::default();
    tick(&mut state, &mut changes);
    assert_eq!(state.settlements.member_count(id), 1);
    state.entities.body_mut(slot).alive = false;
    tick(&mut state, &mut changes);
    assert_eq!(state.settlements.member_count(id), 0);
}

#[test]
fn allocator_scopes_acquisition_to_bound_settlement_radius() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    // In-settlement workstation vs a closer out-of-settlement one: the bound
    // villager must skip the closer POI and claim inside its radius.
    let far = block(&state, "minecraft:composter", &[]);
    state.world.set_block(9, 201, 8, far);
    let near = block(&state, "minecraft:composter", &[]);
    state.world.set_block(14, 201, 8, near);
    state.world.fold_overlay();
    let mut sequence = state.settlement_sequence;
    let id = state
        .settlements
        .create(
            &mut sequence,
            "town".into(),
            "owner".into(),
            [14, 201, 8],
            4,
        )
        .unwrap();
    state.settlement_sequence = sequence;
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(id);
    for _ in 0..60 {
        step(&mut state);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(BlockPos {
            x: 14,
            y: 201,
            z: 8
        }),
        "bound villagers claim inside their anchor radius, not the closer foreign POI"
    );
    // Membership survives the body standing outside its radius.
    state.entities.set_pos(slot, [1000.5, 201.0, 1000.5]);
    for _ in 0..5 {
        step(&mut state);
    }
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        Some(id),
        "moving out of radius never clears membership"
    );
    // Unbound villagers keep vanilla reach: the closer POI wins.
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = None;
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .job_site = None;
    state.entities.set_pos(slot, [8.5, 201.0, 8.5]);
    state
        .villager_pois
        .search_at
        .insert(state.entities.body(slot).id, state.tick);
    for _ in 0..60 {
        step(&mut state);
    }
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().job_site,
        Some(BlockPos { x: 9, y: 201, z: 8 }),
        "unbound villagers keep vanilla nearby acquisition"
    );
}

#[test]
fn allocator_denies_acquisition_for_foreign_dimension_membership() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(9, 201, 8, composter);
    state.world.fold_overlay();
    // Record lives in another dimension's registry, not the local one.
    let mut other = test_state();
    other.dimension = crate::dimensions::Dimension::Nether;
    let mut sequence = other.settlement_sequence;
    let id = other
        .settlements
        .create(
            &mut sequence,
            "nether-town".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(id);
    for _ in 0..60 {
        step(&mut state);
    }
    assert!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .job_site
            .is_none(),
        "foreign-dimension membership must not acquire local POIs"
    );
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        Some(id),
        "foreign membership column is retained, not cleared"
    );
}

#[test]
fn admitted_brain_path_does_not_pay_a_second_decision() {
    let mut state = test_state();
    state.tick_budget.exhaust_for_test(Kind::Decision);
    let nodes = state.tick_budget.remaining(Kind::PathNode);
    let mut cache = PoiCache::default();
    let mut path = Vec::new();
    let mut ctx = RouteCtx::tick(&mut state, Consumer::Brain);
    assert!(
        find_path_exact_ctx(
            &mut ctx,
            &mut cache,
            [2.5, 201.0, 8.5],
            BlockPos {
                x: 10,
                y: 201,
                z: 8
            },
            0.3,
            1.95,
            &mut path,
        ),
        "full brain admission already paid its decision; only A* nodes charge here"
    );
    assert!(!path.is_empty());
    assert_eq!(state.tick_budget.remaining(Kind::Decision), 0);
    assert!(state.tick_budget.remaining(Kind::PathNode) < nodes);
}

#[test]
fn allocator_noop_validation_and_empty_search_leave_journal_rows_quiet() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    state.entities.take_touched();
    validate_changed_claims(&mut state, &mut cache);
    assert!(
        state.entities.take_touched().is_empty(),
        "unchanged validation must not take row_mut"
    );
    cache.search_at.insert(id, state.tick);
    acquire_pois(&mut state, &mut cache);
    assert!(
        state.entities.take_touched().is_empty(),
        "empty search must not take row_mut"
    );
}

#[test]
fn allocator_in_place_validation_covers_collision_and_destruction() {
    let mut state = test_state();
    let home = bed(&mut state, 8, 8);
    let composter = block(&state, "minecraft:composter", &[]);
    state.world.set_block(4, 201, 4, composter);
    state.world.fold_overlay();
    let me = villager(&mut state, [8.5, 201.0, 8.5]);
    let rival = villager(&mut state, [9.5, 201.0, 8.5]);
    let rival_id = state.entities.body(rival).id;
    // Seed identical claims, then index one owner as the claim winner.
    let mut cache = PoiCache::default();
    cache.update_claims(
        state.entities.body(me).id,
        [None; 3],
        [Some(home), Some(BlockPos { x: 4, y: 201, z: 4 }), None],
    );
    cache.refresh(&state);
    for slot in [me, rival] {
        let v = state
            .entities
            .row_mut_unmarked(slot)
            .villager
            .as_mut()
            .unwrap();
        v.home = Some(home);
        v.job_site = Some(BlockPos { x: 4, y: 201, z: 4 });
        v.data.profession = Profession::Farmer;
    }
    state.entities.take_touched();
    // The indexed owner validates cache-only (no mark); the colliding rival
    // releases same-tick and marks its row alone.
    validate_changed_claims(&mut state, &mut cache);
    assert_eq!(state.entities.take_touched(), vec![rival_id]);
    let mine = state.entities.row(me).villager.as_ref().unwrap();
    assert_eq!(mine.home, Some(home));
    assert_eq!(mine.job_site, Some(BlockPos { x: 4, y: 201, z: 4 }));
    // Collision: the rival releases same-tick and the winner keeps custody.
    assert_eq!(
        state.entities.row(rival).villager.as_ref().unwrap().home,
        None
    );
    assert!(
        !cache.claimed(
            state.entities.body(me).id,
            Poi {
                pos: home,
                kind: PoiKind::Home
            }
        ),
        "winner keeps its claim"
    );
    // Destroyed bed/workstation invalidates; traded profession survives.
    state.entities.row_mut(me).villager.as_mut().unwrap().xp = 1;
    state
        .world
        .set_block(home.x, home.y, home.z, state.registry.states.stone);
    state
        .world
        .set_block(4, 201, 4, state.registry.states.stone);
    state.world.fold_overlay();
    cache.refresh(&state);
    state.entities.take_touched();
    validate_changed_claims(&mut state, &mut cache);
    let v = state.entities.row(me).villager.as_ref().unwrap();
    assert_eq!(v.home, None);
    assert_eq!(v.job_site, None);
    assert_eq!(v.data.profession, Profession::Farmer);
    assert!(!state.entities.take_touched().is_empty());
}

#[test]
fn unrelated_activity_change_does_not_refresh_every_resident() {
    let mut state = test_state();
    let slot = villager(&mut state, [8.5, 201.0, 8.5]);
    let id = state.entities.body(slot).id;
    let mut cache = PoiCache::default();
    cache.refresh(&state);
    let mut changed = FxHashSet::default();
    state.entities.swap_brain_changes(&mut changed);
    let unrelated = ChunkPos { x: 100, z: 100 };
    state.world.set_active_chunks(
        [ChunkPos { x: 0, z: 0 }, unrelated].into_iter().collect(),
        state.tick + 1,
    );
    cache.refresh(&state);
    assert!(
        !cache.updates.contains(&id),
        "a global activity fence must not dirty residents of unchanged chunks"
    );
}
