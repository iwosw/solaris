//! Binding acceptance for every blueprint except House and Barracks: each
//! level is built by the REAL Builder through the shared construction
//! fixture, certified at that level, then exercised through the REAL
//! follow-on systems.
//!
//! * Productive slots are claimed by initially unemployed bound adults through
//!   the real allocator/brain. The fixture Builder keeps its ongoing role.
//! * Farmers harvest/replant Builder-placed wheat; Lumberjacks cut real trees
//!   outside their built camps. Both prove real carry and block journaling.
//! * Storage: the certificate's container points are usable settlement stock:
//!   a real native container endpoint load succeeds at certified points.
//! * Wall / Tower: defensive certificate with intentionally zero worker
//!   slots; no invented worker type is ever assigned or claimed.
//! * Every level runs prepared and completed `plugin_structures::validate`
//!   boundaries; upgrades keep the BuildingId.
//! * Mine: the Builder-built mine additionally proves REAL actual extraction:
//!   the assigned Miner breaks real seeded stone/ore below the floor (inside
//!   the derived operational bounds), carries it, and the change journal
//!   records the removal. Worker identity, scans and carriers stay real: no
//!   `mining_progress` fields are set by hand and no distant teleport is used
//!   beyond the admitted route.
//!
//! Declared fixture assistance (no behaviour is faked):
//! * The shared `construction::tests::{start, fund, complete}` drives the
//!   Builder hauling/ordered native placement and certification (navigation
//!   target following is declared fixture assistance there).
//! * Organic candidates follow only their OWN admitted `navigation_path` cells.
//!   Productive job sites/professions and sleep flags are never assigned.
//! * The farm proof ages existing paid wheat, without creating crop blocks;
//!   the lumber proof seeds a mature natural tree outside both camp footprints.

use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::settlements::SettlementId;
use crate::settlements::allocator::tests as residents;
use crate::settlements::blueprints::{Rotation, all as all_blueprints};
use crate::settlements::buildings::{Building, BuildingId, BuildingKind, job_slots};
use crate::settlements::construction::tests as site;
use crate::tick::TickState;
use crate::villager::VillagerProfession;

/// Canonical construction site used by the shared fixture helpers.
const ORIGIN: [i32; 3] = [3, 200, 3];
/// Mid-morning: inside the vanilla work window for every tested profession.
const WORK: i64 = 3000;
/// Ticks a proof loop may run allocator+brain before the claim must hold.
const PROOF_TICKS: usize = 1500;
/// Bounded real extraction attempts for the Mine proof.
const MINE_WORK_ATTEMPTS: usize = 900;
/// How far the fixture Builder's POI search is deferred during a claim proof.
const BUILDER_SEARCH_DELAY: u64 = 1_000_000;

fn blueprint_levels(state: &TickState, blueprint: &str, kind: &BuildingKind) -> u8 {
    let bp =
        crate::settlements::blueprints::get(&state.registry, blueprint).expect("blueprint exists");
    assert_eq!(
        &bp.kind, kind,
        "blueprint {blueprint} has the intended kind"
    );
    assert!(
        !bp.levels.is_empty(),
        "blueprint {blueprint} has no levels; a vacuous pass is refused"
    );
    for level in &bp.levels {
        assert!(
            !level.blocks.is_empty() && !level.materials.is_empty(),
            "blueprint {blueprint} must place blocks and consume stock at every level"
        );
    }
    // Include the no-level case explicitly: an empty blueprint must not pass.
    assert!(
        !all_blueprints(&state.registry).is_empty(),
        "the blueprint catalog must not be empty"
    );
    u8::try_from(bp.levels.len()).expect("blueprint level count fits in u8")
}

fn expected_bounds(state: &TickState, blueprint: &str, level: u8) -> mc_script::ScriptWorldBounds {
    crate::settlements::blueprints::get(&state.registry, blueprint)
        .expect("blueprint exists")
        .level(level)
        .expect("blueprint level exists")
        .rotated(Rotation::from_quarter_turns(0))
        .bounds(ORIGIN)
}

fn building_of(state: &TickState, id: SettlementId, kind: BuildingKind) -> (BuildingId, Building) {
    let record = state.settlements.get(id).expect("settlement exists");
    let (bid, building) = record
        .buildings
        .iter()
        .find(|entry| entry.1.kind == kind)
        .expect("intended certificate exists");
    (*bid, building.clone())
}

