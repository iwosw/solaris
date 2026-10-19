use super::{Activity, VillagerProfession, VillagerState, mining};
use crate::boundary::{BlockPos, ChangeSet, ChunkPos};
use crate::entities::EntitySlot;
use crate::settlements::buildings::{Building, BuildingId, BuildingKind};
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_script::ScriptWorldBounds;

pub(super) fn fixture(kind: BuildingKind) -> (TickState, EntitySlot, Box<VillagerState>) {
    let mut state = super::conversion_tests::state();
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(
            ChunkPos { x: 0, z: 0 },
            0,
        )]));
    for x in 0..16 {
        for z in 0..16 {
            for y in 190..206 {
                state.world.set_block(
                    x,
                    y,
                    z,
                    if y <= 200 {
                        state.registry.states.stone
                    } else {
                        state.registry.states.air
                    },
                );
            }
        }
    }
    let settlement = state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "workers".into(),
            "owner".into(),
            [8, 201, 8],
            16,
        )
        .unwrap();
    let workstation = if kind == BuildingKind::Mine {
        "smithing_table"
    } else {
        "fletching_table"
    };
    let job = BlockPos { x: 3, y: 201, z: 3 };
    let block = block(&state, workstation);
    state.world.set_block(job.x, job.y, job.z, block);
    let id = BuildingId((settlement.0 << 32) | 1);
    let record = state.settlements.get_mut(settlement).unwrap();
    record.buildings.insert(
        id,
        Building {
            id,
            settlement,
            kind,
            level: 1,
            bounds: if kind == BuildingKind::Mine {
                ScriptWorldBounds::new([1, 192, 1], [14, 204, 14]).unwrap()
            } else {
                ScriptWorldBounds::new([1, 200, 1], [5, 204, 5]).unwrap()
            },
            work_points: vec![[3, 201, 3]],
            revision: 1,
        },
    );
    record.building_sequence = 1;
    let entity_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let entity = state.spawn_mob(entity_type, [3.5, 201.0, 3.5], 20.0);
    let slot = state.entities.slot_of(entity).unwrap();
    let mut v = state.entities.row_mut(slot).villager.take().unwrap();
    v.settlement = Some(settlement);
    v.job_site = Some(job);
    v.data.profession = if kind == BuildingKind::Mine {
        VillagerProfession::Miner
    } else {
        VillagerProfession::Lumberjack
    };
    v.activity = Activity::Work;
    state.day_time = 3000;
    (state, slot, v)
}

pub(super) fn block(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
        .default
        .0
}

