//! Physical settlement extraction: a resumable staircase followed by 1x2
//! serpentine branches. One block is removed per work attempt; probes are fixed.
use super::{Activity, VillagerProfession, VillagerState};
use crate::boundary::{BlockChange, BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::settlements::{buildings, inventory};
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_script::ScriptWorldBounds;

const PERIOD: u64 = 20;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub building: u64,
    pub revision: u64,
    pub bounds: Option<ScriptWorldBounds>,
    pub last_open: Option<[i32; 3]>,
    pub lane: u32,
    pub cell: u32,
    pub layer: u8,
    pub returning: bool,
}

pub(super) fn item_id(state: &TickState, name: &str) -> Option<u32> {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).ok()?)
}

pub(super) fn room(state: &TickState, v: &VillagerState, stack: &ItemStack) -> i32 {
    let maximum = state.registry.item_rules().max_stack(stack);
    v.inventory
        .iter()
        .map(|slot| match slot {
            None => maximum,
            Some(existing) if mc_data::inventory_semantics_26_1_2::can_stack(existing, stack) => {
                (maximum - existing.count).max(0)
            }
            _ => 0,
        })
        .sum()
}

pub(super) fn collect(state: &TickState, v: &mut VillagerState, item: u32, count: i32) -> bool {
    let mut stack = ItemStack::new(item, count);
    if room(state, v, &stack) < count {
        return false;
    }
    let maximum = state.registry.item_rules().max_stack(&stack);
    super::brain::add_inventory(v, &mut stack, maximum);
    true
}

pub(super) fn replace(state: &mut TickState, changes: &mut ChangeSet, pos: BlockPos, id: u32) {
    state.world.set_block(pos.x, pos.y, pos.z, id);
    changes.blocks.push(BlockChange {
        position: pos,
        state: id,
    });
}

pub(super) fn near(state: &TickState, slot: EntitySlot, pos: BlockPos, reach: f64) -> bool {
    let p = state.entities.pos(slot);
    (p[0] - f64::from(pos.x) - 0.5).powi(2)
        + (p[1] - f64::from(pos.y)).powi(2)
        + (p[2] - f64::from(pos.z) - 0.5).powi(2)
        <= reach * reach
}

pub(super) fn needs_deposit(v: &VillagerState) -> bool {
    v.inventory
        .iter()
        .all(|slot| slot.as_ref().is_some_and(|s| s.count >= 64))
}

/// Container fences, reservation floors, double chests, viewers and the block
/// entity WAL all go through the existing native inventory authority. Carry is
/// detached by the brain; only remove it after the container commit succeeds.
pub(super) fn deposit(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
) -> Option<BlockPos> {
    if v.inventory.iter().all(Option::is_none) {
        return None;
    }
    let settlement = v.settlement?;
    let points = buildings::storage_points(state, settlement);
    let mut destination = None;
    for position in points {
        let endpoint = inventory::Endpoint::Container {
            dimension: state.dimension.name().into(),
            position,
        };
        let Ok(before) = inventory::load(state, &endpoint) else {
            continue;
        };
        let mut after = before.slots.clone();
        let mut carried = v.inventory.clone();
        let mut moved = false;
        for stack in &mut carried {
            let Some(moving) = stack else {
                continue;
            };
            let maximum = state.registry.item_rules().max_stack(moving);
            // Merge first, then use empty slots: never overwrite unrelated stock.
            for empty in [false, true] {
                for stored in &mut after {
                    if moving.count == 0 {
                        break;
                    }
                    if stored.is_empty() != empty {
                        continue;
                    }
                    if !stored.is_empty()
                        && !mc_data::inventory_semantics_26_1_2::can_stack(stored, moving)
                    {
                        continue;
                    }
                    let count = moving
                        .count
                        .min((maximum - if stored.is_empty() { 0 } else { stored.count }).max(0));
                    if count == 0 {
                        continue;
                    }
                    if stored.is_empty() {
                        *stored = moving.clone();
                        stored.count = 0;
                    }
                    stored.count += count;
                    moving.count -= count;
                    moved = true;
                }
            }
            if moving.count == 0 {
                *stack = None;
            }
        }
        if !moved {
            continue;
        }
        let pos = BlockPos {
            x: position[0],
            y: position[1],
            z: position[2],
        };
        if !near(state, slot, pos, 2.5) {
            let p = state.entities.pos(slot);
            let distance = (p[0] - f64::from(pos.x)).powi(2)
                + (p[1] - f64::from(pos.y)).powi(2)
                + (p[2] - f64::from(pos.z)).powi(2);
            if destination.is_none_or(|(_, best)| distance < best) {
                destination = Some((pos, distance));
            }
            continue;
        }
        if inventory::commit(state, vec![inventory::Change { before, after }]).is_ok() {
            v.inventory = carried;
            return if v.inventory.iter().all(Option::is_none) {
                None
            } else {
                Some(pos)
            };
        }
    }
    destination.map(|(pos, _)| pos)
}

