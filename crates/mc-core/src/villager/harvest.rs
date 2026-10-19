//! Farmer work: vanilla farmers harvest mature crops, replant them and bake
//! bread from wheat. That produce is the village food supply that willingness
//! to breed (`brain::willing`) and `share_food` consume.
//!
//! Simplification: the farmer reaches crops within [`REACH`] blocks of where it
//! stands during work hours instead of walking field rows.
use super::{Activity, VillagerProfession, VillagerState};
use crate::boundary::{BlockChange, BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::tick::TickState;
use mc_data::ItemStack;

const REACH: i32 = 4;
/// Ticks between harvest attempts of one working farmer.
const PERIOD: u64 = 40;
const WHEAT_PER_BREAD: i32 = 3;

/// Crop block path, its item, and the items one mature plant yields.
const CROPS: [(&str, &str, i32); 4] = [
    ("wheat", "wheat", 1),
    ("carrots", "carrot", 3),
    ("potatoes", "potato", 3),
    ("beetroots", "beetroot", 1),
];

fn item_id(state: &TickState, name: &str) -> Option<u32> {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).ok()?)
}

/// `(item, yield, replanted state)` when the block at `pos` is a mature crop.
fn mature_crop(state: &TickState, pos: BlockPos) -> Option<(u32, i32, u32)> {
    let id = state.world.block_at(pos.x, pos.y, pos.z)?;
    let path = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))?
        .block
        .id
        .path();
    let &(_, item, count) = CROPS.iter().find(|(crop, _, _)| *crop == path)?;
    let (age, maximum) = crate::farming::age_of(&state.registry.blocks, id)?;
    if age != maximum {
        return None;
    }
    let replanted = crate::farming::with_prop(&state.registry.blocks, id, "age", "0")?;
    Some((item_id(state, item)?, count, replanted))
}

fn count_of(v: &VillagerState, item: u32) -> i32 {
    v.inventory
        .iter()
        .flatten()
        .filter(|stack| stack.item_id == item)
        .map(|stack| stack.count)
        .sum()
}

fn take(v: &mut VillagerState, item: u32, mut count: i32) {
    for slot in &mut v.inventory {
        if let Some(stack) = slot
            && stack.item_id == item
        {
            let used = stack.count.min(count);
            stack.count -= used;
            count -= used;
            if stack.count == 0 {
                *slot = None;
            }
        }
        if count == 0 {
            break;
        }
    }
}

/// One harvest attempt per [`PERIOD`] for a working, awake farmer, scheduled
/// by a per-villager deadline so reduced brain cadences still harvest. A crop is
/// only broken when its whole yield fits the inventory, then replanted at age 0.
pub(super) fn work(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
) {
    if v.data.profession != VillagerProfession::Farmer
        || v.age < 0
        || v.sleeping
        || v.activity != Activity::Work
        || state.tick < v.harvest_at
        || !crate::command_rules::boolean(state, "mob_griefing")
    {
        return;
    }
    v.harvest_at = state.tick + PERIOD;
    let feet = crate::settlements::allocator::at(state.entities.pos(slot));
    let found = (-REACH..=REACH)
        .flat_map(|dx| (-1..=1).flat_map(move |dy| (-REACH..=REACH).map(move |dz| (dx, dy, dz))))
        .map(|(dx, dy, dz)| BlockPos {
            x: feet.x + dx,
            y: feet.y + dy,
            z: feet.z + dz,
        })
        .find_map(|pos| mature_crop(state, pos).map(|crop| (pos, crop)));
    let Some((pos, (item, count, replanted))) = found else {
        return;
    };
    let mut produce = ItemStack::new(item, count);
    let maximum = state.registry.item_rules().max_stack(&produce);
    let room: i32 = v
        .inventory
        .iter()
        .map(|slot| match slot {
            None => maximum,
            Some(existing)
                if mc_data::inventory_semantics_26_1_2::can_stack(existing, &produce) =>
            {
                (maximum - existing.count).max(0)
            }
            Some(_) => 0,
        })
        .sum();
    if room < count {
        return;
    }
    super::brain::add_inventory(v, &mut produce, maximum);
    state.world.set_block(pos.x, pos.y, pos.z, replanted);
    changes.blocks.push(BlockChange {
        position: pos,
        state: replanted,
    });
    let (Some(wheat), Some(bread)) = (item_id(state, "wheat"), item_id(state, "bread")) else {
        return;
    };
    let loaves = count_of(v, wheat) / WHEAT_PER_BREAD;
    if loaves > 0 {
        let mut baked = ItemStack::new(bread, loaves);
        let maximum = state.registry.item_rules().max_stack(&baked);
        let stored = super::brain::add_inventory(v, &mut baked, maximum);
        take(v, wheat, stored * WHEAT_PER_BREAD);
    }
}
