use crate::enchantments::{durability_damage, level, random};
use crate::intent::DamageSource;
use crate::survival::DamageKind;
use crate::tick::{DamagePiece, TickState};

fn trigger(stack: &mc_data::ItemStack, seed: &mut i32) -> f32 {
    let level = level(stack, "thorns");
    if level == 0 || random(seed) >= (0.15 * f64::from(level)).min(1.0) {
        return 0.0;
    }
    1.0 + (random(seed) * 4.0) as f32
}

fn wear(
    rules: &crate::inventory::ItemRules<'_>,
    stack: &mut mc_data::ItemStack,
    seed: &mut i32,
) -> bool {
    let Some(maximum) = rules
        .facts
        .facts_for_stack(stack, rules.items)
        .and_then(|facts| facts.max_damage)
    else {
        return false;
    };
    let amount = durability_damage(stack, 2, true, seed);
    if amount == 0 {
        return false;
    }
    let damage = stack.damage.unwrap_or(0).saturating_add(amount);
    if u32::try_from(damage).is_ok_and(|damage| damage >= maximum) {
        *stack = mc_data::ItemStack::EMPTY;
    } else {
        stack.damage = Some(damage);
    }
    true
}

/// Stage only admitted hits, without recursive thorns or cross-tick damage custody.
pub(crate) fn accepted_hit(
    state: &mut TickState,
    slot: crate::entities::EntitySlot,
    piece: &DamagePiece,
) {
    if piece.kind == DamageKind::Thorns {
        return;
    }
    let attacker = match piece.source {
        Some(
            DamageSource::Melee { attacker, .. }
            | DamageSource::Mace { attacker, .. }
            | DamageSource::Sweep { attacker, .. },
        ) => attacker,
        Some(DamageSource::Projectile { owner, .. }) => owner,
        Some(DamageSource::Command {
            cause: Some((attacker, _)),
            ..
        }) => attacker,
        Some(DamageSource::Command { cause: None, .. }) => return,
        None | Some(DamageSource::Explosion { .. }) => return,
    };
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let defender = body.id;
    if attacker == defender {
        return;
    }
    let Some(attacker_slot) = state.entities.slot_of(attacker) else {
        return;
    };
    let position = state.entities.pos(slot);
    let mut seed = (row.uuid.as_u128() as u32 ^ row.age) as i32;
    let mut reflected = [0.0; 4];
    let mut changed = false;
    let rules = state.registry.item_rules();
    if let Some(player) = state.players.get_mut(&defender) {
        if player.creative() || player.spectator() || player.vitals.is_dead() {
            return;
        }
        for (stack, damage) in player.inventory.slots[36..40]
            .iter_mut()
            .zip(&mut reflected)
        {
            *damage = trigger(stack, &mut player.enchantment_seed);
            if *damage > 0.0 {
                changed |= wear(&rules, stack, &mut player.enchantment_seed);
            }
        }
    } else {
        let appearance = std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
        for (stack, damage) in appearance.equipment[2..].iter_mut().zip(&mut reflected) {
            let Some(stack) = stack.as_mut() else {
                continue;
            };
            *damage = trigger(stack, &mut seed);
            if *damage > 0.0 {
                wear(&rules, stack, &mut seed);
            }
        }
        for stack in &mut appearance.equipment[2..] {
            if stack.as_ref().is_some_and(mc_data::ItemStack::is_empty) {
                *stack = None;
            }
        }
    }
    if changed {
        crate::projectiles::weapons::sync(state, defender);
    }
    for amount in reflected.into_iter().filter(|amount| *amount > 0.0) {
        state.merge_damage.push((
            attacker_slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount,
                kind: DamageKind::Thorns,
                source: Some(DamageSource::Melee {
                    attacker: defender,
                    position,
                    yaw: 0.0,
                    bonus_knockback: 0.0,
                    sweep: false,
                    fire_ticks: 0,
                    bane_ticks: 0,
                }),
            },
        ));
    }
}
