//! Nearby humanoid equipment collection. Ground debit, equipment credit, swap
//! drops and appearance post-images are published in the same tick/WAL batch.
use crate::boundary::{ChangeSet, EntityEvent};
use crate::entities::{EntityKind, EntitySlot};
use crate::inventory::ItemRules;
use crate::tick::TickState;
use mc_data::ItemStack;
use std::sync::Arc;

fn supported(name: &str) -> bool {
    matches!(
        name,
        "minecraft:zombie"
            | "minecraft:zombie_villager"
            | "minecraft:husk"
            | "minecraft:drowned"
            | "minecraft:zombified_piglin"
            | "minecraft:skeleton"
            | "minecraft:wither_skeleton"
            | "minecraft:stray"
            | "minecraft:bogged"
            | "minecraft:parched"
            | "minecraft:piglin"
    )
}

fn tagged(rules: &ItemRules<'_>, stack: &ItemStack, tag: &str) -> bool {
    rules
        .tags
        .contains_raw_id("minecraft:item", tag, stack.item_id as i32)
}

fn item_name<'a>(rules: &ItemRules<'a>, stack: &ItemStack) -> &'a str {
    rules
        .items
        .name_of(stack.item_id)
        .map_or("", |id| id.path())
}

fn cursed(stack: &ItemStack, curse: &str) -> bool {
    stack
        .enchantments
        .iter()
        .any(|entry| entry.level > 0 && entry.id.as_str() == curse)
}

fn slot(rules: &ItemRules<'_>, stack: &ItemStack) -> Option<usize> {
    let facts = rules.facts.facts_for_stack(stack, rules.items)?;
    if let Some(armor) = &facts.armor {
        return match armor.slot.as_str() {
            "feet" => Some(2),
            "legs" => Some(3),
            "chest" => Some(4),
            "head" => Some(5),
            _ => None,
        };
    }
    (facts.weapon
        || facts.tool.is_some()
        || matches!(item_name(rules, stack), "bow" | "crossbow" | "trident"))
    .then_some(0)
}

fn equal_item_better(new: &ItemStack, old: &ItemStack) -> bool {
    new.damage.unwrap_or(0) < old.damage.unwrap_or(0)
        || new.enchantments.iter().any(|entry| entry.level > 0)
            && !old.enchantments.iter().any(|entry| entry.level > 0)
}

/// Vanilla's category comparisons: armor defense then toughness, swords before
/// tools, damage within a weapon class, durability/enchantment for equal items.
fn better(
    registry: &crate::registry::CoreRegistry,
    mob: &str,
    new: &ItemStack,
    old: Option<&ItemStack>,
) -> bool {
    let rules = registry.item_rules();
    let Some(old) = old.filter(|stack| !stack.is_empty()) else {
        return true;
    };
    let Some(new_facts) = rules.facts.facts_for_stack(new, rules.items) else {
        return false;
    };
    let old_facts = rules.facts.facts_for_stack(old, rules.items);
    if new_facts.armor.is_some() && cursed(old, "minecraft:binding_curse") {
        return false;
    }
    if mob == "minecraft:piglin" {
        let new_gold = tagged(&rules, new, "minecraft:piglin_loved");
        let old_gold = tagged(&rules, old, "minecraft:piglin_loved");
        if new_gold != old_gold {
            return new_gold;
        }
    }
    let new_name = item_name(&rules, new);
    let old_name = item_name(&rules, old);
    if mob == "minecraft:drowned" && (new_name == "trident" || old_name == "trident") {
        return new_name == "trident"
            && (old_name != "trident" || new.damage.unwrap_or(0) < old.damage.unwrap_or(0));
    }
    if let Some(new_armor) = &new_facts.armor {
        let Some(old_armor) = old_facts.and_then(|facts| facts.armor.as_ref()) else {
            return true;
        };
        return match new_armor.armor.total_cmp(&old_armor.armor) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => {
                match new_armor.toughness.total_cmp(&old_armor.toughness) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => equal_item_better(new, old),
                }
            }
        };
    }
    if matches!(new_name, "bow" | "crossbow") {
        return new_name == old_name && equal_item_better(new, old);
    }
    let new_sword = tagged(&rules, new, "minecraft:swords");
    let old_sword = tagged(&rules, old, "minecraft:swords");
    if new_sword && !old_sword {
        return true;
    }
    if !new_sword && old_sword {
        return false;
    }
    // A tool does not displace a bow/crossbow, but may displace a block or
    // another tool. Other melee weapons compare their authored attack damage.
    if matches!(old_name, "bow" | "crossbow") {
        return false;
    }
    if !new_sword && new_facts.tool.is_some() && old_facts.is_none_or(|facts| facts.tool.is_none())
    {
        return rules
            .items
            .name_of(old.item_id)
            .is_some_and(|name| registry.blocks.block(name).is_some());
    }
    let new_damage = new_facts.attack_damage_modifier.unwrap_or(0.0);
    let old_damage = old_facts
        .and_then(|facts| facts.attack_damage_modifier)
        .unwrap_or(0.0);
    match new_damage.total_cmp(&old_damage) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => equal_item_better(new, old),
    }
}