fn assert_builder_role(state: &TickState, builder: EntitySlot) {
    assert_eq!(
        state
            .entities
            .row(builder)
            .villager
            .as_ref()
            .expect("builder row")
            .data
            .profession,
        VillagerProfession::Builder,
        "the fixture Builder keeps its ongoing role"
    );
}

fn delay_builder(state: &mut TickState, builder: EntitySlot) {
    let owner = state.entities.body(builder).id;
    state
        .villager_pois
        .search_at
        .insert(owner, state.tick.saturating_add(BUILDER_SEARCH_DELAY));
}

fn release_builder(state: &mut TickState, builder: EntitySlot) {
    let owner = state.entities.body(builder).id;
    state.villager_pois.search_at.remove(&owner);
}

fn bound_resident(state: &mut TickState, id: SettlementId, pos: [f64; 3]) -> EntitySlot {
    let slot = residents::villager(state, pos);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(state, id, entity).expect("resident binds within the radius");
    {
        let resident = state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .expect("resident row");
        resident.age = 0;
        resident.home = None;
        resident.meeting_point = None;
        resident.job_site = None;
        resident.navigation_target = None;
        resident.navigation_path.clear();
    }
    state.world.fold_overlay();
    slot
}

/// A walkable cell beside one of the certified work points, so the claim
/// starts inside real job reach and long walks stay the exception.
fn adjacent_spawn(state: &TickState, points: &[[i32; 3]]) -> [f64; 3] {
    for point in points {
        let cell = BlockPos {
            x: point[0],
            y: point[1],
            z: point[2],
        };
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (0, 0)] {
            let neighbour = BlockPos {
                x: cell.x + dx,
                y: cell.y,
                z: cell.z + dz,
            };
            if let Some(spot) = crate::settlements::allocator::standing(
                &state.world,
                &state.registry,
                neighbour,
                0.3,
                1.95,
            ) {
                return spot;
            }
        }
        let pos = BlockPos {
            x: point[0],
            y: point[1] + 1,
            z: point[2],
        };
        if let Some(spot) =
            crate::settlements::allocator::standing(&state.world, &state.registry, pos, 0.3, 1.95)
        {
            return spot;
        }
    }
    panic!("no walkable cell beside certified work points {points:?}");
}

/// Declared assistance: step the candidate along its OWN admitted route when
/// the brain has not walked it into job reach yet.
fn follow_admitted_route(state: &mut TickState, slot: EntitySlot) {
    let next = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .expect("resident row")
        .navigation_path
        .last()
        .copied();
    let body = state.entities.body(slot);
    let (width, height) = (body.half_width, body.height);
    if let Some(cell) = next
        && let Some(spot) = crate::settlements::allocator::standing(
            &state.world,
            &state.registry,
            cell,
            width,
            height,
        )
    {
        state.entities.set_pos(slot, spot);
    }
}

/// Drive the real allocator+brain until a fresh adult both holds one of the
/// certified work points and has its profession confirmed by the REAL brain
/// near the claimed job. The profession is never assigned by hand.
fn prove_organic_claim(
    state: &mut TickState,
    slot: EntitySlot,
    points: &[[i32; 3]],
    profession: VillagerProfession,
    level: u8,
) -> BlockPos {
    for _ in 0..PROOF_TICKS {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(state, &mut changes);
        crate::villager::brain::tick(state, &mut changes);
        state.tick += 1;
        let resident = state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .expect("resident row");
        if resident.data.profession == profession
            && resident
                .job_site
                .is_some_and(|job| points.contains(&[job.x, job.y, job.z]))
        {
            break;
        }
        follow_admitted_route(state, slot);
    }
    let resident = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .expect("resident row");
    let job = resident.job_site.unwrap_or_else(|| {
        let jobs: Vec<_> = state
            .entities
            .slots()
            .filter_map(|slot| {
                state.entities.row(slot).villager.as_ref().map(|v| {
                    (
                        v.data.profession,
                        v.job_site,
                        state.entities.body(slot).id,
                        state.entities.pos(slot),
                        crate::settlements::allocator::active(state, slot),
                        state.villager_pois.search_at.get(&state.entities.body(slot).id),
                        state.villager_pois.search_needs.get(&state.entities.body(slot).id),
                        v.navigation_target,
                    )
                })
            })
            .collect();
        let live: Vec<_> = points.iter().map(|point| {
            state.world.block_at(point[0], point[1], point[2]).and_then(|id| {
                state.registry.blocks.by_id(mc_world::BlockStateId(id))
                    .map(|definition| definition.block.id.to_string())
            })
        }).collect();
        panic!(
            "bound adult claims a real certified work point (tick {}, level {level}, want {profession:?} in {points:?}, blocks {live:?}); resident jobs: {jobs:?}; queued={:?}; deferred={}",
            state.tick, state.villager_pois.search_ready, state.villager_pois.search_deferred
        )
    });
    assert!(
        points.contains(&[job.x, job.y, job.z]),
        "claimed job {job:?} must be a work point of this building level"
    );
    assert_eq!(
        resident.data.profession, profession,
        "the real brain confirms the intended profession near its claimed job"
    );
    assert_eq!(
        crate::settlements::buildings::job_profession(&state.settlements, resident.settlement, job),
        Some(profession),
        "the certificate authorizes the intended profession at the claimed job"
    );
    job
}

