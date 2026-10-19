use super::mining_tests::{block, fixture, storage};
use super::{Activity, VillagerProfession, VillagerState, lumber, mining};
use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::settlements::buildings::{Building, BuildingId, BuildingKind};
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_script::ScriptWorldBounds;

/// Oak tree on dirt: trunk `height` logs at 201.., leaf ring at the crown.
fn tree(state: &mut TickState, x: i32, z: i32, height: i32) {
    state.world.set_block(x, 200, z, block(state, "dirt"));
    let log = block(state, "oak_log");
    for y in 201..201 + height {
        state.world.set_block(x, y, z, log);
    }
    let leaves = block(state, "oak_leaves");
    let top = 201 + height - 1;
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        state.world.set_block(x + dx, top, z + dz, leaves);
    }
    state.world.set_block(x, top + 1, z, leaves);
}

fn path(state: &TickState, x: i32, y: i32, z: i32) -> String {
    let id = state.world.block_at(x, y, z).unwrap();
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .unwrap()
        .block
        .id
        .path()
        .to_owned()
}

fn follow(state: &mut TickState, slot: EntitySlot, target: BlockPos) {
    state.entities.set_pos(
        slot,
        [
            f64::from(target.x) + 0.5,
            f64::from(target.y),
            f64::from(target.z) + 0.5,
        ],
    );
}

fn count(v: &VillagerState, item: u32) -> i32 {
    v.inventory
        .iter()
        .flatten()
        .filter(|stack| stack.item_id == item)
        .map(|stack| stack.count)
        .sum()
}

/// Player-placed leaf state: vanilla `persistent=true` over the default states.
fn persistent_leaves(state: &TickState) -> u32 {
    let id = mc_data::Identifier::parse("minecraft:oak_leaves").unwrap();
    let leaves = state.registry.blocks.block(&id).unwrap();
    let mut props = state
        .registry
        .blocks
        .by_id(leaves.default)
        .unwrap()
        .properties
        .clone();
    for (name, value) in &mut props {
        if name == "persistent" {
            *value = "true".to_owned();
        }
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &props)
        .unwrap()
        .0
}

/// Same trunk as [`tree`] but the whole crown is hand-placed decoration.
fn decorative_tree(state: &mut TickState, x: i32, z: i32, height: i32) {
    tree(state, x, z, height);
    let placed = persistent_leaves(state);
    let top = 201 + height - 1;
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        state.world.set_block(x + dx, top, z + dz, placed);
    }
    state.world.set_block(x, top + 1, z, placed);
}

/// Certify one more footprint over `(x, z)` in the villager's settlement.
fn certify_shed(state: &mut TickState, v: &VillagerState, x: i32, z: i32) {
    let settlement = v.settlement.unwrap();
    let id = BuildingId((settlement.0 << 32) | 99);
    state
        .settlements
        .get_mut(settlement)
        .unwrap()
        .buildings
        .insert(
            id,
            Building {
                id,
                settlement,
                kind: BuildingKind::Storage,
                level: 1,
                bounds: ScriptWorldBounds::new([x - 1, 195, z - 1], [x + 1, 207, z + 1]).unwrap(),
                work_points: vec![],
                revision: 1,
            },
        );
}

/// Search until the natural tree routes; the search itself never breaks.
fn acquire(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
) {
    let mut tick = 0u64;
    while !v.lumber_progress.has_target && tick < 80 {
        state.tick = tick * 20;
        tick += 1;
        if let Some(target) = lumber::work(state, slot, v, changes) {
            follow(state, slot, target);
        }
    }
    assert!(
        v.lumber_progress.has_target,
        "scan must find the natural tree"
    );
}

