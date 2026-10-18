//! Lazy ContainerEntity loot is retained until an inventory custody operation.
use super::VehicleState;
use crate::config::CoreError;
use crate::entities::EntitySlot;
use crate::tick::TickState;
use mc_data::loot::chest_26_1_2::{ChestLootCatalog, ChestRng};

pub(super) fn validate(v: &VehicleState) -> Result<(), String> {
    if v.loot_table.is_none() && v.loot_seed != 0 {
        return Err("vehicle LootTableSeed: seed without LootTable".into());
    }
    if v.loot_table.is_some() {
        if v.slots.is_empty() {
            return Err("vehicle LootTable: entity has no container".into());
        }
        if v.slots.iter().any(|stack| !stack.is_empty()) {
            return Err("vehicle LootTable: unresolved loot has populated Items".into());
        }
    }
    Ok(())
}

pub(super) fn materialize(state: &mut TickState, slot: EntitySlot) {
    let v = state
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap();
    let Some(id) = &v.loot_table else { return };
    let table = state
        .registry
        .chest_loot
        .get(id)
        .expect("validated saved vehicle loot table");
    let mut rng = ChestRng::new(v.loot_seed as u64);
    let stacks = table
        .roll(&mut rng, &state.registry.items, 0.0)
        .expect("validated canonical vehicle loot items");
    let v = state
        .entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap();
    let mut remaining = v.slots.len();
    let mut available = [0_usize; 27];
    for (index, available) in available.iter_mut().take(remaining).enumerate() {
        *available = index;
    }
    for stack in stacks {
        if remaining == 0 {
            break;
        }
        let choice = rng.next_bounded(remaining as u64) as usize;
        let index = available[choice];
        remaining -= 1;
        available[choice] = available[remaining];
        v.slots[index] = stack;
    }
    v.loot_table = None;
    v.loot_seed = 0;
}

/// Only an accepted player open supplies provenance; hoppers and destruction do not.
pub(super) fn materialize_for_player(
    state: &mut TickState,
    slot: EntitySlot,
    player: crate::boundary::EntityId,
) {
    let loot_table = state
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .as_ref()
        .and_then(|vehicle| vehicle.loot_table.as_ref())
        .filter(|_| {
            !state.registry.advancements.definitions.is_empty()
                && (state.registry.advancements.trigger_index.is_empty()
                    || state
                        .registry
                        .advancements
                        .trigger_index
                        .contains_key("minecraft:player_generates_container_loot"))
        })
        .map(|id| id.as_str().to_owned());
    materialize(state, slot);
    if let Some(loot_table) = loot_table {
        crate::advancements::trigger(
            state,
            player,
            "minecraft:player_generates_container_loot",
            serde_json::json!({"loot_table": loot_table}),
        );
    }
}

pub(super) fn validate_items(
    v: &VehicleState,
    chest_loot: &ChestLootCatalog,
    items: &mc_data::items::ItemRegistry,
) -> Result<(), CoreError> {
    if let Some(id) = &v.loot_table {
        let table = chest_loot.get(id).ok_or_else(|| {
            CoreError::Storage(format!(
                "vehicle LootTable {id}: unsupported saved container table"
            ))
        })?;
        for pool in table.pools() {
            for entry in &pool.entries {
                if let mc_data::loot::chest_26_1_2::ChestLootEntry::Item { item, .. } = entry
                    && items.id_of(item).is_none()
                {
                    return Err(CoreError::Storage(format!(
                        "vehicle LootTable {id}: unknown item {item}"
                    )));
                }
            }
        }
    }
    Ok(())
}