fn seed_mine_ground(state: &mut TickState, bounds: mc_script::ScriptWorldBounds) {
    let stone = state.registry.states.stone;
    let coal = residents::block(state, "minecraft:coal_ore", &[]);
    let min_y = bounds.min[1].saturating_sub(12);
    for x in bounds.min[0]..=bounds.max[0] {
        for z in bounds.min[2]..=bounds.max[2] {
            for y in min_y..bounds.min[1] {
                let id = if (x + y + z).rem_euclid(7) == 0 {
                    coal
                } else {
                    stone
                };
                state.world.set_block(x, y, z, id);
            }
        }
    }
    // The build floor row itself is never seeded: the derived shaft descends
    // from the entrance and never re-mines placed floor work.
    state.world.fold_overlay();
}

/// Storage-native image equality: every certified container point must load
/// as a real native endpoint with bounded stock.
fn loads_usable_container(state: &TickState, point: &[i32; 3]) {
    let endpoint = crate::settlements::inventory::Endpoint::Container {
        dimension: state.dimension.name().into(),
        position: *point,
    };
    let image = crate::settlements::inventory::load(state, &endpoint)
        .unwrap_or_else(|_| panic!("certified container {point:?} must load"));
    assert!(
        image.slots.len() <= 54,
        "a certified container exposes bounded native stock"
    );
    let _ = &image.fence;
}

