//! Registry-exact block states and checked redstone geometry.
use mc_world::{BlockPos, BlockState};

pub(super) const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];
pub(super) const HORIZONTAL: [(&str, (i32, i32, i32)); 4] = [
    ("north", (0, 0, -1)),
    ("south", (0, 0, 1)),
    ("west", (-1, 0, 0)),
    ("east", (1, 0, 0)),
];
pub(super) fn offset(pos: BlockPos, d: (i32, i32, i32)) -> Option<BlockPos> {
    Some(BlockPos {
        x: pos.x.checked_add(d.0)?,
        y: pos.y.checked_add(d.1)?,
        z: pos.z.checked_add(d.2)?,
    })
}
pub(super) fn facing_delta(facing: &str) -> Option<(i32, i32, i32)> {
    Some(match facing {
        "east" => (1, 0, 0),
        "west" => (-1, 0, 0),
        "up" => (0, 1, 0),
        "down" => (0, -1, 0),
        "south" => (0, 0, 1),
        "north" => (0, 0, -1),
        _ => return None,
    })
}
pub(super) fn is_air(state: &BlockState) -> bool {
    matches!(state.block.id.path(), "air" | "cave_air" | "void_air")
}
