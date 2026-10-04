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

fn facing_state(id: u32, default: bool, facing: &str) -> BlockStateReport {
    fanout_state(id, default, &[("facing", facing)])
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

/// Chunk-boundary double-chest regression for the contact-chunk bound: the
/// proven X-row scheme (hopper → left → right → comparator), shifted so the
/// partner's far-side chunk is required. Without the partner's neighbours
/// staged, that chunk — and the comparator refresh — are silently skipped.
#[test]
fn hopper_contact_chunks_cover_corner_double_chest_comparator_side() {
    let mut chest_properties = BTreeMap::new();
    chest_properties.insert("facing".to_string(), vec!["north".to_string()]);
    chest_properties.insert(
        "type".to_string(),
        vec![
            "single".to_string(),
            "left".to_string(),
            "right".to_string(),
        ],
    );
    let mut hopper_properties = BTreeMap::new();
    hopper_properties.insert("facing".to_string(), vec!["east".to_string()]);
    let mut comparator_properties = BTreeMap::new();
    comparator_properties.insert("facing".to_string(), vec!["west".to_string()]);
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![fanout_state(0, true, &[])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:chest").unwrap(),
                properties: chest_properties,
                states: vec![
                    fanout_state(1, true, &[("facing", "north"), ("type", "single")]),
                    fanout_state(2, false, &[("facing", "north"), ("type", "left")]),
                    fanout_state(3, false, &[("facing", "north"), ("type", "right")]),
                ],
            },
            BlockReport {
                id: Identifier::parse("minecraft:hopper").unwrap(),
                properties: hopper_properties,
                states: vec![facing_state(4, true, "east")],
            },
            BlockReport {
                id: Identifier::parse("minecraft:comparator").unwrap(),
                properties: comparator_properties,
                states: vec![fanout_state(5, true, &[("facing", "west")])],
            },
        ])
        .unwrap(),
    );
    // Proven row along X at fixed y/z: hopper(13) → left(14) → right(15) →
    // comparator(16). Chunks (0,0) and (1,0): contacts of hopper/target end
    // at x=15, so only the partner-neighbour fix stages the x=16 chunk.
    let hopper_pos = mc_world::BlockPos { x: 13, y: 64, z: 1 };
    let chest_pos = mc_world::BlockPos { x: 14, y: 64, z: 1 };
    let partner_pos = mc_world::BlockPos { x: 15, y: 64, z: 1 };
    let comparator_pos = mc_world::BlockPos { x: 16, y: 64, z: 1 };
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 8);
    for cpos in [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }] {
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
    storage.set_block_at(hopper_pos, BlockStateId(4)).unwrap();
    storage.set_block_at(chest_pos, BlockStateId(2)).unwrap();
    storage.set_block_at(partner_pos, BlockStateId(3)).unwrap();
    storage
        .set_block_at(comparator_pos, BlockStateId(5))
        .unwrap();
    // Eject path: the hopper itself is stocked (transfer pulls from above
    // only, and the side chest is the eject target, not a source). Both
    // chest halves stay empty; the moved item lands in the pair and the
    // comparator beside the partner must refresh.
    let mut hopper = HopperBlockEntity::default();
    hopper.slots[0] = FurnaceSlot {
        count: 8,
        item_id: 7,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
        components: None,
    };
    storage.set_hopper_block_entity(hopper_pos, hopper).unwrap();
    storage
        .set_chest_block_entity(chest_pos, ChestBlockEntity::default())
        .unwrap();
    storage
        .set_chest_block_entity(partner_pos, ChestBlockEntity::default())
        .unwrap();
    let snapshot = storage
        .read_view()
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }]);
    // Sanity: the two chest states really pair under the sim rule.
    let paired = mc_simulation::block_placement::chest::paired_position(
        &blocks,
        |position| snapshot.get_cached_block(position),
        chest_pos,
        BlockStateId(2),
    );
    assert_eq!(paired, Some(partner_pos));
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
    let tick = ScheduledBlockTick::new(
        hopper_pos,
        Identifier::parse("minecraft:hopper").unwrap(),
        20,
        0,
    );
    let union = vec![ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }];
    let planned = plan_resident_hopper_transfer(&context, &blocks, &snapshot, &union, &tick, 20)
        .expect("corner double-chest hopper tick plans");
    // Movement first: without it the comparator assertion below is a no-op
    // run, not a regression proof.
    assert!(
        planned.result.moved,
        "stocked hopper ejects one item into the corner pair"
    );
    // The comparator refresh for the pair must be scheduled: without the
    // partner's far-side chunk staged, the refresh is silently skipped.
    assert!(
        planned
            .plan
            .scheduled_block_ticks
            .iter()
            .any(|scheduled| scheduled.pos == comparator_pos),
        "comparator beside the corner partner must get a tick, got {:?}",
        planned.plan.scheduled_block_ticks,
    );
}