#[test]
fn lumberjack_routes_then_cuts_full_trunk_and_replants_matching_sapling() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    tree(&mut state, 10, 10, 5);
    let mut changes = ChangeSet::default();
    // Incremental scan: the planted column is index 612, reached in fixed
    // 32-column steps, so the first route arrives on the 20th attempt with
    // nothing broken before the walk.
    let mut calls = 0u64;
    let route = loop {
        state.tick = calls * 20;
        calls += 1;
        if let Some(target) = lumber::work(&mut state, slot, &mut v, &mut changes) {
            break target;
        }
        assert!(changes.blocks.is_empty(), "search never breaks");
        assert!(calls < 40, "bounded scan must reach the planted tree");
    };
    assert_eq!(calls, 20);
    assert_eq!(
        route,
        BlockPos {
            x: 10,
            y: 201,
            z: 10
        }
    );
    assert!(v.lumber_progress.has_target);
    assert_eq!(v.lumber_progress.height, 5);
    for tick in calls..calls + 60 {
        state.tick = tick * 20;
        let before = changes.blocks.len();
        if let Some(target) = lumber::work(&mut state, slot, &mut v, &mut changes) {
            assert!(
                changes.blocks.len() - before <= 1,
                "one-block mutation budget"
            );
            follow(&mut state, slot, target);
        }
        if !v.lumber_progress.has_target {
            break;
        }
    }
    assert!(!v.lumber_progress.has_target);
    let air = state.registry.states.air;
    assert_eq!(
        changes.blocks.iter().filter(|c| c.state == air).count(),
        5,
        "whole trunk across ticks"
    );
    assert_eq!(changes.blocks.len(), 6, "five cuts plus the replant");
    assert_eq!(
        changes.blocks.last().unwrap().position,
        BlockPos {
            x: 10,
            y: 201,
            z: 10
        }
    );
    for y in 202..206 {
        assert_eq!(path(&state, 10, y, 10), "air");
    }
    assert_eq!(path(&state, 10, 201, 10), "oak_sapling");
    assert_eq!(path(&state, 11, 205, 10), "oak_leaves", "crowns stay up");
    let oak = mining::item_id(&state, "oak_log").unwrap();
    assert_eq!(count(&v, oak), 5, "every trunk block collected");
}
#[test]
fn lumberjack_drops_tree_target_when_radius_shrinks_or_camp_unbinds() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    tree(&mut state, 10, 10, 5);
    let mut changes = ChangeSet::default();
    let mut tick = 0u64;
    while !v.lumber_progress.has_target && tick < 80 {
        state.tick = tick * 20;
        tick += 1;
        if let Some(target) = lumber::work(&mut state, slot, &mut v, &mut changes) {
            follow(&mut state, slot, target);
        }
    }
    assert!(v.lumber_progress.has_target);
    // Routing to the base was already proven by the loop's `follow`; do not
    // call `work` again here: beside the tree it would fell the base log and
    // the restored scan would find no mature tree.
    let cursor = v.lumber_progress.cursor;
    let settlement = v.settlement.unwrap();
    state.settlements.get_mut(settlement).unwrap().radius = 1;
    state.tick = tick * 20;
    assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    assert!(!v.lumber_progress.has_target);
    assert_eq!(v.lumber_progress.cursor, cursor);
    // Restored radius finds the same tree again and cuts the whole column.
    state.settlements.get_mut(settlement).unwrap().radius = 16;
    while !v.lumber_progress.has_target && tick < 200 {
        state.tick = tick * 20;
        tick += 1;
        if let Some(target) = lumber::work(&mut state, slot, &mut v, &mut changes) {
            follow(&mut state, slot, target);
        }
    }
    assert!(v.lumber_progress.has_target);
    v.settlement = None;
    v.job_site = None;
    assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    assert!(!v.lumber_progress.has_target);
    assert_eq!(v.lumber_progress.layer, 0);
}

#[test]
fn lumberjack_scan_is_bounded_per_attempt_and_wraps_radius() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    let mut changes = ChangeSet::default();
    for tick in 0..3 {
        state.tick = tick * 20;
        assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    }
    assert_eq!(v.lumber_progress.cursor, 3 * lumber::SEARCH_PER_TICK);
    assert!(changes.blocks.is_empty());
    // Fixture radius 16 spans 33x33 columns; the cursor wraps without extra scans.
    v.lumber_progress.cursor = 33 * 33 - 10;
    state.tick = 60;
    assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    assert_eq!(v.lumber_progress.cursor, 22);
    assert!(changes.blocks.is_empty());
}

