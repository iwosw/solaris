//! Event-driven lid admission; no idle container/world scans.
use super::{BlockPos, TickState};

pub(super) fn can_open(state: &TickState, pos: BlockPos) -> bool {
    let Some(id) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
        return false;
    };
    match block.block.id.path() {
        "chest" | "trapped_chest" => super::container_cells(state, (pos.x, pos.y, pos.z))
            .into_iter()
            .all(|(x, y, z)| {
                state
                    .world
                    .block_at(x, y + 1, z)
                    .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                    .is_some_and(|above| !crate::redstone::conducts(above))
            }),
        name if name == "shulker_box" || name.ends_with("_shulker_box") => {
            let facing = block
                .properties
                .iter()
                .find(|(key, _)| key == "facing")
                .map(|(_, value)| value.as_str())
                .unwrap_or("up");
            let (axis, direction) = match facing {
                "down" => (1, -1),
                "north" => (2, -1),
                "south" => (2, 1),
                "west" => (0, -1),
                "east" => (0, 1),
                _ => (1, 1),
            };
            let mut adjacent = [pos.x, pos.y, pos.z];
            adjacent[axis] += direction;
            let Some(id) = state.world.block_at(adjacent[0], adjacent[1], adjacent[2]) else {
                return false;
            };
            let Some(shape) = mc_data::collision_shapes::vanilla_collision_shapes().get(id) else {
                return false;
            };
            // The lid sweeps half a block into the neighbouring cell. Shapes
            // touching its boundary are allowed; crossing it is an obstruction.
            !shape.iter().any(|bounds| {
                let bounds = bounds.as_blocks();
                if direction > 0 {
                    bounds[axis] < 0.5 && bounds[axis + 3] > 0.0
                } else {
                    bounds[axis + 3] > 0.5 && bounds[axis] < 1.0
                }
            })
        }
        _ => true, // Barrels and workstations do not require a free lid.
    }
}
