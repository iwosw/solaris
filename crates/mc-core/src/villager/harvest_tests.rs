use super::{Activity, VillagerProfession, harvest};
use crate::boundary::{BlockPos, ChangeSet};
use crate::settlements::buildings::{self, BuildingKind};

fn fixture() -> (
    crate::tick::TickState,
    crate::entities::EntitySlot,
    Box<super::VillagerState>,
) {
    let (mut state, slot, mut v) =
        super::mining_tests::fixture(BuildingKind::Workshop(VillagerProfession::Farmer));
    let settlement = v.settlement.unwrap();
    state
        .settlements
        .get_mut(settlement)
        .unwrap()
        .buildings
        .clear();
    let composter = super::mining_tests::block(&state, "composter");
    // The certified work point is one cell off the spawn: a workstation inside
    // the farmer's own cell is a solid block, so no route could ever start.
    let work_point = BlockPos { x: 2, y: 201, z: 2 };
    state.world.set_block(3, 201, 3, state.registry.states.air);
    state
        .world
        .set_block(work_point.x, work_point.y, work_point.z, composter);
    buildings::certify(
        &mut state,
        settlement,
        BuildingKind::Workshop(VillagerProfession::Farmer),
        mc_script::ScriptWorldBounds::new([1, 200, 1], [14, 203, 14]).unwrap(),
    )
    .unwrap();
    v.job_site = Some(work_point);
    v.data.profession = VillagerProfession::Farmer;
    state.entities.set_ground(slot, true);
    state.world.fold_overlay();
    state.tickets.insert(
        "farm-test".into(),
        (crate::boundary::ChunkPos { x: 0, z: 0 }, 0),
    );
    state.refresh_active_chunks(0);
    (state, slot, v)
}

fn crop(state: &mut crate::tick::TickState, x: i32, z: i32) -> u32 {
    let farmland = super::mining_tests::block(state, "farmland");
    let mature =
        crate::settlements::allocator::tests::block(state, "minecraft:carrots", &[("age", "7")]);
    state.world.set_block(x, 200, z, farmland);
    state.world.set_block(x, 201, z, mature);
    mature
}

#[test]
fn certified_farmer_walks_to_distant_crop_and_repeats() {
    let (mut state, slot, v) = fixture();
    crop(&mut state, 12, 3);
    crop(&mut state, 12, 12);
    state.world.fold_overlay();
    state.entities.row_mut(slot).villager = Some(v);
    let young =
        crate::settlements::allocator::tests::block(&state, "minecraft:carrots", &[("age", "0")]);
    let mut journal = Vec::new();
    for _ in 0..1800 {
        state.tick_budget.reset(state.tick);
        let mut changes = ChangeSet::default();
        crate::settlements::allocator::tick(&mut state, &mut changes);
        super::brain::tick(&mut state, &mut changes);
        journal.extend(changes.blocks);
        state.tick += 1;
        if state.world.block_at(12, 201, 12) == Some(young) {
            break;
        }
    }
    assert_eq!(state.world.block_at(12, 201, 3), Some(young));
    assert_eq!(state.world.block_at(12, 201, 12), Some(young));
    assert!(
        state.entities.pos(slot)[0] > 10.0,
        "real brain motion must reach the field"
    );
    assert_eq!(journal.len(), 2, "both harvests are journaled");
}

/// `[mob_griefing] villager_farming = false` stops crop work while the
/// master gamerule stays on.
#[test]
fn villager_farming_switch_stops_harvesting() {
    let (mut state, slot, mut v) = fixture();
    state.mob_griefing.villager_farming = false;
    let mature = crop(&mut state, 12, 3);
    state.world.fold_overlay();
    state.entities.set_pos(slot, [12.5, 201.0, 3.5]);
    let mut changes = ChangeSet::default();
    for _ in 0..20 {
        harvest::work(&mut state, slot, &mut v, &mut changes);
        state.tick += 40;
    }
    assert_eq!(state.world.block_at(12, 201, 3), Some(mature));
    assert!(
        changes.blocks.is_empty(),
        "the disabled switch harvests nothing"
    );
}

#[test]
fn certified_farmer_never_harvests_outside_bounds_or_after_work() {
    let (mut state, slot, mut v) = fixture();
    let outside = crop(&mut state, 0, 3);
    let inside = crop(&mut state, 12, 3);
    let mut changes = ChangeSet::default();
    for _ in 0..20 {
        harvest::work(&mut state, slot, &mut v, &mut changes);
        state.tick += 40;
    }
    assert_eq!(state.world.block_at(0, 201, 3), Some(outside));
    assert!(
        changes.blocks.is_empty(),
        "no remote harvesting before walking"
    );
    state.entities.set_pos(slot, [12.5, 201.0, 3.5]);
    v.activity = Activity::Rest;
    harvest::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(state.world.block_at(12, 201, 3), Some(inside));
    assert!(changes.blocks.is_empty());
}

#[test]
fn certified_farmer_walks_to_the_nearest_crop_not_the_first_scanned() {
    let (mut state, slot, mut v) = fixture();
    // The sweep order reaches x=2 long before x=12; the farmer stands by the
    // later one, so only a nearest-crop selection may harvest it first.
    let far = crop(&mut state, 2, 2);
    crop(&mut state, 12, 12);
    state.world.fold_overlay();
    state.entities.set_pos(slot, [11.5, 201.0, 11.5]);
    let young =
        crate::settlements::allocator::tests::block(&state, "minecraft:carrots", &[("age", "0")]);
    let mut changes = ChangeSet::default();
    for _ in 0..8 {
        harvest::work(&mut state, slot, &mut v, &mut changes);
        state.tick += 40;
    }
    assert_eq!(
        state.world.block_at(12, 201, 12),
        Some(young),
        "the nearest mature crop is broken and replanted first"
    );
    assert_eq!(
        state.world.block_at(2, 201, 2),
        Some(far),
        "the farther crop scanned first stays standing"
    );
    assert_eq!(changes.blocks.len(), 1);
}

#[test]
fn certified_farm_sweep_is_bounded_and_resets_with_the_certificate() {
    let (mut state, slot, mut v) = fixture();
    let distant = crop(&mut state, 14, 14);
    state.world.fold_overlay();
    state.entities.set_pos(slot, [1.5, 201.0, 1.5]);
    let mut changes = ChangeSet::default();
    let volume: u32 = 14 * 4 * 14;
    let calls = volume.div_ceil(harvest::SCAN_PROBES);
    for done in 1..calls {
        harvest::work(&mut state, slot, &mut v, &mut changes);
        assert_eq!(
            v.harvest_progress.cursor,
            done * harvest::SCAN_PROBES,
            "one decision probes at most SCAN_PROBES cells"
        );
        assert_eq!(
            harvest::target(&v),
            None,
            "no crop is selected before the certificate volume is fully swept"
        );
        state.tick += 40;
    }
    harvest::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(
        harvest::target(&v),
        Some(BlockPos {
            x: 14,
            y: 201,
            z: 14
        })
    );
    assert_eq!(
        state.world.block_at(14, 201, 14),
        Some(distant),
        "a distant crop is only walked to, never broken remotely"
    );
    // A new certificate revision invalidates the cached crop and cursor.
    let settlement = v.settlement.unwrap();
    let record = state.settlements.get_mut(settlement).unwrap();
    record.buildings.values_mut().next().unwrap().revision += 1;
    state.tick += 40;
    harvest::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(harvest::target(&v), None);
    assert_eq!(v.harvest_progress.cursor, harvest::SCAN_PROBES);
}