#[test]
fn lumberjack_completes_trunk_and_replants_across_full_inventory() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    tree(&mut state, 10, 10, 5);
    let oak = mining::item_id(&state, "oak_log").unwrap();
    let cobble = mining::item_id(&state, "cobblestone").unwrap();
    v.inventory = std::array::from_fn(|i| {
        if i == 0 {
            Some(ItemStack::new(oak, 60))
        } else {
            Some(ItemStack::new(cobble, 64))
        }
    });
    storage(&mut state, &v, [12, 201, 12]);
    let mut changes = ChangeSet::default();
    let mut tick = 0u64;
    while tick < 160 && (v.lumber_progress.has_target || changes.blocks.is_empty()) {
        state.tick = tick * 20;
        tick += 1;
        let before = changes.blocks.len();
        if let Some(target) = lumber::work(&mut state, slot, &mut v, &mut changes) {
            assert!(
                changes.blocks.len() - before <= 1,
                "one-block mutation budget"
            );
            follow(&mut state, slot, target);
        }
    }
    assert!(!v.lumber_progress.has_target);
    assert!(!v.lumber_progress.returning);
    let air = state.registry.states.air;
    assert_eq!(
        changes.blocks.iter().filter(|c| c.state == air).count(),
        5,
        "whole trunk cut across the deposit trip"
    );
    assert_eq!(changes.blocks.len(), 6, "five cuts plus the replant");
    assert_eq!(path(&state, 10, 201, 10), "oak_sapling");
    let chest = state.block_entities.get(&(12, 201, 12)).unwrap();
    assert_eq!(
        chest.slots.iter().map(|s| s.count).sum::<i32>(),
        512,
        "deposit trip drained the full inventory"
    );
    assert_eq!(
        chest
            .slots
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.count)
            .sum::<i32>(),
        512
    );
    assert_eq!(count(&v, oak), 1, "last trunk block after the trip");
}

#[test]
fn lumberjack_deposits_at_day_end_and_resumes_trunk_cursor() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    tree(&mut state, 10, 10, 4);
    let mut changes = ChangeSet::default();
    let mut tick = 0u64;
    while changes.blocks.len() < 2 && tick < 80 {
        state.tick = tick * 20;
        tick += 1;
        if let Some(target) = lumber::work(&mut state, slot, &mut v, &mut changes) {
            follow(&mut state, slot, target);
        }
    }
    assert_eq!(changes.blocks.len(), 2);
    assert_eq!(v.lumber_progress.layer, 2);
    storage(&mut state, &v, [3, 201, 5]);
    state.day_time = 9000;
    v.activity = Activity::Meet;
    state.tick = tick * 20;
    tick += 1;
    assert_eq!(
        lumber::work(&mut state, slot, &mut v, &mut changes),
        Some(BlockPos { x: 3, y: 201, z: 5 }),
        "day end routes to storage"
    );
    follow(&mut state, slot, BlockPos { x: 3, y: 201, z: 5 });
    assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    assert!(v.inventory.iter().all(Option::is_none));
    assert!(v.lumber_progress.has_target);
    assert_eq!(v.lumber_progress.layer, 2, "cursor survives the trip");
    let chest = state.block_entities.get(&(3, 201, 5)).unwrap();
    assert_eq!(
        chest.slots.iter().map(|s| s.count).sum::<i32>(),
        2,
        "day-end produce reached storage"
    );
    state.day_time = 3000;
    v.activity = Activity::Work;
    while v.lumber_progress.has_target && tick < 200 {
        state.tick = tick * 20;
        tick += 1;
        if let Some(target) = lumber::work(&mut state, slot, &mut v, &mut changes) {
            follow(&mut state, slot, target);
        }
    }
    assert!(!v.lumber_progress.has_target);
    assert_eq!(changes.blocks.len(), 5, "four trunk cuts plus the replant");
    assert_eq!(path(&state, 10, 201, 10), "oak_sapling");
    let oak = mining::item_id(&state, "oak_log").unwrap();
    assert_eq!(count(&v, oak), 2);
}

