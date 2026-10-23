//! Track endpoints, local shape updates and bounded powered-rail propagation.
use super::{at, change, offset, power_at, prop};
use crate::tick::TickState;
use mc_world::BlockPos;

type End = (i32, i32, i32);
pub(super) fn is_rail(path: &str) -> bool {
    matches!(
        path,
        "rail" | "powered_rail" | "detector_rail" | "activator_rail"
    )
}

fn ends(shape: &str) -> [End; 2] {
    match shape {
        "east_west" => [(-1, 0, 0), (1, 0, 0)],
        "ascending_east" => [(-1, 0, 0), (1, 1, 0)],
        "ascending_west" => [(-1, 1, 0), (1, 0, 0)],
        "ascending_north" => [(0, 1, -1), (0, 0, 1)],
        "ascending_south" => [(0, 0, -1), (0, 1, 1)],
        "south_east" => [(0, 0, 1), (1, 0, 0)],
        "south_west" => [(0, 0, 1), (-1, 0, 0)],
        "north_west" => [(0, 0, -1), (-1, 0, 0)],
        "north_east" => [(0, 0, -1), (1, 0, 0)],
        _ => [(0, 0, -1), (0, 0, 1)],
    }
}

fn connected(state: &TickState, pos: BlockPos, end: End, kind: &str) -> Option<BlockPos> {
    let endpoint = offset(pos, end)?;
    // An endpoint can meet a rail at its own elevation, or the high end of
    // an ascending rail below it. Reciprocal endpoint heights must agree.
    for dy in [0, -1] {
        let candidate = offset(endpoint, (0, dy, 0))?;
        let Some(block) = at(state, candidate) else {
            continue;
        };
        if if kind.is_empty() {
            !is_rail(block.block.id.path())
        } else {
            block.block.id.path() != kind
        } {
            continue;
        }
        if ends(prop(block, "shape").unwrap_or("north_south"))
            .into_iter()
            .any(|back| back.0 == -end.0 && back.2 == -end.2 && candidate.y + back.1 == endpoint.y)
        {
            return Some(candidate);
        }
    }
    None
}

pub(super) fn powered(state: &TickState, pos: BlockPos) -> bool {
    if power_at(state, pos) > 0 {
        return true;
    }
    let Some(block) = at(state, pos) else {
        return false;
    };
    let kind = block.block.id.path();
    for first in ends(prop(block, "shape").unwrap_or("north_south")) {
        let mut previous = pos;
        let Some(mut cell) = connected(state, pos, first, kind) else {
            continue;
        };
        for _ in 0..8 {
            if power_at(state, cell) > 0 {
                return true;
            }
            let Some(block) = at(state, cell) else {
                break;
            };
            let next = ends(prop(block, "shape").unwrap_or("north_south"))
                .into_iter()
                .filter_map(|end| connected(state, cell, end, kind))
                .find(|next| *next != previous);
            let Some(next) = next else {
                break;
            };
            previous = cell;
            cell = next;
        }
    }
    false
}

fn nearby(state: &TickState, pos: BlockPos, d: End) -> Option<BlockPos> {
    [0, 1, -1]
        .into_iter()
        .filter_map(|dy| offset(pos, (d.0, dy, d.2)))
        .find(|cell| {
            let Some(block) = at(state, *cell) else {
                return false;
            };
            if !is_rail(block.block.id.path()) {
                return false;
            }
            let endpoints = ends(prop(block, "shape").unwrap_or("north_south"));
            endpoints.iter().any(|end| end.0 == -d.0 && end.2 == -d.2)
                || endpoints
                    .into_iter()
                    .filter(|end| connected(state, *cell, *end, "").is_some())
                    .count()
                    < 2
        })
}

pub(super) fn reshape(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>, pos: BlockPos) {
    let Some(block) = at(state, pos) else {
        return;
    };
    let path = block.block.id.path();
    if !is_rail(path) {
        return;
    }
    let old = match prop(block, "shape") {
        Some("east_west" | "ascending_east" | "ascending_west") => "east_west",
        Some("south_east") => "south_east",
        Some("south_west") => "south_west",
        Some("north_east") => "north_east",
        Some("north_west") => "north_west",
        _ => "north_south",
    };
    let n = nearby(state, pos, (0, 0, -1));
    let s = nearby(state, pos, (0, 0, 1));
    let w = nearby(state, pos, (-1, 0, 0));
    let e = nearby(state, pos, (1, 0, 0));
    let ns = n.is_some() || s.is_some();
    let ew = w.is_some() || e.is_some();
    let mut shape = if ns && !ew {
        "north_south"
    } else if ew && !ns {
        "east_west"
    } else {
        old
    };
    if path == "rail" {
        // The deterministic unpowered junction favors south/east; power flips
        // the selected branch. Straight-only rail families never form curves.
        let choices = if power_at(state, pos) > 0 {
            [
                (s.is_some() && e.is_some(), "south_east"),
                (s.is_some() && w.is_some(), "south_west"),
                (n.is_some() && e.is_some(), "north_east"),
                (n.is_some() && w.is_some(), "north_west"),
            ]
        } else {
            [
                (n.is_some() && w.is_some(), "north_west"),
                (n.is_some() && e.is_some(), "north_east"),
                (s.is_some() && w.is_some(), "south_west"),
                (s.is_some() && e.is_some(), "south_east"),
            ]
        };
        for (matches, candidate) in choices {
            if matches {
                shape = candidate;
            }
        }
    } else if ns && ew && !matches!(shape, "north_south" | "east_west") {
        shape = "north_south";
    }
    if matches!(shape, "north_south" | "ascending_north" | "ascending_south") {
        shape = "north_south";
        if n.is_some_and(|n| n.y > pos.y) {
            shape = "ascending_north";
        }
        if s.is_some_and(|s| s.y > pos.y) {
            shape = "ascending_south";
        }
    } else if matches!(shape, "east_west" | "ascending_east" | "ascending_west") {
        shape = "east_west";
        if e.is_some_and(|e| e.y > pos.y) {
            shape = "ascending_east";
        }
        if w.is_some_and(|w| w.y > pos.y) {
            shape = "ascending_west";
        }
    }
    change(state, writes, pos, "shape", shape);
}

pub(super) fn wake_diagonals(state: &mut TickState, pos: BlockPos) {
    for (_, d) in super::blocks::HORIZONTAL {
        for dy in [-1, 1] {
            if let Some(cell) = offset(pos, (d.0, dy, d.2))
                && at(state, cell).is_some_and(|b| is_rail(b.block.id.path()))
            {
                super::schedule_cell(state, cell, 1, 0);
            }
        }
    }
}
