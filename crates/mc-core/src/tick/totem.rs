//! Lethal-hit interception, before death/drop/plugin notifications.
use crate::boundary::{Hand, PlayerChange};
use crate::effects::{ActiveEffect, EffectId};
use crate::player::PlayerState;
use crate::registry::CoreRegistry;
use crate::survival::DamageKind;
use mc_data::ItemStack;

pub(super) fn rescue(
    player: &mut PlayerState,
    registry: &CoreRegistry,
    kind: DamageKind,
) -> Option<(ItemStack, Vec<PlayerChange>)> {
    // LivingEntity.checkTotemDeathProtection rejects bypasses_invulnerability.
    if kind.bypasses_invulnerability() {
        return None;
    }
    for (hand, slot) in [
        (Hand::Main, usize::from(player.inventory.selected)),
        (Hand::Off, crate::inventory::OFF_HAND),
    ] {
        let stack = &player.inventory.slots[slot];
        if stack.is_empty()
            || stack
                .extended()
                .preserved_components
                .iter()
                .any(|component| component.name == "!minecraft:death_protection")
        {
            continue;
        }
        let custom = stack
            .extended()
            .preserved_components
            .iter()
            .find(|component| component.name == "minecraft:death_protection");
        if let Some(component) = custom {
            let mut bytes = component.value.as_slice();
            let Ok(value) = mc_nbt::read_network_value(&mut bytes) else {
                continue;
            };
            if !bytes.is_empty() || mc_data::item_stack::validate_death_protection(&value).is_err()
            {
                continue;
            }
        } else if registry
            .items
            .name_of(stack.item_id)
            .is_none_or(|name| name.as_str() != "minecraft:totem_of_undying")
        {
            continue;
        }
        let default_effects = custom.is_none();
        let used = stack.clone();
        let mut remaining = used.clone();
        remaining.count -= 1;
        if remaining.count == 0 {
            remaining = ItemStack::EMPTY;
        }
        let mut changes = player.replace_held(hand, remaining);
        player
            .statistics
            .increment(mc_data::statistics::StatType::Used, used.item_id, 1);
        player.vitals.health = 1.0;
        if default_effects {
            changes.extend(player.clear_effects(None));
            // Pinned 26.1.2 DeathProtection.TOTEM_OF_UNDYING default component.
            for (id, duration, amplifier) in [
                (EffectId::Regeneration, 900, 1),
                (EffectId::Absorption, 100, 1),
                (EffectId::FireResistance, 800, 0),
            ] {
                changes.extend(player.give_effect(ActiveEffect::new(id, duration, amplifier)));
            }
        }
        return Some((used, changes));
    }
    None
}
