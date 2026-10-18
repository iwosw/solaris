//! Obsidian rectangle authority; corners are optional, interiors are 2..21 by 3..21.
use super::{named, state_id, write};
use crate::boundary::BlockPos;
use crate::dimensions::Dimension;
use crate::tick::TickState;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame {
    pub bottom: BlockPos,
    pub width: i32,
    pub height: i32,
    pub x_axis: bool,
}
impl Frame {
    pub fn cell(self, column: i32, row: i32) -> BlockPos {
        BlockPos {
            x: self.bottom.x + if self.x_axis { column } else { 0 },
            y: self.bottom.y + row,
            z: self.bottom.z + if self.x_axis { 0 } else { column },
        }
    }
}
fn interior(state: &TickState, pos: BlockPos) -> bool {
    ["air", "cave_air", "void_air", "fire", "nether_portal"]
        .into_iter()
        .any(|name| named(state, pos, name))
}

pub(crate) fn frame(state: &TickState, pos: BlockPos, x_axis: bool) -> Option<Frame> {
    if state.dimension == Dimension::End || !interior(state, pos) {
        return None;
    }
    let mut shape = Frame {
        bottom: pos,
        width: 0,
        height: 0,
        x_axis,
    };
    for _ in 0..21 {
        let below = shape.cell(0, -1);
        if !interior(state, below) {
            break;
        }
        shape.bottom.y -= 1;
    }
    for _ in 0..21 {
        let left = shape.cell(-1, 0);
        if !interior(state, left) {
            break;
        }
        shape.bottom = left;
    }
    if !named(state, shape.cell(-1, 0), "obsidian") {
        return None;
    }
    for width in 0..=21 {
        if named(state, shape.cell(width, 0), "obsidian") {
            shape.width = width;
            break;
        }
        if !interior(state, shape.cell(width, 0))
            || !named(state, shape.cell(width, -1), "obsidian")
        {
            return None;
        }
    }
    if !(2..=21).contains(&shape.width) {
        return None;
    }
    for row in 0..=21 {
        if (0..shape.width).all(|column| named(state, shape.cell(column, row), "obsidian")) {
            shape.height = row;
            break;
        }
        if !named(state, shape.cell(-1, row), "obsidian")
            || !named(state, shape.cell(shape.width, row), "obsidian")
            || !(0..shape.width).all(|column| interior(state, shape.cell(column, row)))
        {
            return None;
        }
    }
    (3..=21).contains(&shape.height).then_some(shape)
}

pub(crate) fn ignite(
    state: &mut TickState,
    pos: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
) -> bool {
    let Some(shape) = frame(state, pos, true).or_else(|| frame(state, pos, false)) else {
        return false;
    };
    if (0..shape.height)
        .all(|y| (0..shape.width).all(|x| named(state, shape.cell(x, y), "nether_portal")))
    {
        return false;
    }
    let portal = state_id(
        state,
        "minecraft:nether_portal",
        &[("axis", if shape.x_axis { "x" } else { "z" })],
    );
    for y in 0..shape.height {
        for x in 0..shape.width {
            write(state, writes, shape.cell(x, y), portal);
        }
    }
    true
}

/// Recheck only the changed block and its six neighbours. A damaged rectangle
/// collapses as one block post-image set, never on a later unjournaled callback.
pub(crate) fn after_blocks(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>) {
    let changed: Vec<_> = writes.iter().map(|(pos, _)| *pos).collect();
    for pos in changed {
        if named(state, pos, "fire") {
            ignite(state, pos, writes);
        }
        for [dx, dy, dz] in [
            [0, 0, 0],
            [1, 0, 0],
            [-1, 0, 0],
            [0, 1, 0],
            [0, -1, 0],
            [0, 0, 1],
            [0, 0, -1],
        ] {
            let cell = BlockPos {
                x: pos.x + dx,
                y: pos.y + dy,
                z: pos.z + dz,
            };
            if !named(state, cell, "nether_portal") {
                continue;
            }
            let axis = state
                .world
                .block_at(cell.x, cell.y, cell.z)
                .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                .is_some_and(|block| {
                    block
                        .properties
                        .iter()
                        .any(|(k, v)| k == "axis" && v == "x")
                });
            if frame(state, cell, axis).is_some_and(|shape| {
                (0..shape.height).all(|y| {
                    (0..shape.width).all(|x| named(state, shape.cell(x, y), "nether_portal"))
                })
            }) {
                continue;
            }
            let mut pending = vec![cell];
            let air = state.registry.states.air;
            while let Some(cell) = pending.pop() {
                if !named(state, cell, "nether_portal") {
                    continue;
                }
                write(state, writes, cell, air);
                for [dx, dy, dz] in if axis {
                    [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0]]
                } else {
                    [[0, 0, 1], [0, 0, -1], [0, 1, 0], [0, -1, 0]]
                } {
                    let next = BlockPos {
                        x: cell.x + dx,
                        y: cell.y + dy,
                        z: cell.z + dz,
                    };
                    if (next.x - pos.x).abs() <= 22
                        && (next.z - pos.z).abs() <= 22
                        && (next.y - pos.y).abs() <= 22
                    {
                        pending.push(next);
                    }
                }
            }
        }
    }
}