#[test]
fn lumberjack_honors_hours_griefing_state_and_camp_binding() {
    for reason in 0..5 {
        let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
        tree(&mut state, 10, 10, 5);
        match reason {
            0 => {
                state.day_time = 1000;
                v.activity = Activity::Idle;
            }
            1 => v.activity = Activity::Panic,
            2 => v.age = -10,
            3 => v.sleeping = true,
            _ => {
                state
                    .gamerules
                    .insert("mob_griefing".into(), "false".into());
            }
        }
        let mut changes = ChangeSet::default();
        for tick in 0..5 {
            state.tick = tick * 20;
            assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
        }
        assert!(changes.blocks.is_empty(), "reason {reason}");
        assert!(!v.lumber_progress.has_target, "reason {reason}");
    }
    // Sleeping lumberjacks never deposit, even full beside storage.
    {
        let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
        let cobble = mining::item_id(&state, "cobblestone").unwrap();
        v.inventory = std::array::from_fn(|_| Some(ItemStack::new(cobble, 64)));
        storage(&mut state, &v, [3, 201, 5]);
        v.sleeping = true;
        let mut changes = ChangeSet::default();
        state.tick = 20;
        assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
        assert!(v.inventory.iter().all(Option::is_some));
        assert!(
            state
                .block_entities
                .get(&(3, 201, 5))
                .unwrap()
                .slots
                .iter()
                .all(|s| s.count == 0)
        );
    }
    // Wrong profession or wrong camp kind never works.
    {
        let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
        tree(&mut state, 10, 10, 5);
        v.data.profession = VillagerProfession::Miner;
        let mut changes = ChangeSet::default();
        for tick in 0..3 {
            state.tick = tick * 20;
            assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
        }
        assert!(changes.blocks.is_empty());
    }
    {
        let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
        v.data.profession = VillagerProfession::Lumberjack;
        let mut changes = ChangeSet::default();
        for tick in 0..3 {
            state.tick = tick * 20;
            assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
        }
        assert!(changes.blocks.is_empty());
    }
}

#[test]
fn lumberjack_searches_hillsides_and_finishes_tall_trunks_with_matching_replants() {
    for (stem, sapling, base_y, height) in [
        ("spruce", "spruce_sapling", 211, 40),
        ("mangrove", "mangrove_propagule", 201, 5),
    ] {
        let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
        let log = block(&state, &format!("{stem}_log"));
        let leaves = block(&state, &format!("{stem}_leaves"));
        state
            .world
            .set_block(10, base_y - 1, 10, block(&state, "dirt"));
        for y in base_y..base_y + height {
            state.world.set_block(10, y, 10, log);
        }
        state.world.set_block(10, base_y + height, 10, leaves);
        let mut changes = ChangeSet::default();
        lumber::work(&mut state, slot, &mut v, &mut changes);
        v.lumber_progress.cursor = 612;
        v.lumber_progress.vertical = u32::from(base_y > 209);
        state.tick = 20;
        let target = lumber::work(&mut state, slot, &mut v, &mut changes).unwrap();
        follow(&mut state, slot, target);
        for tick in 2..height as u64 + 3 {
            state.tick = tick * 20;
            lumber::work(&mut state, slot, &mut v, &mut changes);
        }
        assert!(!v.lumber_progress.has_target);
        assert_eq!(
            changes
                .blocks
                .iter()
                .filter(|b| b.state == state.registry.states.air)
                .count(),
            height as usize
        );
        assert_eq!(path(&state, 10, base_y, 10), sapling);
        let item = mining::item_id(&state, &format!("{stem}_log")).unwrap();
        assert_eq!(count(&v, item), height);
    }
}