pub(super) fn storage(state: &mut TickState, v: &VillagerState, position: [i32; 3]) {
    let chest = block(state, "chest");
    state
        .world
        .set_block(position[0], position[1], position[2], chest);
    state.block_entities.insert(
        (position[0], position[1], position[2]),
        crate::tick::BlockEntityState {
            slots: vec![ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    let settlement = v.settlement.unwrap();
    let id = BuildingId((settlement.0 << 32) | 2);
    let record = state.settlements.get_mut(settlement).unwrap();
    record.buildings.insert(
        id,
        Building {
            id,
            settlement,
            kind: BuildingKind::Storage,
            level: 1,
            bounds: ScriptWorldBounds::new(
                position,
                [position[0] + 2, position[1] + 2, position[2] + 2],
            )
            .unwrap(),
            work_points: vec![position],
            revision: 1,
        },
    );
    record.building_sequence = 2;
}

#[test]
fn miner_staircase_and_branches_stay_inside_zone_and_one_break_per_attempt() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let bounds = crate::settlements::buildings::building_for_job(&state, &v)
        .unwrap()
        .bounds;
    let mut changes = ChangeSet::default();
    for tick in 0..600 {
        state.tick = tick * 20;
        let before = changes.blocks.len();
        let target = mining::work(&mut state, slot, &mut v, &mut changes);
        assert!(
            changes.blocks.len() - before <= 1,
            "fixed one-block mutation budget"
        );
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
    }
    assert!(
        changes.blocks.len() > 40,
        "miner must get beyond the stairs into branches"
    );
    assert!(v.mining_progress.cell > 20);
    for change in &changes.blocks {
        for (coordinate, (min, max)) in [change.position.x, change.position.y, change.position.z]
            .into_iter()
            .zip(bounds.min.into_iter().zip(bounds.max))
        {
            assert!((min..=max).contains(&coordinate));
        }
        assert_eq!(change.state, state.registry.states.air);
    }
    let dirt = mining::item_id(&state, "cobblestone").unwrap();
    assert!(
        v.inventory
            .iter()
            .flatten()
            .any(|s| s.item_id == dirt && s.count > 0)
    );
}

#[test]
fn miner_refuses_fluids_adjacent_fluids_and_air_pockets_before_breaking() {
    for (position, path) in [
        ([4, 200, 3], "water"),
        ([4, 200, 3], "lava"),
        ([4, 200, 3], "air"),
        ([4, 200, 4], "water"),
        ([4, 199, 3], "lava"),
        ([4, 200, 4], "air"),
    ] {
        let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
        let id = block(&state, path);
        state
            .world
            .set_block(position[0], position[1], position[2], id);
        let mut changes = ChangeSet::default();
        for tick in 0..8 {
            state.tick = tick * 20;
            mining::work(&mut state, slot, &mut v, &mut changes);
            if tick == 0 {
                assert!(
                    changes.blocks.is_empty(),
                    "reject {path} before opening any face"
                );
            }
        }
        assert!(
            changes.blocks.iter().all(|change| {
                let p = change.position;
                (p.x - position[0]).abs() + (p.y - position[1]).abs() + (p.z - position[2]).abs()
                    > 1
            }),
            "a bypass must not expose {path} at {position:?}"
        );
        assert_eq!(
            state.world.block_at(position[0], position[1], position[2]),
            Some(id)
        );
    }
}

#[test]
fn miner_deposits_at_day_end_and_resumes_cursor_after_upgrade() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let mut changes = ChangeSet::default();
    for tick in 0..3 {
        state.tick = tick * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
    }
    assert!(v.inventory.iter().any(Option::is_some));
    let cursor = v.mining_progress.cell;
    storage(&mut state, &v, [3, 201, 5]);
    state.day_time = 9000;
    v.activity = Activity::Meet;
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert!(v.inventory.iter().all(Option::is_none));
    let chest = state.block_entities.get(&(3, 201, 5)).unwrap();
    assert!(chest.slots.iter().any(|s| !s.is_empty()));
    let id = BuildingId(v.mining_progress.building);
    state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .buildings
        .get_mut(&id)
        .unwrap()
        .revision += 1;
    state.day_time = 3000;
    v.activity = Activity::Work;
    state.tick += 1;
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(
        v.mining_progress.cell, cursor,
        "upgrading must not rewind into its own air tunnel"
    );
}

#[test]
fn miner_full_inventory_routes_to_storage_without_digging_and_retains_when_full() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    storage(&mut state, &v, [12, 201, 12]);
    let cobble = mining::item_id(&state, "cobblestone").unwrap();
    v.inventory = std::array::from_fn(|_| Some(ItemStack::new(cobble, 64)));
    let mut changes = ChangeSet::default();
    assert_eq!(
        mining::work(&mut state, slot, &mut v, &mut changes),
        Some(BlockPos {
            x: 12,
            y: 201,
            z: 12
        })
    );
    assert!(changes.blocks.is_empty());
    state.entities.set_pos(slot, [12.5, 201.0, 11.5]);
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert!(v.inventory.iter().all(Option::is_none));
    assert_eq!(
        state
            .block_entities
            .get(&(12, 201, 12))
            .unwrap()
            .slots
            .iter()
            .map(|s| s.count)
            .sum::<i32>(),
        512
    );
    let stock = state.block_entities.get_mut(&(12, 201, 12)).unwrap();
    stock.slots.fill(ItemStack::new(cobble, 64));
    v.inventory = std::array::from_fn(|_| Some(ItemStack::new(cobble, 64)));
    let carry = v.inventory.clone();
    let before = changes.blocks.len();
    assert!(mining::work(&mut state, slot, &mut v, &mut changes).is_none());
    assert_eq!(
        v.inventory, carry,
        "full storage must retain carried resources"
    );
    assert_eq!(
        changes.blocks.len(),
        before,
        "do not dig while waiting to unload"
    );
}

#[test]
fn miner_respects_work_hours_mob_griefing_and_wire_profession_looks() {
    assert_eq!(
        VillagerProfession::Miner.profession_id(),
        VillagerProfession::Toolsmith.profession_id()
    );
    assert_eq!(
        VillagerProfession::Lumberjack.profession_id(),
        VillagerProfession::Fletcher.profession_id()
    );
    for reason in 0..4 {
        let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
        match reason {
            0 => {
                state.day_time = 1000;
                v.activity = Activity::Idle;
            }
            1 => {
                v.age = -10;
            }
            2 => {
                v.activity = Activity::Panic;
            }
            _ => {
                state
                    .gamerules
                    .insert("mob_griefing".into(), "false".into());
            }
        }
        let mut changes = ChangeSet::default();
        for tick in 0..5 {
            state.tick = tick * 20;
            mining::work(&mut state, slot, &mut v, &mut changes);
        }
        assert!(changes.blocks.is_empty());
    }
}