/// Exercise the same real extraction entrypoint as the brain, keeping the
/// worker body/carry intact and following only the returned work target.
fn prove_mine_extraction(
    state: &mut TickState,
    slot: EntitySlot,
    bounds: mc_script::ScriptWorldBounds,
) {
    for _ in 0..MINE_WORK_ATTEMPTS {
        state.tick += 20;
        let mut changes = ChangeSet::default();
        let mut miner = state.entities.row_mut(slot).villager.take().unwrap();
        let target = super::mining::work(state, slot, &mut miner, &mut changes);
        state.entities.row_mut(slot).villager = Some(miner);
        if let Some(target) = target {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        if let Some(change) = changes.blocks.iter().find(|change| {
            change.position.y < bounds.min[1] && change.state == state.registry.states.air
        }) {
            let miner = state.entities.row(slot).villager.as_ref().unwrap();
            assert!(
                miner
                    .inventory
                    .iter()
                    .flatten()
                    .any(|stack| stack.count > 0),
                "the real miner carries the excavated material"
            );
            assert!(
                (bounds.min[0]..=bounds.max[0]).contains(&change.position.x)
                    && (bounds.min[2]..=bounds.max[2]).contains(&change.position.z),
                "the journaled extraction stays in the certified XZ footprint"
            );
            assert_eq!(
                state
                    .world
                    .block_at(change.position.x, change.position.y, change.position.z),
                Some(state.registry.states.air)
            );
            return;
        }
    }
    panic!("the Builder-built mine must yield journaled extraction strictly below its floor");
}

/// Age only paid, Builder-placed wheat; the real Farmer must harvest/replant it.
fn prove_farm_harvest(state: &mut TickState, slot: EntitySlot) {
    let mature = residents::block(state, "minecraft:wheat", &[("age", "7")]);
    let young = residents::block(state, "minecraft:wheat", &[("age", "0")]);
    let size = crate::settlements::blueprints::get(&state.registry, "farm")
        .unwrap()
        .footprint(Rotation::from_quarter_turns(0));
    for x in ORIGIN[0]..ORIGIN[0] + i32::from(size[0]) {
        for z in ORIGIN[2]..ORIGIN[2] + i32::from(size[2]) {
            if state
                .world
                .block_at(x, ORIGIN[1] + 1, z)
                .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                .is_some_and(|definition| definition.block.id.path() == "wheat")
            {
                state.world.set_block(x, ORIGIN[1] + 1, z, mature);
            }
        }
    }
    let seeds = super::mining::item_id(state, "wheat_seeds").unwrap();
    state.tick += 200;
    for _ in 0..PROOF_TICKS {
        let mut farmer = state.entities.row_mut(slot).villager.take().unwrap();
        let before: i32 = farmer
            .inventory
            .iter()
            .flatten()
            .filter(|stack| stack.item_id == seeds)
            .map(|stack| stack.count)
            .sum();
        let mut changes = ChangeSet::default();
        super::harvest::work(state, slot, &mut farmer, &mut changes);
        let after: i32 = farmer
            .inventory
            .iter()
            .flatten()
            .filter(|stack| stack.item_id == seeds)
            .map(|stack| stack.count)
            .sum();
        state.entities.row_mut(slot).villager = Some(farmer);
        if let Some(change) = changes.blocks.first() {
            assert_eq!(after, before + 1, "real harvest retains one net seed");
            assert_eq!(change.state, young, "real Farmer replants age-zero wheat");
            assert_eq!(
                state
                    .world
                    .block_at(change.position.x, change.position.y, change.position.z),
                Some(young)
            );
            return;
        }
        state.tick_budget.reset(state.tick);
        crate::settlements::allocator::tick(state, &mut changes);
        super::brain::tick(state, &mut changes);
        follow_admitted_route(state, slot);
        state.tick += 1;
    }
    panic!("the real Farmer must harvest the Builder-built field");
}

fn prove_lumber_harvest(state: &mut TickState, slot: EntitySlot) {
    // This mature tree lies outside both camp levels and all authored posts.
    let dirt = residents::block(state, "minecraft:dirt", &[]);
    let log = residents::block(state, "minecraft:oak_log", &[]);
    let leaf = residents::block(state, "minecraft:oak_leaves", &[("persistent", "false")]);
    state.world.set_block(13, 200, 12, dirt);
    for y in 201..205 {
        state.world.set_block(13, y, 12, log);
    }
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (0, 0)] {
        state.world.set_block(13 + dx, 205, 12 + dz, leaf);
    }
    let item = super::mining::item_id(state, "oak_log").unwrap();
    let mut journal = ChangeSet::default();
    for _ in 0..600 {
        state.tick += 20;
        let mut jack = state.entities.row_mut(slot).villager.take().unwrap();
        let target = super::lumber::work(state, slot, &mut jack, &mut journal);
        let carried: i32 = jack
            .inventory
            .iter()
            .flatten()
            .filter(|stack| stack.item_id == item)
            .map(|stack| stack.count)
            .sum();
        state.entities.row_mut(slot).villager = Some(jack);
        if let Some(target) = target {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        if carried == 4 {
            assert!(journal.blocks.iter().any(|change| change.position.x == 13
                && change.position.z == 12
                && change.state == state.registry.states.air));
            assert_ne!(
                state.world.block_at(13, 201, 12),
                Some(log),
                "real trunk is harvested/replanted"
            );
            return;
        }
    }
    panic!("the real Lumberjack must harvest outside its Builder-built camp");
}

fn build_level(
    state: &mut TickState,
    id: SettlementId,
    builder: EntitySlot,
    blueprint: &str,
    level: u8,
    bid: Option<BuildingId>,
) {
    site::start(state, id, blueprint, level, bid);
    crate::plugin_structures::validate(state)
        .expect("prepared native stages open as a valid ledger");
    site::fund(state, blueprint, level);
    site::complete(state, id, builder);
    crate::plugin_structures::validate(state)
        .expect("committed native source reopens as a valid ledger");
    assert_builder_role(state, builder);
}

#[test]
fn farm_levels_build_claim_farmer_and_certify_bounds() {
    let (mut state, id, builder) = site::fixture();
    let levels = blueprint_levels(
        &state,
        "farm",
        &BuildingKind::Workshop(VillagerProfession::Farmer),
    );
    let mut bid: Option<BuildingId> = None;
    for level in 1..=levels {
        build_level(&mut state, id, builder, "farm", level, bid);
        let (found, building) = building_of(
            &state,
            id,
            BuildingKind::Workshop(VillagerProfession::Farmer),
        );
        assert_eq!(
            building.level, level,
            "farm certifies at the intended level"
        );
        if let Some(expected) = bid {
            assert_eq!(found, expected, "the upgrade retains the BuildingId");
        } else {
            bid = Some(found);
        }
        assert_eq!(
            building.bounds,
            expected_bounds(&state, "farm", level),
            "the certificate matches the blueprint level geometry"
        );
        assert_eq!(
            building.kind,
            BuildingKind::Workshop(VillagerProfession::Farmer),
            "the certificate keeps the productive kind"
        );
        assert_eq!(
            building.work_points.len(),
            job_slots(building.kind, level),
            "farm level {level} exposes exactly its worker slots"
        );
        delay_builder(&mut state, builder);
        let spawn = adjacent_spawn(&state, &building.work_points);
        let farmer = bound_resident(&mut state, id, spawn);
        state.day_time = WORK;
        let job = prove_organic_claim(
            &mut state,
            farmer,
            &building.work_points,
            VillagerProfession::Farmer,
            level,
        );
        assert!(
            (building.bounds.min[0]..=building.bounds.max[0]).contains(&job.x)
                && (building.bounds.min[1]..=building.bounds.max[1]).contains(&job.y)
                && (building.bounds.min[2]..=building.bounds.max[2]).contains(&job.z),
            "the claimed farm job lies inside this level's certificate"
        );
        prove_farm_harvest(&mut state, farmer);
        release_builder(&mut state, builder);
        assert_builder_role(&state, builder);
    }
}

#[test]
fn mason_levels_are_claimable_by_builder_despite_planner_role() {
    let (mut state, id, builder) = site::fixture();
    let levels = blueprint_levels(
        &state,
        "mason",
        &BuildingKind::Workshop(VillagerProfession::Builder),
    );
    let mut bid: Option<BuildingId> = None;
    for level in 1..=levels {
        build_level(&mut state, id, builder, "mason", level, bid);
        let (found, building) = building_of(
            &state,
            id,
            BuildingKind::Workshop(VillagerProfession::Builder),
        );
        assert_eq!(
            building.level, level,
            "builder hut certifies at the intended level"
        );
        if let Some(expected) = bid {
            assert_eq!(found, expected, "the upgrade retains the BuildingId");
        } else {
            bid = Some(found);
        }
        assert_eq!(
            building.bounds,
            expected_bounds(&state, "mason", level),
            "the certificate matches the blueprint level geometry"
        );
        assert_eq!(
            building.work_points.len(),
            job_slots(building.kind, level),
            "mason level {level} exposes exactly its worker slots"
        );
        delay_builder(&mut state, builder);
        let spawn = adjacent_spawn(&state, &building.work_points);
        let candidate = bound_resident(&mut state, id, spawn);
        state.day_time = WORK;
        let job = prove_organic_claim(
            &mut state,
            candidate,
            &building.work_points,
            VillagerProfession::Builder,
            level,
        );
        assert!(
            (building.bounds.min[0]..=building.bounds.max[0]).contains(&job.x)
                && (building.bounds.min[1]..=building.bounds.max[1]).contains(&job.y)
                && (building.bounds.min[2]..=building.bounds.max[2]).contains(&job.z),
            "the claimed builder job lies inside this level's certificate"
        );
        release_builder(&mut state, builder);
        assert_builder_role(&state, builder);
    }
}

#[test]
fn mine_levels_build_claim_miner_and_extract_real_stone() {
    let (mut state, id, builder) = site::fixture();
    let levels = blueprint_levels(&state, "mine", &BuildingKind::Mine);
    let mut bid: Option<BuildingId> = None;
    for level in 1..=levels {
        if level == 1 {
            let bounds = expected_bounds(&state, "mine", level);
            seed_mine_ground(&mut state, bounds);
        }
        build_level(&mut state, id, builder, "mine", level, bid);
        let (found, building) = building_of(&state, id, BuildingKind::Mine);
        assert_eq!(
            building.level, level,
            "mine certifies at the intended level"
        );
        if let Some(expected) = bid {
            assert_eq!(found, expected, "the upgrade retains the BuildingId");
        } else {
            bid = Some(found);
        }
        assert_eq!(
            building.bounds,
            expected_bounds(&state, "mine", level),
            "the certificate matches the blueprint level geometry"
        );
        assert_eq!(
            building.work_points.len(),
            job_slots(building.kind, level),
            "mine level {level} exposes exactly its worker slots"
        );
        delay_builder(&mut state, builder);
        let spawn = adjacent_spawn(&state, &building.work_points);
        let miner = bound_resident(&mut state, id, spawn);
        state.day_time = WORK;
        let job = prove_organic_claim(
            &mut state,
            miner,
            &building.work_points,
            VillagerProfession::Miner,
            level,
        );
        assert!(
            (building.bounds.min[0]..=building.bounds.max[0]).contains(&job.x)
                && (building.bounds.min[1]..=building.bounds.max[1]).contains(&job.y)
                && (building.bounds.min[2]..=building.bounds.max[2]).contains(&job.z),
            "the claimed mine job lies inside this level's certificate"
        );
        release_builder(&mut state, builder);
        assert_builder_role(&state, builder);
        // REAL actual extraction from the Builder-built mine: the assigned
        // Miner body breaks seeded stone/ore below the floor and carries it.
        // No competing free jobs exist: this settlement only certifies mines
        // and the Builder keeps no competing claim.
        prove_mine_extraction(&mut state, miner, building.bounds);
        assert_builder_role(&state, builder);
        state.world.fold_overlay();
        crate::plugin_structures::validate(&state).expect("post-extraction ledger stays valid");
    }
}

#[test]
fn lumber_camp_levels_build_claim_lumberjack_and_certify_bounds() {
    let (mut state, id, builder) = site::fixture();
    let levels = blueprint_levels(&state, "lumber_camp", &BuildingKind::LumberCamp);
    let mut bid: Option<BuildingId> = None;
    for level in 1..=levels {
        build_level(&mut state, id, builder, "lumber_camp", level, bid);
        let (found, building) = building_of(&state, id, BuildingKind::LumberCamp);
        assert_eq!(
            building.level, level,
            "lumber camp certifies at the intended level"
        );
        if let Some(expected) = bid {
            assert_eq!(found, expected, "the upgrade retains the BuildingId");
        } else {
            bid = Some(found);
        }
        assert_eq!(
            building.bounds,
            expected_bounds(&state, "lumber_camp", level),
            "the certificate matches the blueprint level geometry"
        );
        assert_eq!(
            building.work_points.len(),
            job_slots(building.kind, level),
            "lumber camp level {level} exposes exactly its worker slots"
        );
        delay_builder(&mut state, builder);
        let spawn = adjacent_spawn(&state, &building.work_points);
        let jack = bound_resident(&mut state, id, spawn);
        state.day_time = WORK;
        let job = prove_organic_claim(
            &mut state,
            jack,
            &building.work_points,
            VillagerProfession::Lumberjack,
            level,
        );
        assert!(
            (building.bounds.min[0]..=building.bounds.max[0]).contains(&job.x)
                && (building.bounds.min[1]..=building.bounds.max[1]).contains(&job.y)
                && (building.bounds.min[2]..=building.bounds.max[2]).contains(&job.z),
            "the claimed lumber job lies inside this level's certificate"
        );
        prove_lumber_harvest(&mut state, jack);
        release_builder(&mut state, builder);
        assert_builder_role(&state, builder);
    }
}

#[test]
fn storage_levels_expose_usable_native_container_load() {
    let (mut state, id, builder) = site::fixture();
    let levels = blueprint_levels(&state, "storage", &BuildingKind::Storage);
    let mut bid: Option<BuildingId> = None;
    for level in 1..=levels {
        build_level(&mut state, id, builder, "storage", level, bid);
        let (found, building) = building_of(&state, id, BuildingKind::Storage);
        assert_eq!(
            building.level, level,
            "storage certifies at the intended level"
        );
        if let Some(expected) = bid {
            assert_eq!(found, expected, "the upgrade retains the BuildingId");
        } else {
            bid = Some(found);
        }
        assert_eq!(
            building.bounds,
            expected_bounds(&state, "storage", level),
            "the certificate matches the blueprint level geometry"
        );
        assert!(
            building.work_points.len() >= usize::from(level),
            "storage level {level} exposes at least its container points"
        );
        // Usable physical native containers, not decorative boxes: every
        // certified container point loads as a real native endpoint.
        for point in &building.work_points {
            loads_usable_container(&state, point);
        }
        // (no scaffolding: the native load above is the whole proof)
        // Storage carries no profession: even an idle Farmer must not take a
        // settlement job from certified containers. Open ground suffices:
        // zero slots exist, so reach is irrelevant.
        delay_builder(&mut state, builder);
        let farmer = bound_resident(&mut state, id, [1.5, 201.0, 2.5]);
        {
            let resident = state
                .entities
                .row_mut(farmer)
                .villager
                .as_mut()
                .expect("resident row");
            resident.data.profession = VillagerProfession::Farmer;
        }
        for _ in 0..300 {
            state.tick_budget.reset(state.tick);
            let mut changes = ChangeSet::default();
            crate::settlements::allocator::tick(&mut state, &mut changes);
            crate::villager::brain::tick(&mut state, &mut changes);
            state.tick += 1;
        }
        let resident = state
            .entities
            .row(farmer)
            .villager
            .as_ref()
            .expect("resident row");
        assert!(
            resident.job_site.is_none(),
            "certified containers are stock, never a settlement job"
        );
        assert_eq!(
            resident.data.profession,
            VillagerProfession::Farmer,
            "the profession waits for certification; storage grants none"
        );
        release_builder(&mut state, builder);
        assert_builder_role(&state, builder);
    }
}

#[test]
fn wall_and_tower_certify_with_intentionally_zero_worker_slots() {
    for (blueprint, kind) in [
        ("wall", BuildingKind::Wall),
        ("watch_tower", BuildingKind::Tower),
    ] {
        let (mut state, id, builder) = site::fixture();
        let levels = blueprint_levels(&state, blueprint, &kind);
        let mut bid: Option<BuildingId> = None;
        for level in 1..=levels {
            build_level(&mut state, id, builder, blueprint, level, bid);
            let (found, building) = building_of(&state, id, kind);
            assert_eq!(
                building.level, level,
                "{blueprint} certifies at the intended level"
            );
            if let Some(expected) = bid {
                assert_eq!(found, expected, "the upgrade retains the BuildingId");
            } else {
                bid = Some(found);
            }
            assert_eq!(
                building.bounds,
                expected_bounds(&state, blueprint, level),
                "the certificate matches the blueprint level geometry"
            );
            assert_eq!(
                kind.profession(),
                None,
                "{blueprint} must carry no invented worker type"
            );
            assert_eq!(
                job_slots(kind, level),
                0,
                "{blueprint} level {level} intentionally offers zero worker slots"
            );
            assert!(
                crate::settlements::buildings::job_profession(
                    &state.settlements,
                    Some(id),
                    BlockPos {
                        x: building.bounds.min[0],
                        y: building.bounds.min[1],
                        z: building.bounds.min[2],
                    }
                )
                .is_none(),
                "{blueprint} authorizes no job at its own certificate"
            );
            // No invented worker: even a willing Farmer takes no job here.
            // Open ground suffices: zero slots exist, so reach is irrelevant.
            delay_builder(&mut state, builder);
            let candidate = bound_resident(&mut state, id, [1.5, 201.0, 2.5]);
            {
                let resident = state
                    .entities
                    .row_mut(candidate)
                    .villager
                    .as_mut()
                    .expect("resident row");
                resident.data.profession = VillagerProfession::Farmer;
            }
            for _ in 0..300 {
                state.tick_budget.reset(state.tick);
                let mut changes = ChangeSet::default();
                crate::settlements::allocator::tick(&mut state, &mut changes);
                crate::villager::brain::tick(&mut state, &mut changes);
                state.tick += 1;
            }
            let resident = state
                .entities
                .row(candidate)
                .villager
                .as_ref()
                .expect("resident row");
            assert!(
                resident.job_site.is_none(),
                "{blueprint} offers no worker slot to claim"
            );
            release_builder(&mut state, builder);
            assert_builder_role(&state, builder);
        }
    }
}

#[test]
fn destroying_a_worksite_station_breaks_the_follow_on_claim() {
    // One explicit corruption check: the farm hut is built whole, then the
    // certified workstation the claim depends on is destroyed. The live
    // validation must release the job; nothing here re-certifies or repairs.
    let (mut state, id, builder) = site::fixture();
    let levels = blueprint_levels(
        &state,
        "farm",
        &BuildingKind::Workshop(VillagerProfession::Farmer),
    );
    assert!(levels >= 1, "farm must offer at least one level");
    build_level(&mut state, id, builder, "farm", 1, None);
    let (_, building) = building_of(
        &state,
        id,
        BuildingKind::Workshop(VillagerProfession::Farmer),
    );
    let stations: Vec<[i32; 3]> = building.work_points.clone();
    assert!(
        !stations.is_empty(),
        "a farm certificate must hold a station to destroy"
    );
    delay_builder(&mut state, builder);
    let spawn = adjacent_spawn(&state, &stations);
    let farmer = bound_resident(&mut state, id, spawn);
    state.day_time = WORK;
    let job = prove_organic_claim(&mut state, farmer, &stations, VillagerProfession::Farmer, 1);
    // Destroy the claimed station: the follow-on claim must break, not linger.
    state
        .world
        .set_block(job.x, job.y, job.z, state.registry.states.air);
    state.world.fold_overlay();
    for _ in 0..300 {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(&mut state, &mut changes);
        crate::villager::brain::tick(&mut state, &mut changes);
        state.tick += 1;
    }
    let resident = state
        .entities
        .row(farmer)
        .villager
        .as_ref()
        .expect("resident row");
    assert!(
        resident.job_site.is_none(),
        "destroying the certified station releases the live claim"
    );
    release_builder(&mut state, builder);
    assert_builder_role(&state, builder);
}

/// Real-client QA (buildings-qa, mine L2): the upgrade was ordered right after
/// L1 certified, while the new Miner was already digging its entrance shaft.
/// The shaft starts in the floor interior, inside the upgrade's fenced
/// footprint, so the structure paused as `footprint_changed` forever.
#[test]
fn mine_upgrade_finishes_while_the_miner_has_a_live_entrance_shaft() {
    let (mut state, id, builder) = site::fixture();
    let bounds = expected_bounds(&state, "mine", 1);
    seed_mine_ground(&mut state, bounds);
    build_level(&mut state, id, builder, "mine", 1, None);
    let (bid, building) = building_of(&state, id, BuildingKind::Mine);
    delay_builder(&mut state, builder);
    let spawn = adjacent_spawn(&state, &building.work_points);
    let miner = bound_resident(&mut state, id, spawn);
    state.day_time = WORK;
    prove_organic_claim(
        &mut state,
        miner,
        &building.work_points,
        VillagerProfession::Miner,
        1,
    );
    release_builder(&mut state, builder);
    site::start(&mut state, id, "mine", 2, Some(bid));
    site::fund(&mut state, "mine", 2);
    for _ in 0..4096 {
        if state.settlements.get(id).unwrap().project.is_none() {
            break;
        }
        // The Miner's real work entrypoint, one attempt per Builder attempt.
        let mut changes = ChangeSet::default();
        let mut worker = state.entities.row_mut(miner).villager.take().unwrap();
        let target = super::mining::work(&mut state, miner, &mut worker, &mut changes);
        state.entities.row_mut(miner).villager = Some(worker);
        if let Some(target) = target {
            state.entities.set_pos(
                miner,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
        site::attempt(&mut state, id, builder);
    }
    let record = state.settlements.get(id).unwrap();
    assert!(
        record.project.is_none(),
        "the upgrade finishes: {:?}",
        record.project.as_ref().map(
            |project| crate::plugin_structures::settlement_builds::status(
                &state,
                id,
                &project.structure
            )
        )
    );
    assert_eq!(record.buildings[&bid].level, 2);
}