fn swap_drop(chance: f32, mut seed: u64) -> bool {
    // Mob.equipItemIfPossible: nextFloat() - 0.1 < old slot drop chance.
    seed ^= seed >> 30;
    seed = seed.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    seed ^= seed >> 27;
    seed = seed.wrapping_mul(0x94d0_49bb_1331_11eb);
    seed ^= seed >> 31;
    let roll = seed as f64 / u64::MAX as f64;
    roll - 0.1 < f64::from(chance)
}

/// Called after the existing specialized collectors and player merge. Removing
/// immediately by ID makes custody exclusive even for overlapping collectors;
/// partial stacks keep their identity and journal an updated item post-image.
pub(crate) fn run(state: &mut TickState, changes: &mut ChangeSet) {
    if !crate::command_rules::boolean(state, "mob_griefing") {
        return;
    }
    let collectors: Vec<_> = state
        .entities
        .mob_slots_matching(|type_id| {
            state
                .registry
                .blueprint(type_id)
                .is_some_and(|bp| supported(&bp.name))
        })
        .into_iter()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            (body.alive && !row.controls.no_ai && row.loot.can_pick_up_loot).then_some(body.id)
        })
        .collect();
    if collectors.is_empty() {
        return;
    }
    crate::natural_spawn::census(state, &[]);
    let mut candidates = Vec::new();
    // Immutable startup data: one handle outlives the mutable state borrows below.
    let registry = std::sync::Arc::clone(&state.registry);
    for collector in collectors {
        let Some(collector_slot) = state.entities.slot_of(collector) else {
            continue;
        };
        let p = state.entities.pos(collector_slot);
        if !state.world.is_active(
            (p[0].floor() as i32).div_euclid(16),
            (p[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let body = state.entities.body(collector_slot);
        let reach = body.half_width + 1.0;
        let height = body.height;
        let EntityKind::Mob { type_id } = body.kind else {
            continue;
        };
        let name = registry.blueprint(type_id).unwrap().name.as_str();
        candidates.clear();
        candidates.extend(state.spawn_census.nearby_entities(p, reach + 1.0));
        for item in candidates.iter().copied() {
            // A previous full pickup may have swap-removed the collector's
            // row into another slot; IDs, never cached slots, own custody.
            let Some(collector_slot) = state.entities.slot_of(collector) else {
                break;
            };
            let Some(item_slot) = state.entities.slot_of(item) else {
                continue;
            };
            let body = state.entities.body(item_slot);
            let row = state.entities.row(item_slot);
            let q = state.entities.pos(item_slot);
            if !body.alive
                || !matches!(body.kind, EntityKind::Item { .. })
                || body.pickup_delay != 0
                || (p[0] - q[0]).abs() > reach + body.half_width
                || (p[2] - q[2]).abs() > reach + body.half_width
                || q[1] > p[1] + height
                || q[1] + body.height < p[1]
            {
                continue;
            }
            let Some(stack) = row.item.as_ref().filter(|item| item.count > 0) else {
                continue;
            };
            let plain = ItemStack::new(stack.item_id, stack.count);
            let offered = stack.stack.as_ref().unwrap_or(&plain);
            let rules = state.registry.item_rules();
            let Some(index) = slot(&rules, offered) else {
                continue;
            };
            if !better(
                &state.registry,
                name,
                offered,
                state.entities.row(collector_slot).appearance.equipment[index].as_ref(),
            ) {
                continue;
            }
            let mut equipped = offered.clone();
            equipped.count = 1;
            let row = state.entities.row_mut(collector_slot);
            let previous = Arc::make_mut(&mut row.appearance).equipment[index].replace(equipped);
            let chance = row.loot.drop_chances[index];
            row.loot.drop_chances[index] = 2.0;
            row.persistent = true;
            state.appearance_changed.push(collector);
            crate::entity_loot::restore(state, collector_slot);
            debit(state, item_slot, changes);
            if let Some(previous) = previous.filter(|stack| !stack.is_empty())
                && swap_drop(
                    chance,
                    state.tick ^ (collector.0 as u64).rotate_left(19) ^ item.0 as u64,
                )
            {
                changes
                    .entities_spawned
                    .push(crate::tick::spawn_item_entity(state, p, previous));
            }
            changes.entity_events.push(EntityEvent::Pickup {
                item,
                collector,
                count: 1,
            });
        }
    }
}

fn debit(state: &mut TickState, slot: EntitySlot, changes: &mut ChangeSet) {
    let (body, row) = state.entities.parts_mut(slot);
    let id = body.id;
    let item = row.item.as_mut().expect("validated item custody");
    item.count -= 1;
    if item.count == 0 {
        state.entities.remove(slot);
        changes.entities_removed.push(id);
    } else {
        if let Some(stack) = &mut item.stack {
            stack.count = item.count;
        }
        Arc::make_mut(&mut row.appearance).item = Some(item.full_stack());
        state.appearance_changed.push(id);
    }
}

#[cfg(test)]
#[path = "mob_equipment_pickup_tests.rs"]
mod tests;