fn inside(bounds: ScriptWorldBounds, p: BlockPos) -> bool {
    [p.x, p.y, p.z]
        .into_iter()
        .zip(bounds.min.into_iter().zip(bounds.max))
        .all(|(n, (min, max))| n >= min && n <= max)
}

/// O(1) cursor decoding: descend along X, extend to the wall, then alternate
/// parallel X branches separated by three Z blocks, joined at their ends.
fn cell(bounds: ScriptWorldBounds, entrance: BlockPos, index: u32) -> Option<BlockPos> {
    let left = bounds.min[0] + 1;
    let right = bounds.max[0] - 1;
    let direction = if right - entrance.x >= entrance.x - left {
        1
    } else {
        -1
    };
    let edge = if direction > 0 { right } else { left };
    let run = (edge - entrance.x).unsigned_abs();
    if run == 0 || entrance.y <= bounds.min[1] + 1 {
        return None;
    }
    let depth = (entrance.y - bounds.min[1] - 1).min(run as i32);
    let p = if index < run {
        BlockPos {
            x: entrance.x + direction * (index as i32 + 1),
            y: entrance.y - (index as i32 + 1).min(depth),
            z: entrance.z,
        }
    } else {
        let width = right - left;
        if width <= 0 {
            return None;
        }
        let n = index - run;
        let stride = width as u32 + 3;
        let row = n / stride;
        if row > (bounds.max[2] - bounds.min[2]) as u32 {
            return None;
        }
        let offset = n % stride;
        let z_direction = if bounds.max[2] - 1 - entrance.z >= entrance.z - bounds.min[2] - 1 {
            1
        } else {
            -1
        };
        let from = if row.is_multiple_of(2) {
            edge
        } else if edge == right {
            left
        } else {
            right
        };
        let across = if from == right { -1 } else { 1 };
        BlockPos {
            x: from + across * (offset as i32 - 2).max(0),
            y: entrance.y - depth,
            z: entrance.z + z_direction * (row as i32 * 3 + (offset as i32 + 1).min(3)),
        }
    };
    (inside(bounds, p) && p.z > bounds.min[2] && p.z < bounds.max[2] && p.y + 2 <= bounds.max[1])
        .then_some(p)
}

fn block_path(state: &TickState, pos: BlockPos) -> Option<&str> {
    let id = state.world.block_at(pos.x, pos.y, pos.z)?;
    Some(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(id))?
            .block
            .id
            .path(),
    )
}

fn natural(path: &str) -> bool {
    matches!(
        path,
        "stone"
            | "deepslate"
            | "granite"
            | "diorite"
            | "andesite"
            | "tuff"
            | "dirt"
            | "gravel"
            | "sand"
            | "calcite"
    ) || path.ends_with("_ore")
}

fn drop_name(path: &str) -> &str {
    match path {
        "stone" => "cobblestone",
        "deepslate" => "cobbled_deepslate",
        "coal_ore" | "deepslate_coal_ore" => "coal",
        "iron_ore" | "deepslate_iron_ore" => "raw_iron",
        "copper_ore" | "deepslate_copper_ore" => "raw_copper",
        "gold_ore" | "deepslate_gold_ore" => "raw_gold",
        "diamond_ore" | "deepslate_diamond_ore" => "diamond",
        "emerald_ore" | "deepslate_emerald_ore" => "emerald",
        "redstone_ore" | "deepslate_redstone_ore" => "redstone",
        "lapis_ore" | "deepslate_lapis_ore" => "lapis_lazuli",
        other => other,
    }
}

fn clear(path: &str) -> bool {
    matches!(path, "air" | "cave_air" | "void_air")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Obstruction {
    Unloaded,
    Permanent,
}

fn probe(state: &TickState, p: BlockPos) -> Result<(u32, &str), Obstruction> {
    let id = state
        .world
        .block_at(p.x, p.y, p.z)
        .ok_or(Obstruction::Unloaded)?;
    let path = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .ok_or(Obstruction::Permanent)?
        .block
        .id
        .path();
    Ok((id, path))
}

fn unsafe_fluid(state: &TickState, id: u32, path: &str) -> bool {
    crate::fluids::contained(state, id).is_some()
        || matches!(
            path,
            "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass" | "bubble_column" | "powder_snow"
        )
}

/// Reject fluids, voids and caves before opening any face. Missing chunks
/// pause the same cell; permanent obstructions abandon this staircase lane.
fn safe(
    state: &TickState,
    bounds: ScriptWorldBounds,
    entrance: BlockPos,
    current: BlockPos,
    previous: BlockPos,
    height: i32,
    layer: u8,
) -> Result<(), Obstruction> {
    for y in current.y..current.y + height {
        let p = BlockPos { y, ..current };
        if !inside(bounds, p) {
            return Err(Obstruction::Permanent);
        }
        let (id, path) = probe(state, p)?;
        if unsafe_fluid(state, id, path) {
            return Err(Obstruction::Permanent);
        }
        let already = y > current.y + height - 1 - i32::from(layer);
        let surface_air = clear(path) && y >= entrance.y;
        if (already && !clear(path)) || (!already && !surface_air && !natural(path)) {
            return Err(Obstruction::Permanent);
        }
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let neighbor = BlockPos {
                x: p.x + dx,
                y: p.y + dy,
                z: p.z + dz,
            };
            if neighbor.x == current.x
                && neighbor.z == current.z
                && (current.y..current.y + height).contains(&neighbor.y)
            {
                continue;
            }
            let (id, path) = probe(state, neighbor)?;
            if unsafe_fluid(state, id, path)
                || (clear(path)
                    && neighbor.y < entrance.y
                    && !(neighbor.x == previous.x
                        && neighbor.z == previous.z
                        && (previous.y..previous.y + 3).contains(&neighbor.y)))
            {
                return Err(Obstruction::Permanent);
            }
        }
    }
    Ok(())
}

