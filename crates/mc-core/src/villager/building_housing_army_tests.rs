//! Binding acceptance for House and Barracks blueprints: every level is built
//! by the REAL Builder through the shared construction fixture, certified at
//! that level, then exercised through the REAL follow-on systems.
//!
//! * House: a bound adult resident claims a real blueprint bed through the
//!   real allocator and falls asleep through the real brain at night.
//! * Barracks: the real army recruiter fills exactly the beds-backed
//!   `job_slots` quota with native residents carrying Soldier state.
//!
//! Declared fixture assistance (no behaviour is faked):
//! * The Builder is the first housed resident. Each upgrade adds one resident,
//!   matching the real 1/2/3 bed capacity without suppressing competing claims.
//! * Residents follow only their OWN admitted `navigation_path` cells.
//!   Home and sleeping flags are never assigned manually.
//! * Day/night is controlled explicitly; allocator+brain use the same step
//!   sequence as allocator regression tests, without unrelated planner ticks.
//! * Barracks upgrades keep the same BuildingId, so no dismiss is needed for
//!   the upgrade itself; dismiss is still demonstrated against the level-1
//!   quota to prove a freed slot is reclaimable by the same native body.

use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::settlements::SettlementId;
use crate::settlements::allocator::tests as residents;
use crate::settlements::blueprints::Rotation;
use crate::settlements::buildings::{Building, BuildingId, BuildingKind};
use crate::settlements::construction::tests as site;
use crate::tick::TickState;
use crate::villager::VillagerProfession;
use mc_script::ScriptWorldBounds;
use mc_world::BlockStateId;
use uuid::Uuid;

/// Canonical construction site used by the shared fixture helpers.
const ORIGIN: [i32; 3] = [3, 200, 3];
/// Mid-night: the vanilla schedule rests adults, so a homed resident sleeps.
const NIGHT: i64 = 18_000;
/// Ticks a proof loop may run allocator+brain before the claim must hold.
const PROOF_TICKS: usize = 1500;

fn blueprint_levels(state: &TickState, blueprint: &str, kind: BuildingKind) -> u8 {
    let bp =
        crate::settlements::blueprints::get(&state.registry, blueprint).expect("blueprint exists");
    assert_eq!(bp.kind, kind, "blueprint {blueprint} has the intended kind");
    assert!(
        !bp.levels.is_empty(),
        "blueprint {blueprint} has no levels; a vacuous pass is refused"
    );
    u8::try_from(bp.levels.len()).expect("blueprint level count fits in u8")
}

