//! Tick-owned wooden-door interaction. Only doors opened by this villager are
//! remembered; a player's open door is never claimed or toggled by the brain.
use super::VillagerState;
use crate::boundary::{BlockChange, BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::registry::CoreRegistry;
use crate::tick::TickState;
use mc_world::BlockStateId;

pub(crate) fn wooden(registry: &CoreRegistry, id: u32) -> bool {
    registry
        .wooden_doors
        .get(id as usize)
        .copied()
        .unwrap_or(false)
}

fn door(state: &TickState, pos: BlockPos) -> Option<(BlockPos, bool)> {
    let id = state.world.block_at(pos.x, pos.y, pos.z)?;
    if !wooden(&state.registry, id) {
        return None;
    }
    let block = state.registry.blocks.by_id(BlockStateId(id))?;
    let upper = block
        .properties
        .iter()
        .any(|(key, value)| key == "half" && value == "upper");
    let open = block
        .properties
        .iter()
        .any(|(key, value)| key == "open" && value == "true");
    Some((
        BlockPos {
            y: pos.y - i32::from(upper),
            ..pos
        },
        open,
    ))
}

fn set_open(state: &mut TickState, pos: BlockPos, open: bool, changes: &mut ChangeSet) {
    if door(state, pos).is_none_or(|(_, current)| current == open) {
        return;
    }
    if let Some(cells) = crate::placement::use_block(&state.registry.blocks, pos, 0.0, &|p| {
        state.world.block_at(p.x, p.y, p.z)
    }) {
        for (position, block) in cells {
            state
                .world
                .set_block(position.x, position.y, position.z, block);
            changes.blocks.push(BlockChange {
                position,
                state: block,
            });
        }
    }
}

fn needed_by_other(state: &TickState, opener: EntitySlot, lower: BlockPos) -> bool {
    let centre = [
        f64::from(lower.x) + 0.5,
        f64::from(lower.y),
        f64::from(lower.z) + 0.5,
    ];
    // Two path cells plus the waypoint arrival tolerance can span a chunk edge.
    super::brain::bodies_near(state, centre, 3.0, true).any(|other| {
        if other == opener {
            return false;
        }
        let body = state.entities.body(other);
        let Some(v) = state.entities.row(other).villager.as_ref() else {
            return false;
        };
        if !body.alive || v.conversion_time.is_some() {
            return false;
        }
        let p = state.entities.pos(other);
        let overlaps = (p[0] - centre[0]).abs() < 0.5 + body.half_width
            && (p[2] - centre[2]).abs() < 0.5 + body.half_width
            && p[1] < centre[1] + 2.0
            && p[1] + body.height > centre[1];
        overlaps
            || (!v.sleeping
                && v.navigation_path.iter().rev().take(2).any(|cell| {
                    cell.x == lower.x && cell.z == lower.z && cell.y.abs_diff(lower.y) <= 1
                }))
    })
}

pub(super) fn interact(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
) {
    let pos = state.entities.pos(slot);
    if !v.sleeping {
        // Look one step ahead as well: open before the collision box reaches
        // the door plane, not after physics has stopped against it.
        for index in 0..v.navigation_path.len().min(2) {
            let cell = v.navigation_path[v.navigation_path.len() - 1 - index];
            if (pos[0] - (f64::from(cell.x) + 0.5)).powi(2)
                + (pos[2] - (f64::from(cell.z) + 0.5)).powi(2)
                > 2.25
            {
                continue;
            }
            if let Some((lower, false)) = door(state, cell) {
                set_open(state, lower, true, changes);
                if !v.opened_doors.contains(&lower) {
                    v.opened_doors.push(lower);
                }
            }
        }
    }
    let width = state.entities.body(slot).half_width;
    v.opened_doors.retain(|&lower| {
        let Some((_, open)) = door(state, lower) else {
            return false;
        };
        if !open {
            return false;
        }
        // Do not swing the door into the villager while it occupies the cell.
        let in_door = (pos[0] - (f64::from(lower.x) + 0.5)).abs() < 0.5 + width
            && (pos[2] - (f64::from(lower.z) + 0.5)).abs() < 0.5 + width;
        let approaching = !v.sleeping
            && v.navigation_path.iter().rev().take(2).any(|cell| {
                cell.x == lower.x && cell.z == lower.z && cell.y.abs_diff(lower.y) <= 1
            });
        if in_door || approaching || needed_by_other(state, slot, lower) {
            return true;
        }
        set_open(state, lower, false, changes);
        false
    });
}
