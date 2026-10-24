//! Armadillo threat curl, scute shedding and brushing (vanilla-like).
use mc_data::ItemStack;
use serde::{Deserialize, Serialize};

use crate::boundary::{EntityId, Hand};
use crate::entities::EntitySlot;
use crate::entities::EntityKind;
use crate::tick::TickState;

const SCARE_DISTANCE: f64 = 7.0;
const CHECK_INTERVAL: u64 = 10;
const SCUTE_PERIOD: u32 = 6_000;
const BRUSH_COOLDOWN: u32 = 100;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArmadilloState {
    pub rolled: bool,
    pub scute_ticks: u32,
    pub brush_cooldown: u32,
    /// Ticks of continuous calm before unrolling (vanilla unroll delay).
    pub calm_ticks: u32,
}

pub(crate) fn is_armadillo(state: &TickState, type_id: u32) -> bool {
    state.registry.blueprint(type_id).is_some_and(|b| b.name == "minecraft:armadillo")
}

fn threat_near(state: &TickState, at: [f64; 3]) -> bool {
    // Undead mobs (inverted healing) threaten; sprinting players threaten.
    for index in 0..state.entities.len() as u32 {
        let slot = EntitySlot(index);
        let body = state.entities.body(slot);
        if !body.alive {
            continue;
        }
        let pos = state.entities.pos(slot);
        let dx = pos[0] - at[0];
        let dy = pos[1] - at[1];
        let dz = pos[2] - at[2];
        if dx * dx + dy * dy + dz * dz > SCARE_DISTANCE * SCARE_DISTANCE {
            continue;
        }
        match body.kind {
            EntityKind::Mob { type_id } => {
                if state.registry.blueprint(type_id).is_some_and(|b| b.capabilities.inverted_healing) {
                    return true;
                }
            }
            EntityKind::Player { .. } => {
                let id = body.id;
                if state.players.get(&id).is_some_and(|p| p.sprinting && !p.vitals.is_dead()) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

pub(crate) fn run(state: &mut TickState) {
    if !state.tick.is_multiple_of(CHECK_INTERVAL) {
        // Scute shedding still ticks every check interval (compressed below).
        run_interval(state);
        return;
    }
    run_interval(state);
}

fn run_interval(state: &mut TickState) {
    let mut drops: Vec<([f64; 3], ItemStack)> = Vec::new();
    for index in 0..state.entities.len() as u32 {
        let slot = EntitySlot(index);
        let body = state.entities.body(slot);
        let EntityKind::Mob { type_id } = body.kind else {
            continue;
        };
        if !body.alive || !is_armadillo(state, type_id) {
            continue;
        }
        let at = state.entities.pos(slot);
        let threatened = threat_near(state, at);
        let row = state.entities.row_mut(slot);
        let st = row.entity_life.armadillo.get_or_insert_with(ArmadilloState::default);
        if st.brush_cooldown > 0 {
            st.brush_cooldown = st.brush_cooldown.saturating_sub(CHECK_INTERVAL as u32);
        }
        if threatened {
            st.rolled = true;
            st.calm_ticks = 0;
        } else if st.rolled {
            st.calm_ticks += CHECK_INTERVAL as u32;
            // Unroll after ~4 seconds of calm (vanilla UNROLLING delay).
            if st.calm_ticks >= 80 {
                st.rolled = false;
                st.calm_ticks = 0;
            }
        }
        // Adults shed a scute on their timer (vanilla scuteTime drop).
        if row.entity_life.child_ticks == 0 {
            st.scute_ticks = st.scute_ticks.saturating_add(CHECK_INTERVAL as u32);
            if st.scute_ticks >= SCUTE_PERIOD {
                st.scute_ticks = 0;
                let scute = state
                    .registry
                    .items
                    .id_of(&mc_data::Identifier::parse("minecraft:armadillo_scute").unwrap())
                    .map(|id| ItemStack::new(id, 1));
                if let Some(scute) = scute {
                    drops.push((at, scute));
                }
            }
        }
    }
    for (at, scute) in drops {
        crate::tick::spawn_item_entity(state, at, scute);
    }
}

/// Brushing an adult armadillo sheds a scute (vanilla brush hookup).
pub(crate) fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
) -> bool {
    if hand != Hand::Main {
        return false;
    }
    let Some(slot) = state.entities.slot_of(target) else {
        return false;
    };
    let body = state.entities.body(slot);
    let EntityKind::Mob { type_id } = body.kind else {
        return false;
    };
    if !body.alive || !is_armadillo(state, type_id) {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let held_slot = crate::tick::held_slot_index(player, hand);
    let held = &player.inventory.slots[held_slot];
    if player.spectator()
        || held.is_empty()
        || state
            .registry
            .items
            .name_of(held.item_id)
            .is_none_or(|name| name.as_str() != "minecraft:brush")
        || crate::tick::held_debit_below_floor(state, actor, hand, 0)
    {
        return false;
    }
    let row = state.entities.row(slot);
    if row.entity_life.child_ticks != 0 {
        return false;
    }
    if row.entity_life.armadillo.as_ref().is_some_and(|st| st.brush_cooldown > 0) {
        return false;
    }
    let scute_id = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:armadillo_scute").unwrap());
    let Some(scute_id) = scute_id else {
        return false;
    };
    let at = state.entities.pos(slot);
    crate::tick::spawn_item_entity(state, at, ItemStack::new(scute_id, 1));
    state
        .entities
        .row_mut(slot)
        .entity_life
        .armadillo
        .get_or_insert_with(ArmadilloState::default)
        .brush_cooldown = BRUSH_COOLDOWN;
    // Brushes wear (vanilla brush durability): debit one use via the armor
    // damage path is brush-specific; reuse the generic held-damage helper.
    let player = state.players.get_mut(&actor).expect("interacting player");
    if !player.creative() {
        let stack = &mut player.inventory.slots[held_slot];
        let worn = stack.damage.unwrap_or(0).saturating_add(1);
        stack.damage = Some(worn);
    }
    let connection = player.connection;
    let contents = player.inventory_contents();
    state.push_player(connection, contents);
    true
}
