//! Farmer work: vanilla farmers harvest mature crops, replant them and bake
//! bread from wheat. That produce is the village food supply that willingness
//! to breed (`brain::willing`) and `share_food` consume.
//!
//! A farmer bound to a certified Farmer workshop walks its farm: a resumable
//! cursor sweeps the certificate volume for the nearest mature crop, the
//! selected crop becomes the normal navigation target, and it is broken and
//! replanted within [`FARM_REACH`]. Unbound farmers keep the original
//! [`REACH`]-block local scan.
use super::{Activity, VillagerProfession, VillagerState};
use crate::boundary::{BlockChange, BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::settlements::buildings::{self, BuildingKind};
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_script::ScriptWorldBounds;

/// Block scan radius of an unbound farmer around its feet.
const REACH: i32 = 4;
/// Certificate cells probed per brain decision; the cursor lives in [`Progress`].
pub(super) const SCAN_PROBES: u32 = 128;
/// A bound farmer breaks its selected crop only from this distance.
const FARM_REACH: f64 = 2.0;
/// Ticks between harvest attempts of one working farmer.
const PERIOD: u64 = 40;
const WHEAT_PER_BREAD: i32 = 3;

/// Resumable certified-farm sweep: cursor over the certificate volume, the
/// nearest mature crop of the current sweep, and the crop being walked to.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Progress {
    pub building: u64,
    pub revision: u64,
    pub bounds: Option<ScriptWorldBounds>,
    pub cursor: u32,
    pub origin: Option<[i32; 3]>,
    pub best: Option<(i64, BlockPos)>,
    pub target: Option<BlockPos>,
}

/// The crop a certified farmer is walking to, if any. The brain forwards it as
/// the ordinary worker navigation target.
pub(super) fn target(v: &VillagerState) -> Option<BlockPos> {
    v.harvest_progress.target
}

/// Certificate extents, bounded exactly like certification: at most 64 blocks
/// per axis and 32768 cells, so one sweep is finite and cheap.
fn dimensions(bounds: ScriptWorldBounds) -> Option<[u32; 3]> {
    let mut dims = [0u32; 3];
    for (axis, dim) in dims.iter_mut().enumerate() {
        let span = i64::from(bounds.max[axis]) - i64::from(bounds.min[axis]) + 1;
        *dim = u32::try_from(span).ok()?;
        if *dim == 0 || *dim > 64 {
            return None;
        }
    }
    (u64::from(dims[0]) * u64::from(dims[1]) * u64::from(dims[2]) <= 32768).then_some(dims)
}

fn cell(bounds: ScriptWorldBounds, dims: [u32; 3], index: u32) -> BlockPos {
    let plane = dims[1] * dims[2];
    let x = index / plane;
    let rest = index % plane;
    BlockPos {
        x: bounds.min[0] + x as i32,
        y: bounds.min[1] + (rest % dims[1]) as i32,
        z: bounds.min[2] + (rest / dims[1]) as i32,
    }
}

fn distance_sq(origin: [i32; 3], pos: BlockPos) -> i64 {
    [pos.x, pos.y, pos.z]
        .into_iter()
        .zip(origin)
        .map(|(p, o)| i64::from(p) - i64::from(o))
        .map(|d| d * d)
        .sum()
}

fn inside(bounds: ScriptWorldBounds, pos: BlockPos) -> bool {
    [pos.x, pos.y, pos.z]
        .into_iter()
        .enumerate()
        .all(|(axis, value)| (bounds.min[axis]..=bounds.max[axis]).contains(&value))
}