/// Each permanently blocked branch retries a different surface entrance column,
/// without treating the skipped solid cell as a traversable route waypoint.
fn lane_entrance(bounds: ScriptWorldBounds, entrance: BlockPos, lane: u32) -> Option<BlockPos> {
    let width = bounds.max[2] - bounds.min[2] - 1;
    if width <= 0 || lane >= width as u32 {
        return None;
    }
    let offset = (entrance.z - bounds.min[2] - 1).clamp(0, width - 1);
    Some(BlockPos {
        z: bounds.min[2] + 1 + (offset + lane as i32) % width,
        ..entrance
    })
}

pub(super) fn work(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
) -> Option<BlockPos> {
    if v.data.profession != VillagerProfession::Miner
        || v.age < 0
        || v.sleeping
        || v.activity == Activity::Panic
    {
        return None;
    }
    let building = buildings::building_for_job(state, v)?;
    if building.kind != buildings::BuildingKind::Mine {
        return None;
    }
    let (id, revision, bounds) = (building.id.0, building.revision, building.bounds);
    let entrance = v.job_site?;
    if v.mining_progress.building != id || v.mining_progress.bounds != Some(bounds) {
        v.mining_progress = Progress {
            building: id,
            revision,
            bounds: Some(bounds),
            ..Progress::default()
        };
    }
    v.mining_progress.revision = revision;
    let working =
        v.activity == Activity::Work && (2_000..9_000).contains(&state.day_time.rem_euclid(24_000));
    if !working || needs_deposit(v) {
        v.mining_progress.returning = true;
    }
    if v.mining_progress.returning {
        let target = deposit(state, slot, v);
        v.mining_progress.returning = v.inventory.iter().any(Option::is_some);
        return target;
    }
    if !working || !crate::command_rules::boolean(state, "mob_griefing") {
        return None;
    }
    let origin = lane_entrance(bounds, entrance, v.mining_progress.lane)?;
    let current = cell(bounds, origin, v.mining_progress.cell)?;
    let previous = v
        .mining_progress
        .last_open
        .map(|[x, y, z]| BlockPos { x, y, z })
        .unwrap_or(origin);
    let planned_previous = v
        .mining_progress
        .cell
        .checked_sub(1)
        .and_then(|n| cell(bounds, origin, n))
        .unwrap_or(origin);
    let height = if current.y < planned_previous.y { 3 } else { 2 };
    if i32::from(v.mining_progress.layer) >= height {
        return None;
    }
    if !near(state, slot, current, 3.0) {
        return Some(previous);
    }
    if state.tick < v.harvest_at {
        return Some(previous);
    }
    v.harvest_at = state.tick + PERIOD;
    match safe(
        state,
        bounds,
        origin,
        current,
        previous,
        height,
        v.mining_progress.layer,
    ) {
        Ok(()) => {}
        Err(Obstruction::Unloaded) => return Some(previous),
        Err(Obstruction::Permanent) => {
            v.mining_progress.lane += 1;
            v.mining_progress.cell = 0;
            v.mining_progress.layer = 0;
            v.mining_progress.last_open = None;
            return Some(previous);
        }
    }
    let pos = BlockPos {
        y: current.y + height - 1 - i32::from(v.mining_progress.layer),
        ..current
    };
    let path = block_path(state, pos)?;
    if !clear(path) {
        let item = item_id(state, drop_name(path))?;
        if room(state, v, &ItemStack::new(item, 1)) < 1 {
            v.mining_progress.returning = true;
            return deposit(state, slot, v);
        }
        let air = state
            .registry
            .blocks
            .block(&mc_data::Identifier::parse("minecraft:air").ok()?)?
            .default
            .0;
        collect(state, v, item, 1);
        replace(state, changes, pos, air);
    }
    v.mining_progress.layer += 1;
    if i32::from(v.mining_progress.layer) == height {
        v.mining_progress.layer = 0;
        v.mining_progress.cell += 1;
        v.mining_progress.last_open = Some([current.x, current.y, current.z]);
        return Some(current);
    }
    Some(previous)
}
