//! LivingEntity's transient embedded arrows and bee stingers (26.1.2 javap).
//! Neither counts nor removal timers occur in LivingEntity save/load methods.
use std::sync::Arc;

use crate::boundary::EntityId;
use crate::entities::{ColdRow, EntityKind, EntitySlot};
use crate::intent::DamageSource;
use crate::projectiles::{ProjectileKind, ProjectileState};
use crate::tick::TickState;

fn living(state: &TickState, slot: EntitySlot) -> bool {
    match state.entities.body(slot).kind {
        EntityKind::Player { .. } => true,
        EntityKind::Mob { type_id } => {
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id).is_some_and(
                |contract| {
                    contract.behavior.archetype == mc_data::entity_types::EntityArchetype::Living
                },
            )
        }
        _ => false,
    }
}

pub(crate) fn queue_arrow_hit(
    state: &mut TickState,
    projectile: EntityId,
    target: EntityId,
    record: &ProjectileState,
) {
    if !matches!(
        record.kind,
        ProjectileKind::Arrow | ProjectileKind::SpectralArrow
    ) || record.piercing != 0
    {
        return;
    }
    let Some(slot) = state.entities.slot_of(target) else {
        return;
    };
    if !living(state, slot) {
        return;
    }
    if matches!(state.entities.body(slot).kind, EntityKind::Mob { type_id }
        if state.registry.blueprint(type_id).is_some_and(|blueprint| blueprint.name == "minecraft:enderman"))
    {
        return;
    }
    state.projectiles.embedded_hits.insert((projectile, target));
}

/// Called only after shared damage admission succeeds, including shield/hurt windows.
pub(crate) fn accepted_hit(
    state: &mut TickState,
    victim: EntitySlot,
    source: Option<DamageSource>,
) {
    let Some(DamageSource::Projectile { projectile, .. }) = source else {
        return;
    };
    let target = state.entities.body(victim).id;
    if state
        .projectiles
        .embedded_hits
        .remove(&(projectile, target))
    {
        let appearance = Arc::make_mut(&mut state.entities.row_mut(victim).appearance);
        appearance.arrow_count = appearance.arrow_count.wrapping_add(1);
        state.appearance_changed.push(target);
    }
}

/// The bee's accepted-hit callback owns its one successful sting transition.
pub(crate) fn sting(state: &mut TickState, victim: EntitySlot) {
    if living(state, victim) {
        let (body, row) = state.entities.parts_mut(victim);
        let appearance = Arc::make_mut(&mut row.appearance);
        appearance.stinger_count = appearance.stinger_count.wrapping_add(1);
        state.appearance_changed.push(body.id);
    }
}

fn decay(count: i32, timer: &mut i32) -> bool {
    if count <= 0 {
        return false;
    }
    if *timer <= 0 {
        *timer = 30_i32.wrapping_sub(count).wrapping_mul(20);
    }
    *timer = timer.wrapping_sub(1);
    *timer <= 0
}

/// Reuse the existing row tick; unchanged timers never clone the appearance.
pub(crate) fn advance(row: &mut ColdRow) -> bool {
    let arrows = decay(
        row.appearance.arrow_count,
        &mut row.embedded_projectile_timers[0],
    );
    let stingers = decay(
        row.appearance.stinger_count,
        &mut row.embedded_projectile_timers[1],
    );
    if arrows || stingers {
        let appearance = Arc::make_mut(&mut row.appearance);
        if arrows {
            appearance.arrow_count -= 1;
        }
        if stingers {
            appearance.stinger_count -= 1;
        }
        return true;
    }
    false
}

#[cfg(test)]
#[path = "embedded_projectiles_tests.rs"]
mod tests;