fn expected_bounds(state: &TickState, blueprint: &str, level: u8) -> ScriptWorldBounds {
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

/// Every bed head the certification box actually holds: a real placed block
/// whose foot half is present, never a catalog or level-table expectation.
fn real_beds(state: &TickState, bounds: ScriptWorldBounds) -> Vec<BlockPos> {
    let mut beds = Vec::new();
    for x in bounds.min[0]..=bounds.max[0] {
        for y in bounds.min[1]..=bounds.max[1] {
            for z in bounds.min[2]..=bounds.max[2] {
                let Some(block) = state.world.block_at(x, y, z) else {
                    continue;
                };
                let Some(definition) = state.registry.blocks.by_id(BlockStateId(block)) else {
                    continue;
                };
                if !definition.block.id.path().ends_with("_bed") {
                    continue;
                }
                if !definition
                    .properties
                    .iter()
                    .any(|(key, value)| key == "part" && value == "head")
                {
                    continue;
                }
                let pos = BlockPos { x, y, z };
                if crate::settlements::allocator::bed_pair(&state.world, &state.registry, pos)
                    .is_some()
                {
                    beds.push(pos);
                }
            }
        }
    }
    beds.sort_unstable_by_key(|pos| (pos.x, pos.y, pos.z));
    beds
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

fn bound_resident(state: &mut TickState, id: SettlementId, pos: [f64; 3]) -> EntitySlot {
    let slot = residents::villager(state, pos);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(state, id, entity).expect("resident binds within the radius");
    slot
}

/// Prefer a bed no other resident claims, so the proof never depends on
/// scheduler tie-breaking; any real bed is acceptable as a fallback.
fn pick_free_bed(state: &TickState, beds: &[BlockPos]) -> BlockPos {
    beds.iter()
        .find(|bed| {
            state
                .villager_pois
                .homes
                .get(*bed)
                .is_none_or(|owners| owners.is_empty())
        })
        .copied()
        .or_else(|| beds.first().copied())
        .expect("a certified level always holds a bed")
}

/// A walkable cell beside the bed (head or foot neighbours), so the claim
/// starts inside the real sleep reach and long walks stay the exception.
fn adjacent_spawn(state: &TickState, bed: BlockPos) -> Option<[f64; 3]> {
    let mut anchors = vec![bed];
    if let Some(foot) = crate::settlements::allocator::bed_pair(&state.world, &state.registry, bed)
    {
        anchors.push(foot);
    }
    for anchor in anchors {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let cell = BlockPos {
                x: anchor.x + dx,
                y: anchor.y,
                z: anchor.z + dz,
            };
            if let Some(spot) = crate::settlements::allocator::standing(
                &state.world,
                &state.registry,
                cell,
                0.3,
                1.95,
            ) {
                return Some(spot);
            }
        }
    }
    None
}

/// Declared assistance: step the resident along its OWN admitted route when
/// the brain has not walked it into sleep reach yet.
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

fn resident_step(state: &mut TickState) {
    state.tick_budget.reset(state.tick);
    let mut changes = ChangeSet::default();
    crate::settlements::allocator::tick(state, &mut changes);
    crate::villager::brain::tick(state, &mut changes);
    state.tick += 1;
}

/// Drive the real allocator+brain at night until the resident both holds a
/// real blueprint bed and actually sleeps in it.
fn prove_sleep(state: &mut TickState, slot: EntitySlot, beds: &[BlockPos]) -> BlockPos {
    for _ in 0..PROOF_TICKS {
        resident_step(state);
        let resident = state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .expect("resident row");
        if resident.home.is_some() && resident.sleeping {
            break;
        }
        if resident.home.is_some() {
            follow_admitted_route(state, slot);
        }
    }
    let resident = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .expect("resident row");
    let home = resident
        .home
        .expect("bound adult claims a real blueprint bed");
    assert!(
        beds.contains(&home),
        "claimed home {home:?} must be a bed this building level placed"
    );
    assert!(
        resident.sleeping,
        "the resident must actually sleep; flags are never assigned manually"
    );
    assert!(
        crate::settlements::allocator::bed_pair(&state.world, &state.registry, home).is_some(),
        "the slept-in bed keeps both halves"
    );
    assert!(
        resident.last_slept.is_some(),
        "sleep leaves its durable trace"
    );
    home
}

fn assert_soldier(state: &TickState, id: SettlementId, slot: EntitySlot, bid: BuildingId) {
    let resident = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .expect("resident row");
    assert_eq!(
        resident.settlement,
        Some(id),
        "the soldier stays a bound resident"
    );
    assert_eq!(
        resident.data.profession,
        VillagerProfession::Soldier,
        "the recruit takes the Soldier profession"
    );
    assert_eq!(
        resident
            .soldier
            .as_ref()
            .expect("recruit carries soldier state")
            .barracks,
        bid,
        "soldier state references the completed barracks"
    );
}

/// Open ground outside the build footprint; no full tick ever runs in the
/// barracks test, so only settlement-radius membership matters.
fn recruit_spot(counter: &mut usize) -> [f64; 3] {
    const SPOTS: [[f64; 3]; 6] = [
        [1.5, 201.0, 2.5],
        [1.5, 201.0, 3.5],
        [1.5, 201.0, 4.5],
        [1.5, 201.0, 5.5],
        [1.5, 201.0, 6.5],
        [2.5, 201.0, 1.5],
    ];
    let spot = SPOTS[*counter % SPOTS.len()];
    *counter += 1;
    spot
}

#[test]
fn house_levels_build_retain_identity_and_sleep_in_real_beds() {
    let (mut state, id, builder) = site::fixture();
    let levels = blueprint_levels(&state, "house", BuildingKind::House);
    let mut bid: Option<BuildingId> = None;
    for level in 1..=levels {
        state.day_time = 3000;
        for _ in 0..PROOF_TICKS {
            let worker = state.entities.row(builder).villager.as_ref().unwrap();
            if !worker.sleeping && worker.activity == crate::villager::Activity::Work {
                break;
            }
            resident_step(&mut state);
        }
        assert_eq!(
            state
                .entities
                .row(builder)
                .villager
                .as_ref()
                .unwrap()
                .activity,
            crate::villager::Activity::Work,
            "the real brain resumes Builder work after night"
        );
        site::start(&mut state, id, "house", level, bid);
        site::fund(&mut state, "house", level);
        site::complete(&mut state, id, builder);
        state.world.fold_overlay();
        assert_builder_role(&state, builder);
        crate::plugin_structures::validate(&state)
            .expect("structure ledger stays valid after a house build");
        let (found, building) = building_of(&state, id, BuildingKind::House);
        assert_eq!(
            building.level, level,
            "the house certifies at the intended level"
        );
        if let Some(expected) = bid {
            assert_eq!(found, expected, "the upgrade retains the BuildingId");
        } else {
            bid = Some(found);
        }
        assert_eq!(
            building.bounds,
            expected_bounds(&state, "house", level),
            "the certificate matches the blueprint level geometry"
        );
        let beds = real_beds(&state, building.bounds);
        assert_eq!(
            beds.len(),
            usize::from(level),
            "house level {level} holds exactly its beds"
        );
        let resident = if level == 1 {
            builder
        } else {
            let bed = pick_free_bed(&state, &beds);
            let spawn = adjacent_spawn(&state, bed).expect("a walkable cell beside a real bed");
            bound_resident(&mut state, id, spawn)
        };
        state.day_time = NIGHT;
        let home = prove_sleep(&mut state, resident, &beds);
        assert!(
            (building.bounds.min[0]..=building.bounds.max[0]).contains(&home.x)
                && (building.bounds.min[1]..=building.bounds.max[1]).contains(&home.y)
                && (building.bounds.min[2]..=building.bounds.max[2]).contains(&home.z),
            "the slept-in bed lies inside this level's certificate"
        );
    }
}

#[test]
fn barracks_levels_build_retain_identity_and_recruit_capped_soldiers() {
    let (mut state, id, builder) = site::fixture();
    let levels = blueprint_levels(&state, "barracks", BuildingKind::Barracks);
    let mut bid: Option<BuildingId> = None;
    let mut soldiers: Vec<Uuid> = Vec::new();
    let mut standby: Option<EntitySlot> = None;
    let mut counter = 0usize;
    for level in 1..=levels {
        site::start(&mut state, id, "barracks", level, bid);
        site::fund(&mut state, "barracks", level);
        site::complete(&mut state, id, builder);
        assert_builder_role(&state, builder);
        crate::plugin_structures::validate(&state)
            .expect("structure ledger stays valid after a barracks build");
        let (found, building) = building_of(&state, id, BuildingKind::Barracks);
        assert_eq!(
            building.level, level,
            "the barracks certifies at the intended level"
        );
        if let Some(expected) = bid {
            assert_eq!(found, expected, "the upgrade retains the BuildingId");
        } else {
            bid = Some(found);
        }
        let current = bid.expect("barracks certified");
        assert_eq!(
            building.bounds,
            expected_bounds(&state, "barracks", level),
            "the certificate matches the blueprint level geometry"
        );
        let beds = real_beds(&state, building.bounds);
        let quota = crate::settlements::buildings::job_slots(BuildingKind::Barracks, level);
        assert_eq!(
            quota,
            usize::from(level) * 2,
            "barracks slots are beds-backed"
        );
        assert_eq!(
            beds.len(),
            quota,
            "built barracks level {level} holds exactly its beds"
        );
        while soldiers.len() < quota {
            if let Some(slot) = standby.take() {
                let uuid = crate::settlements::army::recruit(&mut state, id)
                    .expect("carried-over idle adult claims a free soldier slot");
                let got = state
                    .entities
                    .slot_of_uuid(uuid)
                    .expect("recruit is a live native body");
                assert_eq!(got, slot, "the intended idle worker claims its slot");
                assert_soldier(&state, id, got, current);
                soldiers.push(uuid);
            } else {
                let slot = bound_resident(&mut state, id, recruit_spot(&mut counter));
                let uuid = crate::settlements::army::recruit(&mut state, id)
                    .expect("idle bound adult claims a free soldier slot");
                let got = state
                    .entities
                    .slot_of_uuid(uuid)
                    .expect("recruit is a live native body");
                assert_eq!(got, slot, "the intended idle worker claims its slot");
                assert_soldier(&state, id, got, current);
                soldiers.push(uuid);
            }
        }
        let probe = bound_resident(&mut state, id, recruit_spot(&mut counter));
        standby = Some(probe);
        assert!(
            crate::settlements::army::recruit(&mut state, id).is_err(),
            "level {level} barracks caps at {quota} soldiers despite an idle adult"
        );
        if level == 1 {
            let freed = soldiers[0];
            crate::settlements::army::dismiss(&mut state, id, freed)
                .expect("dismiss returns the soldier");
            let slot = state
                .entities
                .slot_of_uuid(freed)
                .expect("dismissed body stays live");
            let resident = state
                .entities
                .row(slot)
                .villager
                .as_ref()
                .expect("resident row");
            assert!(resident.soldier.is_none(), "dismiss clears soldier state");
            assert_eq!(
                resident.data.profession,
                VillagerProfession::None,
                "dismiss restores the idle profession"
            );
            soldiers.remove(0);
            let again = crate::settlements::army::recruit(&mut state, id)
                .expect("a freed slot is reclaimable");
            assert_eq!(again, freed, "the same native body reclaims its freed slot");
            let slot = state
                .entities
                .slot_of_uuid(again)
                .expect("re-recruit is a live native body");
            assert_soldier(&state, id, slot, current);
            soldiers.push(again);
        }
    }
}
