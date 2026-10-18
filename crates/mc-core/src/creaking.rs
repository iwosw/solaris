//! Unbound creakings use normal living damage/persistence. Their gaze gate is
//! derived from current player views, so no restart-local or retained timer is
//! needed. Heart spawning/linking has no native block owner yet.
use crate::entities::{EntityKind, EntitySlot};
use crate::tick::TickState;
use mc_physics::mth;
use std::sync::LazyLock;

static CREAKING_TYPE: LazyLock<Option<u32>> =
    LazyLock::new(|| crate::piglin::type_id_of("minecraft:creaking"));

/// Cheap row gate shared by [`observed`] and the tick's row preparation: only
/// creakings carry observer state.
pub(crate) fn needs_state(body: &crate::entities::Body) -> bool {
    matches!(body.kind, EntityKind::Mob { type_id } if Some(type_id) == *CREAKING_TYPE)
}

/// Survival/adventure observers within follow range freeze the creaking when
/// their view lies within 60 degrees and an unobstructed ray reaches its body.
/// Reference: https://minecraft.wiki/w/Creaking#Field_of_view_detection.
pub(crate) fn observed(state: &TickState, slot: EntitySlot) -> bool {
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    if !needs_state(body) {
        return false;
    }
    let pos = state.entities.pos(slot);
    state.players.iter().any(|(id, player)| {
        if player.creative() || player.spectator() {
            return false;
        }
        let Some(observer) = state.entities.slot_of(*id) else {
            return false;
        };
        if !state.entities.body(observer).alive || state.entities.row(observer).health <= 0.0 {
            return false;
        }
        if player.inventory.slots.get(39).is_some_and(|stack| {
            state
                .registry
                .items
                .name_of(stack.item_id)
                .is_some_and(|name| {
                    name.as_str() == "minecraft:carved_pumpkin" && !stack.is_empty()
                })
        }) {
            return false;
        }
        let mut eye = state.entities.pos(observer);
        eye[1] += 1.62;
        let rot = state.entities.rot(observer);
        let yaw = f64::from(rot[0]).to_radians();
        let pitch = f64::from(rot[1]).to_radians();
        let view = [
            -f64::from(mth::sin(yaw)) * f64::from(mth::cos(pitch)),
            -f64::from(mth::sin(pitch)),
            f64::from(mth::cos(yaw)) * f64::from(mth::cos(pitch)),
        ];
        // Seeing any of the lower body, torso or eyes is sufficient. A low
        // wall must not hide the entire tall body.
        [0.0, body.height * 0.5, body.height * 0.85]
            .into_iter()
            .any(|height| {
                let at = [pos[0], pos[1] + height, pos[2]];
                let delta = std::array::from_fn::<_, 3, _>(|axis| at[axis] - eye[axis]);
                let distance_sq = delta.iter().map(|v| v * v).sum::<f64>();
                if distance_sq > f64::from(row.sight_range).powi(2) {
                    return false;
                }
                let dot = (0..3).map(|axis| view[axis] * delta[axis]).sum::<f64>();
                dot >= distance_sq.sqrt() * 0.5 && crate::hostile::sight_clear(state, eye, at)
            })
    })
}

#[cfg(test)]
#[path = "creaking_tests.rs"]
mod tests;