#[test]
fn certified_miner_brain_uses_job_mapping_and_existing_navigation() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    v.data.profession = VillagerProfession::None;
    state.entities.row_mut(slot).villager = Some(v);
    state.entities.set_pos(slot, [2.5, 201.0, 3.5]);
    state
        .tickets
        .insert("worker-fixture".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(0);
    for _ in 0..300 {
        crate::tick::run_tick(&mut state, &[]);
    }
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.data.profession, VillagerProfession::Miner);
    assert!(
        v.mining_progress.cell >= 2,
        "the brain must dispatch physical mining"
    );
    assert!(
        state.entities.pos(slot)[1] < 201.0,
        "the miner must follow its excavated stairs"
    );
}

#[test]
fn miner_skips_a_permanent_hazard_and_opens_a_different_safe_branch() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let lava = block(&state, "lava");
    state.world.set_block(4, 200, 3, lava);
    let mut changes = ChangeSet::default();
    for tick in 0..160 {
        state.tick = tick * 20;
        if let Some(target) = mining::work(&mut state, slot, &mut v, &mut changes) {
            state.entities.set_pos(
                slot,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y),
                    f64::from(target.z) + 0.5,
                ],
            );
        }
    }
    assert!(
        changes.blocks.len() > 3,
        "one unsafe cell must not stop all mine output"
    );
    assert_eq!(state.world.block_at(4, 200, 3), Some(lava));
    assert!(
        changes.blocks.iter().all(|change| {
            let p = change.position;
            (p.x - 4).abs() + (p.y - 200).abs() + (p.z - 3).abs() > 1
        }),
        "no excavation may expose the lava face"
    );
}

#[test]
fn miner_refuses_waterlogged_blocks_and_fluid_holding_neighbors() {
    for name in [
        "oak_stairs",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
        "bubble_column",
        "powder_snow",
    ] {
        let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
        let id = block(&state, name);
        let id = if name == "oak_stairs" {
            crate::fluids::waterlogged(&state, id, true).unwrap()
        } else {
            id
        };
        state.world.set_block(4, 200, 4, id);
        let mut changes = ChangeSet::default();
        for tick in 0..3 {
            state.tick = tick * 20;
            mining::work(&mut state, slot, &mut v, &mut changes);
        }
        assert!(changes.blocks.is_empty(), "must not expose {name}");
        assert_eq!(state.world.block_at(4, 200, 4), Some(id));
    }
}

#[test]
fn miner_resets_cursor_on_bounds_change_but_not_level_only_upgrade() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let mut changes = ChangeSet::default();
    for tick in 0..3 {
        state.tick = tick * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
    }
    assert_eq!(v.mining_progress.cell, 1);
    let id = BuildingId(v.mining_progress.building);
    state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .buildings
        .get_mut(&id)
        .unwrap()
        .revision += 1;
    state.day_time = 1000;
    v.activity = Activity::Idle;
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(v.mining_progress.cell, 1);
    let building = state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .buildings
        .get_mut(&id)
        .unwrap();
    building.bounds = ScriptWorldBounds::new([1, 191, 1], [14, 204, 14]).unwrap();
    building.revision += 1;
    mining::work(&mut state, slot, &mut v, &mut changes);
    assert_eq!(
        v.mining_progress.cell, 0,
        "a new zone changes cursor geometry"
    );
}

#[test]
fn miner_pauses_for_unloaded_neighbor_then_resumes_the_same_cell() {
    let (mut state, slot, mut v) = fixture(BuildingKind::Mine);
    let building = state
        .settlements
        .get_mut(v.settlement.unwrap())
        .unwrap()
        .buildings
        .values_mut()
        .next()
        .unwrap();
    building.bounds = ScriptWorldBounds::new([12, 192, 1], [20, 204, 14]).unwrap();
    building.work_points = vec![[13, 201, 3]];
    v.job_site = Some(BlockPos {
        x: 13,
        y: 201,
        z: 3,
    });
    state.entities.set_pos(slot, [13.5, 201.0, 3.5]);
    let mut changes = ChangeSet::default();
    for tick in 0..6 {
        state.tick = tick * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
    }
    assert_eq!(
        v.mining_progress.cell, 1,
        "unloaded space must not be treated as a permanent obstruction"
    );
    assert_eq!(changes.blocks.len(), 1);
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(
            ChunkPos { x: 1, z: 0 },
            0,
        )]));
    for x in 16..22 {
        for z in 0..16 {
            for y in 190..206 {
                state.world.set_block(
                    x,
                    y,
                    z,
                    if y <= 200 {
                        state.registry.states.stone
                    } else {
                        state.registry.states.air
                    },
                );
            }
        }
    }
    for tick in 6..9 {
        state.tick = tick * 20;
        mining::work(&mut state, slot, &mut v, &mut changes);
    }
    assert_eq!(v.mining_progress.cell, 2);
    assert_eq!(
        changes.blocks.len(),
        3,
        "loading the neighbor must release the paused excavation"
    );
}
