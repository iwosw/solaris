//! Golden dandelions toggle the baby age lock (26.1.2 AgeableMob bytecode).
use crate::boundary::{EntityDataValue, EntityId, Hand};
use crate::entities::{ColdRow, EntityKind};
use crate::tick::TickState;
use mc_nbt::Tag;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgeLock {
    pub locked: bool,
    pub cooldown_until: u64,
    /// Tadpole has its own nonnegative Age, rather than AgeableMob's signed age.
    pub tadpole_age: u32,
}

pub(crate) fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
) -> bool {
    let Some(slot) = state.entities.slot_of(target) else {
        return false;
    };
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let EntityKind::Mob { type_id } = body.kind else {
        return false;
    };
    let registry = std::sync::Arc::clone(&state.registry);
    let Some(name) = registry.entity_type_name(type_id) else {
        return false;
    };
    let ageable = registry
        .blueprint(type_id)
        .is_some_and(|b| b.offspring.is_some() || b.capabilities.ageable);
    if !body.alive
        || (!ageable && name != "minecraft:tadpole")
        || (!row.appearance.baby && name != "minecraft:tadpole")
        || matches!(
            name,
            "minecraft:villager" | "minecraft:skeleton_horse" | "minecraft:zombie_horse"
        )
        || state.tick < row.entity_life.age_lock.cooldown_until
    {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let held_slot = crate::tick::held_slot_index(player, hand);
    let held = &player.inventory.slots[held_slot];
    if held.is_empty()
        || state
            .registry
            .items
            .name_of(held.item_id)
            .is_none_or(|id| id.path() != "golden_dandelion")
    {
        return false;
    }
    if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
        return false;
    }
    let row = state.entities.row_mut(slot);
    row.entity_life.age_lock.locked = !row.entity_life.age_lock.locked;
    row.entity_life.age_lock.cooldown_until = state.tick.saturating_add(40);
    if name == "minecraft:tadpole" {
        row.entity_life.age_lock.tadpole_age = 0;
    } else {
        row.entity_life.child_ticks = if name == "minecraft:sniffer" {
            48_000
        } else {
            crate::entity_life::CHILD_TICKS
        };
    }
    if row.entity_life.age_lock.locked {
        row.persistent = true;
    }
    sync(row);
    state
        .hostile
        .events
        .push(crate::boundary::EntityEvent::GoldenDandelion {
            entity: target,
            locked: row.entity_life.age_lock.locked,
        });
    state.appearance_changed.push(target);
    let player = state.players.get_mut(&actor).expect("interacting player");
    if !player.creative() {
        mc_data::inventory_semantics_26_1_2::decrement_stack(
            &mut player.inventory.slots[held_slot],
        );
    }
    let connection = player.connection;
    let contents = player.inventory_contents();
    state.push_player(connection, contents);
    true
}

pub(crate) fn sync(row: &mut ColdRow) {
    // AgeableMob's age-lock accessor follows its baby accessor, index 17.
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.extra.retain(|(index, _)| *index != 17);
    appearance.extra.push((
        17,
        EntityDataValue::Boolean(row.entity_life.age_lock.locked),
    ));
}

pub(crate) fn from_fields(fields: &[(String, Tag)]) -> Result<AgeLock, crate::CoreError> {
    let mut state = AgeLock::default();
    for name in ["AgeLocked", "solaris:AgeLockCooldownUntil"] {
        let mut entries = fields.iter().filter(|(key, _)| key == name);
        let Some((_, value)) = entries.next() else {
            continue;
        };
        if entries.next().is_some() {
            return Err(crate::CoreError::Storage(format!(
                "{name}: duplicate field"
            )));
        }
        match (name, value) {
            ("AgeLocked", Tag::Byte(0 | 1)) => state.locked = value == &Tag::Byte(1),
            ("solaris:AgeLockCooldownUntil", Tag::Long(value)) if *value >= 0 => {
                state.cooldown_until = *value as u64
            }
            ("AgeLocked", _) => {
                return Err(crate::CoreError::Storage(
                    "AgeLocked: expected boolean byte".into(),
                ));
            }
            _ => {
                return Err(crate::CoreError::Storage(
                    "solaris:AgeLockCooldownUntil: expected nonnegative long".into(),
                ));
            }
        }
    }
    Ok(state)
}

#[cfg(test)]
#[path = "golden_dandelion_tests.rs"]
mod tests;