#[test]
fn lumberjack_ignores_decorative_persistent_leaves() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    decorative_tree(&mut state, 10, 10, 5);
    let mut changes = ChangeSet::default();
    for tick in 0..40 {
        state.tick = tick * 20;
        assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    }
    assert!(
        !v.lumber_progress.has_target,
        "decorative crown is not a tree"
    );
    assert!(changes.blocks.is_empty(), "search never breaks");
    // Full radius sweep (33x33 columns) with no target: the scan ran.
    assert_eq!(
        v.lumber_progress.cursor,
        40 * lumber::SEARCH_PER_TICK % (33 * 33)
    );
}

#[test]
fn lumberjack_skips_natural_tree_inside_certified_building_at_search() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    tree(&mut state, 10, 10, 5);
    certify_shed(&mut state, &v, 10, 10);
    let mut changes = ChangeSet::default();
    for tick in 0..40 {
        state.tick = tick * 20;
        assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    }
    assert!(
        !v.lumber_progress.has_target,
        "shed footprint protects the tree"
    );
    assert!(changes.blocks.is_empty(), "search never breaks");
}

#[test]
fn lumberjack_stops_mid_trunk_when_building_certified_over_target() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    tree(&mut state, 10, 10, 5);
    let mut changes = ChangeSet::default();
    acquire(&mut state, slot, &mut v, &mut changes);
    let mut tick = 80u64;
    for _ in 0..2 {
        state.tick = tick * 20;
        tick += 1;
        let before = changes.blocks.len();
        let target =
            lumber::work(&mut state, slot, &mut v, &mut changes).expect("routing to trunk");
        assert_eq!(changes.blocks.len(), before + 1, "one layer per attempt");
        follow(&mut state, slot, target);
    }
    assert_eq!(v.lumber_progress.layer, 2);
    // A player certifies a shed over the stump: no further breaks.
    certify_shed(&mut state, &v, 10, 10);
    let breaks = changes.blocks.len();
    for _ in 0..5 {
        state.tick = tick * 20;
        tick += 1;
        assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    }
    assert!(!v.lumber_progress.has_target);
    assert_eq!(changes.blocks.len(), breaks, "certification stops logging");
}

#[test]
fn lumberjack_stops_mid_trunk_when_crown_turns_persistent() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    tree(&mut state, 10, 10, 5);
    let mut changes = ChangeSet::default();
    acquire(&mut state, slot, &mut v, &mut changes);
    let mut tick = 80u64;
    for _ in 0..2 {
        state.tick = tick * 20;
        tick += 1;
        let before = changes.blocks.len();
        let target =
            lumber::work(&mut state, slot, &mut v, &mut changes).expect("routing to trunk");
        assert_eq!(changes.blocks.len(), before + 1, "one layer per attempt");
        follow(&mut state, slot, target);
    }
    assert_eq!(v.lumber_progress.layer, 2);
    // A player replaces the wild crown with decorative persistent leaves.
    let placed = persistent_leaves(&state);
    let top = 201 + 5 - 1;
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        state.world.set_block(10 + dx, top, 10 + dz, placed);
    }
    state.world.set_block(10, top + 1, 10, placed);
    let breaks = changes.blocks.len();
    for _ in 0..5 {
        state.tick = tick * 20;
        tick += 1;
        assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    }
    assert!(!v.lumber_progress.has_target);
    assert_eq!(
        changes.blocks.len(),
        breaks,
        "decorative crown stops logging"
    );
}

#[test]
fn lumberjack_protects_its_own_certified_camp_footprint_too() {
    let (mut state, slot, mut v) = fixture(BuildingKind::LumberCamp);
    tree(&mut state, 4, 4, 5);
    let mut changes = ChangeSet::default();
    for tick in 0..40 {
        state.tick = tick * 20;
        assert_eq!(lumber::work(&mut state, slot, &mut v, &mut changes), None);
    }
    assert!(!v.lumber_progress.has_target);
    assert!(
        changes.blocks.is_empty(),
        "the worker's camp is also a certified building"
    );
}
