//! Dispenser construct completion: pumpkins/skulls into golem/wither bodies.

use crate::boundary::BlockPos;
use crate::tick::TickState;
use mc_data::ItemStack;

/// Complete a golem/wither construct at `target` with the dispenser's head
/// item. Returns true when the item was a construct head consumed (or
/// refused by an incomplete/invalid frame); false when not applicable.
pub(super) fn complete(
    state: &mut TickState,
    pos: BlockPos,
    target: BlockPos,
    index: usize,
    held: &ItemStack,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &[crate::boundary::EntityId],
) -> bool {
    let Some(item) = state.registry.items.name_of(held.item_id) else {
        return false;
    };
    let name = item.as_str();
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    let pumpkin = matches!(path, "carved_pumpkin" | "jack_o_lantern");
    let skull = matches!(path, "wither_skeleton_skull");
    if !pumpkin && !skull {
        return false;
    }
    // The head cell must be empty (the dispenser places the head there);
    // a wearable entity target instead takes the equipment path.
    let occupied = state.entities.slots().any(|slot| {
        let body = state.entities.body(slot);
        let at = state.entities.pos(slot);
        body.alive
            && (at[0] - (f64::from(target.x) + 0.5)).abs() < 0.6
            && (at[2] - (f64::from(target.z) + 0.5)).abs() < 0.6
            && at[1] < f64::from(target.y) + 1.0
            && at[1] + body.height > f64::from(target.y)
    });
    if occupied {
        return false;
    }
    let head_state = if pumpkin {
        state
            .registry
            .blocks
            .block(&mc_data::Identifier::parse("minecraft:carved_pumpkin").expect("pumpkin block"))
    } else {
        state.registry.blocks.block(
            &mc_data::Identifier::parse("minecraft:wither_skeleton_skull").expect("skull block"),
        )
    };
    let Some(head_state) = head_state else {
        return false;
    };
    // Place the head, then run the shared construction check so a completed
    // frame summons (or a Peaceful wither refuses) exactly like manual play.
    // Snapshot the write log: on failure every entry appended below (mine and
    // any from after_blocks) is truncated and the cell restored, so the
    // caller's footprint stays consistent and no head block remains.
    let writes_len = writes.len();
    state
        .world
        .set_block(target.x, target.y, target.z, head_state.default.0);
    writes.push((target, head_state.default.0));
    let before = spawned.len();
    crate::golems::after_blocks(state, writes);
    crate::special_mobs::after_blocks(state, writes);
    // Attribute the summon to THIS dispenser: the spawned entity must
    // appear near the target cell (after_blocks scans all writes and could
    // otherwise credit an unrelated frame completed by the same tick).
    let summoned = spawned[before..].iter().any(|id| {
        state.entities.slot_of(*id).is_some_and(|slot| {
            let at = state.entities.pos(slot);
            (at[0] - (f64::from(target.x) + 0.5)).abs() < 4.0
                && (at[2] - (f64::from(target.z) + 0.5)).abs() < 4.0
        })
    });
    // Count golem summons routed to hostile.spawned instead.
    let summoned = summoned || {
        let before_hostile = state.hostile.spawned.len();
        let _ = before_hostile;
        // after_blocks pushed into hostile.spawned on success; detect by
        // checking the head cell was consumed into air.
        state
            .world
            .block_at(target.x, target.y, target.z)
            .is_some_and(|id| {
                state.registry.materials.classify(id) == mc_physics::BlockMaterial::Air
            })
    };
    if summoned {
        let key = (pos.x, pos.y, pos.z);
        mc_data::inventory_semantics_26_1_2::decrement_stack(
            &mut state
                .block_entities
                .get_mut(&key)
                .expect("construct custody")
                .slots[index],
        );
    } else {
        // Incomplete frame: truncate every speculative entry and restore the
        // original cell (a dispenser ejects the head instead of placing it —
        // returning false routes to the item-ejection path).
        writes.truncate(writes_len);
        state
            .world
            .set_block(target.x, target.y, target.z, state.registry.states.air);
    }
    summoned
}
