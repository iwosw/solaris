//! What a dead mob drops (ADR 0013): the registry-owned vanilla 26.1.2 entity loot
//! table named by the mob's blueprint. Pure; the merge commit spawns the
//! returned stacks as item entities.

use mc_data::loot::LootRandomBinding;
use mc_data::loot::entity_26_1_2::{
    EntityDeathCause, EntityLootContext, EntityLootEntity, VanillaEntityLootLookups,
};
use mc_data::{Identifier, ItemStack};

use crate::registry::CoreRegistry;

/// Stacks from the blueprint table with the accepted hit's retained context.
/// `wool` (color id, `0x10` sheared) picks a sheep's wool table.
#[must_use]
pub fn mob_drops(
    registry: &CoreRegistry,
    mut entity: EntityLootEntity,
    table: &Identifier,
    origin: [f64; 3],
    seed: u64,
    wool: Option<u8>,
    cause: EntityDeathCause,
) -> Vec<ItemStack> {
    let items = registry.item_rules();
    let catalog = &*registry.entity_loot;
    if !catalog.has_root(table) {
        return Vec::new();
    }
    if let Some(wool) = wool {
        entity.components.insert(
            Identifier::parse("minecraft:sheep/color").expect("component id parses"),
            crate::blueprint::DYE_COLORS[usize::from(wool & 0x0F)].to_owned(),
        );
        entity.sheep_sheared = Some(wool & 0x10 != 0);
    }
    let mut context = EntityLootContext::new(
        entity,
        cause,
        LootRandomBinding::new(catalog.random_sequence(table).cloned(), seed),
    );
    context.origin = origin;
    let lookups: &VanillaEntityLootLookups = &registry.entity_loot_lookups;
    let Ok(rolled) = catalog.evaluate(table, &context.with_lookups(lookups, lookups)) else {
        return Vec::new();
    };
    let mut stacks = Vec::new();
    for drop in rolled {
        let Some(item_id) = items.items.id_of(&drop.item) else {
            continue;
        };
        let mut template = ItemStack::new(item_id, 1);
        if let Some(potion) = drop.components.potion {
            template.extended_mut().potion = Some(potion);
        }
        if let Some(amplifier) = drop.components.ominous_bottle_amplifier {
            template.extended_mut().ominous_bottle_amplifier = Some(i32::from(amplifier));
        }
        let max = items.max_stack(&template).max(1);
        let mut remaining = i32::try_from(drop.count).unwrap_or(i32::MAX);
        while remaining > 0 {
            let mut stack = template.clone();
            stack.count = remaining.min(max);
            remaining -= stack.count;
            stacks.push(stack);
        }
    }
    stacks
}

#[cfg(test)]
#[path = "mob_loot_tests.rs"]
mod tests;
