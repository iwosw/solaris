//! Dispenser vehicle placement: boats onto water, minecarts onto rails.

use crate::boundary::BlockPos;
use crate::tick::TickState;

/// Place the dispenser's boat/minecart at `target`. Returns true when the item
/// was a vehicle consumed by placement (or refused for an invalid target);
/// false when this behavior does not apply.
pub(super) fn place(
    state: &mut TickState,
    pos: BlockPos,
    target: BlockPos,
    index: usize,
    spawned: &mut Vec<crate::boundary::EntityId>,
) -> bool {
    let held = state.block_entities[&(pos.x, pos.y, pos.z)].slots[index].clone();
    let Some(item) = state.registry.items.name_of(held.item_id) else {
        return false;
    };
    let name = item.as_str();
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    let boat = path.ends_with("_boat") || path == "bamboo_raft" || path == "bamboo_chest_raft";
    let minecart = matches!(
        path,
        "minecart" | "chest_minecart" | "hopper_minecart" | "furnace_minecart" | "tnt_minecart"
    );
    if !boat && !minecart {
        return false;
    }
    let Some(type_id) =
        state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap_or_else(|_| {
                mc_data::Identifier::parse("minecraft:boat").expect("static identifier")
            }))
    else {
        return true;
    };
    // Dispenser faces the target cell directly (no clicked-face offset):
    // boats need water in the target cell; minecarts need a rail there.
    let at = if minecart {
        let Some(rail) = crate::vehicles::motion::rail_at(state, target) else {
            return true;
        };
        rail.point(0.5)
    } else {
        let Some(block) = state.world.block_at(target.x, target.y, target.z) else {
            return true;
        };
        let water = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(block))
            .is_some_and(|b| b.block.id.path() == "water");
        if !water {
            return true;
        }
        [
            f64::from(target.x) + 0.5,
            f64::from(target.y) + 0.8,
            f64::from(target.z) + 0.5,
        ]
    };
    let Some(bp) = state.registry.blueprint(type_id) else {
        return true;
    };
    if !crate::vehicles::motion::clear(state, at, bp.half_width, bp.height) {
        return true;
    }
    let id = state.spawn_mob(type_id, at, 4.0);
    let key = (pos.x, pos.y, pos.z);
    mc_data::inventory_semantics_26_1_2::decrement_stack(
        &mut state
            .block_entities
            .get_mut(&key)
            .expect("vehicle custody")
            .slots[index],
    );
    spawned.push(id);
    true
}