/// Probe at most [`SCAN_PROBES`] certificate cells from the cursor, keeping the
/// nearest mature crop of the sweep measured from its origin. Returns whether
/// the whole volume has now been swept.
fn sweep(
    state: &TickState,
    v: &mut VillagerState,
    bounds: ScriptWorldBounds,
    dims: [u32; 3],
    feet: BlockPos,
) -> bool {
    let total = dims[0] * dims[1] * dims[2];
    if v.harvest_progress.cursor == 0 {
        v.harvest_progress.origin = Some([feet.x, feet.y, feet.z]);
        v.harvest_progress.best = None;
    }
    let origin = v
        .harvest_progress
        .origin
        .unwrap_or([feet.x, feet.y, feet.z]);
    let mut cursor = v.harvest_progress.cursor;
    let end = cursor.saturating_add(SCAN_PROBES).min(total);
    while cursor < end {
        let pos = cell(bounds, dims, cursor);
        cursor += 1;
        let distance = distance_sq(origin, pos);
        if v.harvest_progress
            .best
            .is_some_and(|(best, _)| best <= distance)
        {
            continue;
        }
        if mature_crop(state, pos).is_some() {
            v.harvest_progress.best = Some((distance, pos));
        }
    }
    v.harvest_progress.cursor = cursor;
    cursor == total
}

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
/// Returns the crop a certified farmer is walking to.
pub(super) fn work(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
) -> Option<BlockPos> {
    if v.data.profession != VillagerProfession::Farmer
        || v.age < 0
        || v.sleeping
        || v.activity != Activity::Work
        || !crate::command_rules::mob_griefing(
            state,
            crate::command_rules::MobGrief::VillagerFarming,
        )
    {
        v.harvest_progress.target = None;
        v.harvest_progress.best = None;
        return None;
    }
    if v.settlement.is_none() {
        local(state, slot, v, changes);
        return None;
    }
    let Some(building) = buildings::building_for_job(state, v) else {
        v.harvest_progress.target = None;
        return None;
    };
    if building.kind != BuildingKind::Workshop(VillagerProfession::Farmer) {
        v.harvest_progress.target = None;
        return None;
    }
    let (id, revision, bounds) = (building.id.0, building.revision, building.bounds);
    certified(state, slot, v, changes, id, revision, bounds)
}