/// POIs are derived from admitted block state. No stale link cache survives a
/// broken frame or restart. Vanilla uses 16 blocks in the Nether, 128 outside.
pub(crate) fn search(state: &TickState, target: [f64; 3], radius: i32) -> Option<Frame> {
    let portal_x = state_id(state, "minecraft:nether_portal", &[("axis", "x")]);
    let portal_z = state_id(state, "minecraft:nether_portal", &[("axis", "z")]);
    let mut best: Option<(f64, i32, Frame)> = None;
    for chunk in state.world.chunks() {
        if (chunk.pos.x * 16 - target[0].floor() as i32).abs() > radius + 15
            || (chunk.pos.z * 16 - target[2].floor() as i32).abs() > radius + 15
        {
            continue;
        }
        for (section_y, section) in chunk.sections.iter().enumerate() {
            if section.palette().map_or_else(
                || ![portal_x, portal_z].contains(&section.get(0, 0, 0).0),
                |palette| {
                    !palette
                        .iter()
                        .any(|id| [portal_x, portal_z].contains(&id.0))
                },
            ) {
                continue;
            }
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        let id = section.get(x, y, z).0;
                        if id != portal_x && id != portal_z {
                            continue;
                        }
                        let pos = BlockPos {
                            x: chunk.pos.x * 16 + i32::from(x),
                            y: chunk.geometry().min_y() + section_y as i32 * 16 + i32::from(y),
                            z: chunk.pos.z * 16 + i32::from(z),
                        };
                        if (f64::from(pos.x) - target[0]).abs() > f64::from(radius)
                            || (f64::from(pos.z) - target[2]).abs() > f64::from(radius)
                        {
                            continue;
                        }
                        if !named(state, pos, "nether_portal") {
                            continue;
                        }
                        let distance = (f64::from(pos.x) - target[0]).powi(2)
                            + (f64::from(pos.y) - target[1]).powi(2)
                            + (f64::from(pos.z) - target[2]).powi(2);
                        if best.as_ref().is_some_and(|(d, by, _)| {
                            distance > *d || (distance == *d && pos.y >= *by)
                        }) {
                            continue;
                        }
                        if let Some(shape) = frame(state, pos, id == portal_x) {
                            best = Some((distance, pos.y, shape));
                        }
                    }
                }
            }
        }
    }
    best.map(|(_, _, shape)| shape)
}

fn clear(state: &TickState, pos: BlockPos) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| matches!(block.block.id.path(), "air" | "cave_air" | "void_air"))
}
fn solid(state: &TickState, pos: BlockPos) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .is_some_and(|id| state.registry.materials.classify(id) == mc_physics::BlockMaterial::Solid)
}

pub(crate) fn create(
    state: &mut TickState,
    target: [f64; 3],
    x_axis: bool,
    writes: &mut Vec<(BlockPos, u32)>,
) -> Frame {
    let geometry = state.dimension.geometry();
    let max_y = if state.dimension == Dimension::Nether {
        127
    } else {
        geometry.max_y()
    };
    let base = BlockPos {
        x: target[0].floor() as i32,
        y: target[1].floor() as i32,
        z: target[2].floor() as i32,
    };
    let mut best: Option<(f64, Frame)> = None;
    let mut narrow: Option<(f64, Frame)> = None;
    for dx in -16..=16 {
        for dz in -16..=16 {
            for y in (geometry.min_y() + 1..max_y - 3).rev() {
                let shape = Frame {
                    bottom: BlockPos {
                        x: base.x + dx,
                        y,
                        z: base.z + dz,
                    },
                    width: 2,
                    height: 3,
                    x_axis,
                };
                if !(-1..=2).all(|x| {
                    solid(state, shape.cell(x, -1))
                        && (0..=3).all(|y| clear(state, shape.cell(x, y)))
                }) {
                    continue;
                }
                let distance = f64::from(dx * dx + dz * dz) + (f64::from(y) - target[1]).powi(2);
                if narrow.as_ref().is_none_or(|(d, _)| distance < *d) {
                    narrow = Some((distance, shape));
                }
                let free = [-1, 1].into_iter().all(|normal| {
                    (-1..=2).all(|x| {
                        (-1..=3).all(|y| {
                            let mut pos = shape.cell(x, y);
                            if x_axis {
                                pos.z += normal;
                            } else {
                                pos.x += normal;
                            }
                            if y == -1 {
                                solid(state, pos)
                            } else {
                                clear(state, pos)
                            }
                        })
                    })
                });
                if free && best.as_ref().is_none_or(|(d, _)| distance < *d) {
                    best = Some((distance, shape));
                }
            }
        }
    }
    let natural = best.or(narrow);
    let fallback = natural.is_none();
    let shape = natural.map(|(_, shape)| shape).unwrap_or(Frame {
        bottom: BlockPos {
            y: base.y.clamp(70, max_y - 10),
            ..base
        },
        width: 2,
        height: 3,
        x_axis,
    });
    let obsidian = state_id(state, "minecraft:obsidian", &[]);
    if fallback {
        for normal in -1..=1 {
            for x in 0..=1 {
                for y in -1..=2 {
                    let mut pos = shape.cell(x, y);
                    if x_axis {
                        pos.z += normal;
                    } else {
                        pos.x += normal;
                    }
                    write(
                        state,
                        writes,
                        pos,
                        if y == -1 {
                            obsidian
                        } else {
                            state.registry.states.air
                        },
                    );
                }
            }
        }
    }
    for y in -1..=3 {
        for x in -1..=2 {
            if y == -1 || y == 3 || x == -1 || x == 2 {
                write(state, writes, shape.cell(x, y), obsidian);
            }
        }
    }
    ignite(state, shape.bottom, writes);
    shape
}
