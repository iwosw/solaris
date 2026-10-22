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
use std::sync::LazyLock;

/// `#minecraft:fall_damage_immune` of the 26.1.2 data pack.
const IMMUNE: [&str; 18] = [
    "minecraft:copper_golem",
    "minecraft:iron_golem",
    "minecraft:snow_golem",
    "minecraft:shulker",
    "minecraft:allay",
    "minecraft:bat",
    "minecraft:bee",
    "minecraft:blaze",
    "minecraft:cat",
    "minecraft:chicken",
    "minecraft:ghast",
    "minecraft:happy_ghast",
    "minecraft:phantom",
    "minecraft:magma_cube",
    "minecraft:ocelot",
    "minecraft:parrot",
    "minecraft:wither",
    "minecraft:breeze",
];

static IMMUNE_BY_TYPE: LazyLock<[bool; mc_data::entity_types::ENTITY_TYPE_COUNT]> =
    LazyLock::new(|| {
        let mut immune = [false; mc_data::entity_types::ENTITY_TYPE_COUNT];
        for (index, slot) in immune.iter_mut().enumerate() {
            *slot = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(index as u32)
                .is_some_and(|contract| IMMUNE.contains(&contract.name));
        }
        immune
    });

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
) -> f32 {
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
    ) || IMMUNE_BY_TYPE
        .get(type_id as usize)
        .copied()
        .unwrap_or(true)
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