/// Unbound farmer: the original local scan around the feet, unchanged.
fn local(state: &mut TickState, slot: EntitySlot, v: &mut VillagerState, changes: &mut ChangeSet) {
    v.harvest_progress = Progress::default();
    if state.tick < v.harvest_at {
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
    harvest(state, v, changes, pos, item, count, replanted);
}

/// Certified farm: sweep the certificate volume with a bounded cursor, walk to
/// the nearest mature crop and break it only from [`FARM_REACH`]. The cursor,
/// nearest candidate of the current sweep and selected crop survive normal
/// navigation and invalidate together with the certificate identity.
fn certified(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
    id: u64,
    revision: u64,
    bounds: ScriptWorldBounds,
) -> Option<BlockPos> {
    let Some(dims) = dimensions(bounds) else {
        // An impossible certificate never leaves a stale crop target behind.
        v.harvest_progress = Progress::default();
        return None;
    };
    if v.harvest_progress.building != id
        || v.harvest_progress.revision != revision
        || v.harvest_progress.bounds != Some(bounds)
    {
        v.harvest_progress = Progress {
            building: id,
            revision,
            bounds: Some(bounds),
            ..Progress::default()
        };
    }
    // A broken, edited or re-certified crop releases the target and restarts
    // the sweep from the beginning.
    if let Some(target) = v.harvest_progress.target
        && (!inside(bounds, target) || mature_crop(state, target).is_none())
    {
        v.harvest_progress.target = None;
        v.harvest_progress.best = None;
        v.harvest_progress.cursor = 0;
    }
    if v.harvest_progress.target.is_none() && state.tick >= v.harvest_at {
        let feet = crate::settlements::allocator::at(state.entities.pos(slot));
        let complete = sweep(state, v, bounds, dims, feet);
        // Partial sweeps must not repeat on every brain decision.
        v.harvest_at = state.tick + PERIOD;
        if complete {
            v.harvest_progress.cursor = 0;
            v.harvest_progress.origin = None;
            v.harvest_progress.target = v.harvest_progress.best.take().map(|(_, pos)| pos);
        }
    }
    let target = v.harvest_progress.target?;
    if state.tick >= v.harvest_at && super::mining::near(state, slot, target, FARM_REACH) {
        v.harvest_at = state.tick + PERIOD;
        let Some((item, count, replanted)) = mature_crop(state, target) else {
            v.harvest_progress.target = None;
            v.harvest_progress.cursor = 0;
            return None;
        };
        if harvest(state, v, changes, target, item, count, replanted) {
            v.harvest_progress.target = None;
            v.harvest_progress.cursor = 0;
        }
    }
    Some(target)
}

/// Break and replant one mature crop, banking its whole yield and one net seed.
/// Returns whether the block changed.
fn harvest(
    state: &mut TickState,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
    pos: BlockPos,
    item: u32,
    count: i32,
    replanted: u32,
) -> bool {
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
        return false;
    }
    // One net seed after replant is a deterministic vanilla-attainable yield.
    // Keep it real stock so the settlement can seed its own future fields.
    let seed = state
        .registry
        .items
        .name_of(item)
        .and_then(|name| match name.path() {
            "wheat" => item_id(state, "wheat_seeds"),
            "beetroot" => item_id(state, "beetroot_seeds"),
            _ => None,
        })
        .filter(|_| v.settlement.is_some());
    let before = seed.map(|_| v.inventory.clone());
    super::brain::add_inventory(v, &mut produce, maximum);
    if let Some(seed) = seed {
        let mut seed = ItemStack::new(seed, 1);
        let maximum = state.registry.item_rules().max_stack(&seed);
        if super::brain::add_inventory(v, &mut seed, maximum) != 1 {
            v.inventory = before.expect("seed output has a transaction pre-image");
            return false;
        }
    }
    state.world.set_block(pos.x, pos.y, pos.z, replanted);
    changes.blocks.push(BlockChange {
        position: pos,
        state: replanted,
    });
    let (Some(wheat), Some(bread)) = (item_id(state, "wheat"), item_id(state, "bread")) else {
        return false;
    };
    let loaves = count_of(v, wheat) / WHEAT_PER_BREAD;
    let loaves = if v.settlement.is_some() {
        loaves.min((3 - count_of(v, bread)).max(0))
    } else {
        loaves
    };
    if loaves > 0 {
        let mut baked = ItemStack::new(bread, loaves);
        let maximum = state.registry.item_rules().max_stack(&baked);
        let stored = super::brain::add_inventory(v, &mut baked, maximum);
        take(v, wheat, stored * WHEAT_PER_BREAD);
    }
    true
}

/// Farmers keep the vanilla three-bread breeding ration and deliver surplus
/// produce through the same physical storage transactions as other gatherers.
pub(super) fn deposit(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
) -> Option<BlockPos> {
    if v.data.profession != VillagerProfession::Farmer
        || v.settlement.is_none()
        || v.age < 0
        || v.sleeping
        || v.activity == Activity::Panic
    {
        return None;
    }
    let bread = item_id(state, "bread")?;
    let mut needed = 3;
    let mut reserved: [Option<ItemStack>; 8] = std::array::from_fn(|_| None);
    for (index, stack) in v.inventory.iter_mut().enumerate() {
        if let Some(stack) = stack
            && stack.item_id == bread
            && needed > 0
        {
            let count = stack.count.min(needed);
            let mut ration = stack.clone();
            ration.count = count;
            reserved[index] = Some(ration);
            stack.count -= count;
            needed -= count;
        }
    }
    for stack in &mut v.inventory {
        if stack.as_ref().is_some_and(|stack| stack.count == 0) {
            *stack = None;
        }
    }
    let target = super::mining::deposit(state, slot, v);
    for mut ration in reserved.into_iter().flatten() {
        let maximum = state.registry.item_rules().max_stack(&ration);
        super::brain::add_inventory(v, &mut ration, maximum);
        debug_assert_eq!(ration.count, 0, "deposit only removes inventory");
    }
    target
}
