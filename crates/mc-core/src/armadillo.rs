//! Armadillo threat curl, scute shedding and brushing (vanilla-like).
use mc_data::ItemStack;
use serde::{Deserialize, Serialize};

use crate::boundary::{EntityId, Hand};
use crate::entities::EntitySlot;
use crate::entities::EntityKind;
use crate::tick::TickState;

const SCARE_DISTANCE: f64 = 7.0;
const SCARE_DISTANCE_V: f64 = 2.0;
const CHECK_INTERVAL: u64 = 10;
const SCUTE_PERIOD: u32 = 6_000;

/// Deterministic stagger for the scute countdown (vanilla
/// pickNextScuteDropTime is 6000 + nextInt(6000) ticks).
fn scute_period(id: crate::boundary::EntityId, drops: u32) -> u32 {
    let mut h = (id.0 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (drops as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    6_000 + (h % 6_000) as u32
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArmadilloState {
    pub rolled: bool,
    /// Ticks until the next scute shed (vanilla scuteTime countdown).
    pub scute_ticks: u32,
    /// Number of shed scutes (staggers the next countdown deterministically).
    pub drops: u32,
    /// Ticks of continuous calm before unrolling (vanilla unroll delay).
    pub calm_ticks: u32,
    /// True once the countdown was seeded (distinguishes fresh zeros).
    pub seeded: bool,
}

impl Default for ArmadilloState {
    fn default() -> Self {
        Self {
            rolled: false,
            scute_ticks: 0,
            drops: 0,
            calm_ticks: 0,
            seeded: false,
        }
    }
}

pub(crate) fn is_armadillo(state: &TickState, type_id: u32) -> bool {
    state.registry.blueprint(type_id).is_some_and(|b| b.name == "minecraft:armadillo")
}

fn threat_near(state: &TickState, self_slot: EntitySlot, at: [f64; 3]) -> bool {
    // Armadillo.isScaredBy (26.1.2): 7x2x7 box around self; undead entity
    // types, the last player/mob that hurt the armadillo, or a sprinting
    // (non-spectator) player threaten.
    let hurt_by = state
        .entities
        .row(self_slot)
        .last_player_hit
        .map(|(attacker, _)| attacker);
    for index in 0..state.entities.len() as u32 {
        let slot = EntitySlot(index);
        if slot == self_slot {
            continue;
        }
        let body = state.entities.body(slot);
        if !body.alive {
            continue;
        }
        let pos = state.entities.pos(slot);
        if (pos[0] - at[0]).abs() > SCARE_DISTANCE
            || (pos[1] - at[1]).abs() > SCARE_DISTANCE_V
            || (pos[2] - at[2]).abs() > SCARE_DISTANCE
        {
            continue;
        }
        match body.kind {
            EntityKind::Mob { type_id } => {
                // Engine's undead marker is the inverted_healing capability
                // (vanilla EntityTypeTags.UNDEAD members are exactly the
                // inverted-healing mobs); the embedded test registry carries
                // no undead tag, so the capability is the portable check.
                if state
                    .registry
                    .blueprint(type_id)
                    .is_some_and(|b| b.capabilities.inverted_healing)
                {
                    return true;
                }
                if hurt_by.is_some_and(|attacker| attacker == body.id) {
                    return true;
                }
            }
            EntityKind::Player { .. } => {
                let id = body.id;
                if hurt_by.is_some_and(|attacker| attacker == id) {
                    return true;
                }
                let passenger = state.entities.row(slot).entity_life.vehicle.is_some();
                if state.players.get(&id).is_some_and(|p| {
                    !p.spectator() && (p.sprinting || passenger) && !p.vitals.is_dead()
                }) {
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
        let threatened = threat_near(state, slot, at);
        let id = state.entities.body(slot).id;
        let row = state.entities.row_mut(slot);
        let st = row.entity_life.armadillo.get_or_insert_with(ArmadilloState::default);
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
        // Adults shed a scute on their timer (vanilla scuteTime
        // countdown from pickNextScuteDropTime: 6000 + nextInt(6000)).
        if row.entity_life.child_ticks == 0 {
            if !st.seeded {
                st.scute_ticks = scute_period(id, st.drops);
                st.seeded = true;
            }
            st.scute_ticks = st.scute_ticks.saturating_sub(CHECK_INTERVAL as u32);
            if st.scute_ticks == 0 {
                st.drops += 1;
                st.scute_ticks = scute_period(id, st.drops);
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
    let held_slot = {
        let Some(player) = state.players.get(&actor) else {
            return false;
        };
        let held_slot = crate::tick::held_slot_index(player, hand);
        let held = &player.inventory.slots[held_slot];
        let valid = !player.spectator()
            && !held.is_empty()
            && state
                .registry
                .items
                .name_of(held.item_id)
                .is_some_and(|name| name.as_str() == "minecraft:brush");
        if !valid {
            return false;
        }
        held_slot
    };
    if crate::tick::held_debit_below_floor(state, actor, hand, 0) {
        return false;
    }
    if state.entities.row(slot).entity_life.child_ticks != 0 {
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
    // Brushes wear 16 per scute (mobInteract hurtAndBreak, 26.1.2),
    // breaking at max durability.
    let creative = state.players.get(&actor).is_some_and(|p| p.creative());
    if !creative {
        let max = state
            .registry
            .item_rules()
            .facts
            .facts_for_stack(
                &state.players.get(&actor).expect("player").inventory.slots[held_slot],
                &state.registry.items,
            )
            .and_then(|facts| facts.max_damage);
        let player = state.players.get_mut(&actor).expect("interacting player");
        let stack = &mut player.inventory.slots[held_slot];
        let worn = stack.damage.unwrap_or(0).saturating_add(16);
        if max.is_some_and(|max| worn as u32 >= max) {
            *stack = ItemStack::EMPTY;
        } else {
            stack.damage = Some(worn);
        }
    }
    let player = state.players.get(&actor).expect("interacting player");
    let connection = player.connection;
    let contents = player.inventory_contents();
    state.push_player(connection, contents);
    true
}
