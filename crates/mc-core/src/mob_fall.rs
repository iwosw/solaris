//! Vanilla `Entity.checkFallDamage` for mobs: a falling body accumulates its
//! drop, and the landing move owes `ceil(distance - 3)` fall damage.
//!
//! Called from the serial movement writeback with the pre/post positions, so
//! walkers on level ground (no drop, no ground transition) never touch the
//! cold row. The accumulated distance lives in `Controls::fall_distance`
//! (already persisted as `FallDistance`); it is written without a durable
//! mark because a transient mid-air value is not worth a WAL image per tick.

use crate::blueprint::MovementClass;
use crate::effects::EffectId;
use crate::entities::{EntityKind, EntitySlot};
use crate::tick::TickState;

/// Account one mob move; returns the fall damage owed on a landing.
///
/// `was_ground` is the support flag before the move, `ground` after it.
pub(crate) fn after_move(
    state: &mut TickState,
    slot: EntitySlot,
    old: [f64; 3],
    new: [f64; 3],
    was_ground: bool,
    ground: bool,
    wall_climbing: bool,
) -> f32 {
    // Vanilla LivingEntity.handleOnClimbable resets fall distance. A clipped
    // climb still counts: otherwise tiny ceiling drops accumulate across rises.
    if wall_climbing {
        state.entities.row_mut_unmarked(slot).controls.fall_distance = 0.0;
        return 0.0;
    }
    let dy = new[1] - old[1];
    if (was_ground || !ground) && dy >= 0.0 {
        return 0.0;
    }
    let body = state.entities.body(slot);
    let EntityKind::Mob { type_id } = body.kind else {
        return 0.0;
    };
    if matches!(
        body.movement,
        MovementClass::Flyer | MovementClass::Swimmer | MovementClass::Stationary
    ) || state
        .registry
        .blueprint(type_id)
        .is_some_and(|blueprint| blueprint.capabilities.fall_damage_immune)
    {
        return 0.0;
    }
    let feet = [
        new[0].floor() as i32,
        new[1].floor() as i32,
        new[2].floor() as i32,
    ];
    let cushioned = state
        .world
        .block_at(feet[0], feet[1], feet[2])
        .is_some_and(|id| {
            id == state.registry.states.water
                || state
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(id))
                    .is_some_and(|block| crate::statistics::climbable(state, block.block.raw_id))
        });
    let row = state.entities.row_mut_unmarked(slot);
    if cushioned
        || row.active_effects.has(EffectId::SlowFalling)
        || row.active_effects.has(EffectId::Levitation)
    {
        row.controls.fall_distance = 0.0;
        return 0.0;
    }
    if dy < 0.0 {
        row.controls.fall_distance -= f64::from(dy as f32);
    }
    if !ground {
        return 0.0;
    }
    let distance = std::mem::take(&mut row.controls.fall_distance);
    crate::survival::fall_damage(distance)
}
