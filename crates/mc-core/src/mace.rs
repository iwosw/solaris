//! Merge-owned mace smash impulses; weapon facts and damage stay in combat.
use crate::boundary::PlayerChange;
use crate::entities::{EntityKind, EntitySlot};
use crate::intent::DamageSource;
use crate::tick::TickState;
use mc_physics::mth;

pub(crate) fn breach(source: Option<DamageSource>) -> u32 {
    match source {
        Some(DamageSource::Mace { attack, .. }) => attack.breach,
        _ => 0,
    }
}

fn motion(state: &mut TickState, slot: EntitySlot, velocity: [f64; 3]) {
    state.entities.set_vel(slot, velocity);
    state.entities.set_ground(slot, false);
    let id = state.entities.body(slot).id;
    if !matches!(state.entities.body(slot).kind, EntityKind::Player { .. }) {
        state
            .entities
            .set_horizontal_impulse(slot, [velocity[0], velocity[2]]);
    }
    if let Some(player) = state.players.get(&id) {
        let connection = player.connection;
        state.push_player(
            connection,
            PlayerChange::Motion {
                entity: id,
                velocity,
            },
        );
    }
    state.appearance_changed.push(id);
}

pub(crate) fn accepted_hit(
    state: &mut TickState,
    target: EntitySlot,
    source: Option<DamageSource>,
) {
    let Some(DamageSource::Mace {
        attacker, attack, ..
    }) = source
    else {
        return;
    };
    if attack.fall_distance <= 1.5 {
        return;
    }
    let Some(actor) = state.entities.slot_of(attacker) else {
        return;
    };
    let origin = state.entities.pos(target);
    let primary = state.entities.body(target).id;
    let impact = state.entities.pos(actor);
    if let Some(player) = state.players.get_mut(&attacker) {
        player.fall_distance = 0.0;
        player.current_impulse_impact_pos = Some(impact);
    }
    let mut velocity = state.entities.vel(actor);
    velocity[1] = 0.2; // MaceItem.hurtEnemy: 0.01 blocks/tick.
    motion(state, actor, velocity);
    for index in 0..state.entities.len() as u32 {
        let slot = EntitySlot(index);
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if !body.alive
            || body.id == attacker
            || body.id == primary
            || !matches!(
                body.kind,
                EntityKind::Mob { .. } | EntityKind::Player { .. }
            )
            || row.projectile.is_some()
            || row.falling.is_some()
            || row.tnt.is_some()
            || crate::taming::excluded_sweep(row)
            || state.players.get(&body.id).is_some_and(|p| p.spectator())
        {
            continue;
        }
        let delta: [f64; 3] =
            std::array::from_fn(|axis| state.entities.pos(slot)[axis] - origin[axis]);
        let distance = mth::length3(delta[0], delta[1], delta[2]);
        if distance >= 3.5 {
            continue;
        }
        let power = (3.5 - distance)
            * 0.7
            * if attack.fall_distance > 5.0 { 2.0 } else { 1.0 }
            * (1.0
                - crate::entity_loot::effective_attribute(
                    &state.registry.item_rules(),
                    row,
                    "knockback_resistance",
                    0.0,
                )
                .clamp(0.0, 1.0));
        if power <= 0.0 {
            continue;
        }
        let horizontal = mth::length2(delta[0], delta[2]);
        let direction = if horizontal > 1e-8 {
            [delta[0] / horizontal, delta[2] / horizontal]
        } else {
            [1.0, 0.0]
        };
        let mut velocity = state.entities.vel(slot);
        velocity[0] += direction[0] * power * 20.0;
        velocity[1] += 14.0;
        velocity[2] += direction[1] * power * 20.0;
        motion(state, slot, velocity);
    }
    if attack.wind_burst > 0 {
        crate::projectiles::payloads::wind_burst_scaled(
            state,
            impact,
            3.5,
            crate::enchantments::wind_burst_multiplier(attack.wind_burst),
        );
    }
}

pub(crate) fn deserialize_impact<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<[f64; 3]>, D::Error> {
    use serde::Deserialize;
    let value = Option::<[f64; 3]>::deserialize(deserializer)?;
    if value.is_some_and(|pos| pos.into_iter().any(|v| !v.is_finite())) {
        return Err(serde::de::Error::custom(
            "current_explosion_impact_pos must contain finite coordinates",
        ));
    }
    Ok(value)
}

pub(crate) fn read_impact(
    fields: &[(String, mc_nbt::Tag)],
) -> Result<Option<[f64; 3]>, &'static str> {
    let mut matches = fields
        .iter()
        .filter(|(name, _)| name == "current_explosion_impact_pos");
    let Some((_, tag)) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err("duplicate field");
    }
    let mc_nbt::Tag::List(mc_nbt::ListTag {
        element_type: 6,
        elements,
    }) = tag
    else {
        return Err("expected three-double list");
    };
    if elements.len() != 3 {
        return Err("expected three-double list");
    }
    let mut pos = [0.0; 3];
    for (value, tag) in pos.iter_mut().zip(elements) {
        let mc_nbt::Tag::Double(v) = tag else {
            return Err("expected three-double list");
        };
        if !v.is_finite() {
            return Err("coordinate must be finite");
        }
        *value = *v;
    }
    Ok(Some(pos))
}
