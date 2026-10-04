use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_world::{
    BlockRegistry, BlockStateId, ChestBlockEntity, Chunk, ChunkPos, FurnaceBlockEntity,
    FurnaceSlot, HopperBlockEntity, ScheduledBlockTick, WorldStorage,
};

use super::{
    HopperTransferContext, insert_hopper_fuel_into_furnace, plan_resident_hopper_transfer,
};
use crate::play::FurnaceKind;

#[test]
fn hopper_rejects_non_flammable_wood_without_mutating_the_furnace() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:warped_stairs").unwrap(),
        protocol_id: 44,
    }]);
    let tags = mc_data::tags::solaris_required_item_tags(&items);
    let moving = FurnaceSlot {
        count: 1,
        item_id: 44,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
        components: None,
    };
    let mut furnace = FurnaceBlockEntity::default();

    assert_eq!(
        insert_hopper_fuel_into_furnace(&tags, FurnaceKind::Furnace, &mut furnace, &moving,),
        None
    );
    assert!(furnace.slots[1].is_empty());
}

fn fanout_state(id: u32, default: bool, properties: &[(&str, &str)]) -> BlockStateReport {
    BlockStateReport {
        id,
        default,
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    }
}

/// Built-base-class planning fanout: many due hoppers spread across chunks share
/// one planning-chunk union. Times per-tick resident transfer planning over the
/// shared snapshot; correctness assertions pin the planned movement while the
/// printed wall time is the before/after signal for the staging bound.
#[test]
fn hopper_transfer_planning_covers_spread_due_union() {
    let mut hopper_properties = BTreeMap::new();
    hopper_properties.insert("facing".to_string(), vec!["east".to_string()]);
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![fanout_state(0, true, &[])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:chest").unwrap(),
                properties: BTreeMap::new(),
                states: vec![fanout_state(1, true, &[])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: hopper_properties,
                states: vec![fanout_state(2, true, &[("facing", "east")])],
            },
        ])
        .unwrap(),
    );
    let hopper_id = Identifier::parse("minecraft:hopper").unwrap();
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 64);
    for cx in -1..=4 {
        for cz in -1..=4 {
            let cpos = ChunkPos { x: cx, z: cz };
            storage
                .insert_generated_chunk(
                    cpos,
                    Chunk::empty(
                        cpos,
                        BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
        }
    }
    let mut ticks = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            let hopper_pos = mc_world::BlockPos {
                x: 1 + i * 8,
                y: 64,
                z: 1 + j * 8,
            };
            let source_pos = mc_world::BlockPos {
                y: 65,
                ..hopper_pos
            };
            let target_pos = mc_world::BlockPos {
                x: hopper_pos.x + 1,
                ..hopper_pos
            };
            storage.set_block_at(hopper_pos, BlockStateId(2)).unwrap();
            storage.set_block_at(source_pos, BlockStateId(1)).unwrap();
            storage.set_block_at(target_pos, BlockStateId(1)).unwrap();
            let mut source = ChestBlockEntity::default();
            source.slots[0] = FurnaceSlot {
                count: 8,
                item_id: 7,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
                stew_effects: Vec::new(),
                components: None,
            };
            storage.set_chest_block_entity(source_pos, source).unwrap();
            storage
                .set_chest_block_entity(target_pos, ChestBlockEntity::default())
                .unwrap();
            storage
                .set_hopper_block_entity(hopper_pos, HopperBlockEntity::default())
                .unwrap();
            ticks.push(ScheduledBlockTick::new(
                hopper_pos,
                hopper_id.clone(),
                20,
                0,
            ));
        }
    }
    let union = mc_simulation::scheduled_blocks::scheduled_block_planning_chunks(&ticks);
    assert!(
        union.len() > 9,
        "repro needs a wide due union, got {} chunks",
        union.len()
    );
    let snapshot = storage.read_view().snapshot_chunks(&union);
    let items = mc_data::items::ItemRegistry::default();
    let tags = mc_data::tags::TagsData::default();
    let recipes = Vec::new();
    let sessions = crate::play::SessionRegistry::new();
    let context = HopperTransferContext {
        blocks: blocks.as_ref(),
        items: &items,
        tags: &tags,
        recipes: &recipes,
        sessions: &sessions,
    };
    let started = Instant::now();
    let mut planned = 0usize;
    let mut chest_changes = 0usize;
    for _ in 0..60 {
        for tick in &ticks {
            let planned_transfer =
                plan_resident_hopper_transfer(&context, &blocks, &snapshot, &union, tick, 20)
                    .expect("spread hopper tick plans against its snapshot");
            assert!(
                planned_transfer.result.moved,
                "stocked source above an empty hopper moves one item"
            );
            assert!(
                !planned_transfer.plan.hoppers.is_empty(),
                "plan carries the hopper cooldown debit"
            );
            chest_changes += planned_transfer.plan.chests.len();
            planned += 1;
        }
    }
    let elapsed = started.elapsed();
    eprintln!(
        "hopper_fanout_repro: planned={planned} chest_changes={chest_changes} union_chunks={} elapsed_ms={} elapsed_us_per_plan={}",
        union.len(),
        elapsed.as_millis(),
        elapsed.as_micros() / planned.max(1) as u128,
    );
    assert!(
        chest_changes > 0,
        "repro must exercise chest movement planning"
    );
}
