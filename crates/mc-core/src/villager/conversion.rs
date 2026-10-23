//! Zombie infection and weakness/golden-apple cure on the single tick owner.
use super::{Gossip, VillagerState};
use crate::{
    boundary::{EntityId, Hand},
    effects::{ActiveEffect, EffectId},
    entities::{EntityKind, EntitySlot},
    tick::TickState,
};

fn type_id(state: &TickState, name: &str) -> Option<u32> {
    state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).ok()?)
}

pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot, type_id: u32) {
    let name = state.registry.blueprint(type_id).map(|b| b.name.as_str());
    if matches!(
        name,
        Some("minecraft:villager" | "minecraft:zombie_villager")
    ) {
        debug_assert!(
            matches!(
                state.entities.body(slot).kind,
                EntityKind::Mob { type_id: kind }
                if kind == type_id
            ),
            "villager component implies the villager/zombie mob lanes"
        );
        let mut villager = VillagerState::default();
        if name == Some("minecraft:zombie_villager") {
            villager.conversion_time = Some(-1);
        }
        state.entities.row_mut(slot).villager = Some(Box::new(villager));
        super::sync_appearance(state, slot);
    }
}

/// Infection replaces the living type in-place; no death/drop/XP credit occurs.
pub(crate) fn infect(state: &mut TickState, slot: EntitySlot, attacker: Option<EntityId>) -> bool {
    if state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .is_none_or(|v| v.wandering || v.conversion_time.is_some())
    {
        return false;
    }
    let Some(attacker) = attacker.and_then(|id| state.entities.slot_of(id)) else {
        return false;
    };
    let EntityKind::Mob {
        type_id: attacker_type,
    } = state.entities.body(attacker).kind
    else {
        return false;
    };
    if !state.registry.blueprint(attacker_type).is_some_and(|b| {
        matches!(
            b.name.as_str(),
            "minecraft:zombie" | "minecraft:zombie_villager"
        )
    }) {
        return false;
    }
    let chance = match state.difficulty {
        crate::survival::Difficulty::Hard => true,
        crate::survival::Difficulty::Normal => {
            (state.tick ^ state.entities.row(slot).uuid.as_u128() as u64).is_multiple_of(2)
        }
        _ => false,
    };
    if !chance {
        return false;
    }
    let Some(zombie_type) = type_id(state, "minecraft:zombie_villager") else {
        return false;
    };
    replace_type(state, slot, zombie_type);
    let row = state.entities.row_mut(slot);
    let villager = row.villager.as_mut().expect("observed villager");
    villager.conversion_time = Some(-1);
    villager.conversion_player = None;
    villager.data_finalized = true;
    villager.sleeping = false;
    crate::settlements::allocator::clear_pois(villager);
    villager.partner = None;
    villager.courtship_until = None;
    super::sync_appearance(state, slot);
    true
}

fn replace_type(state: &mut TickState, slot: EntitySlot, type_id: u32) {
    let blueprint = state
        .registry
        .blueprint(type_id)
        .expect("resolved registered blueprint");
    state.entities.set_half_width(slot, blueprint.half_width);
    state.entities.set_kind(slot, EntityKind::Mob { type_id });
    let (body, row) = state.entities.parts_mut(slot);
    body.alive = true;
    row.health = blueprint.params.max_health;
    body.movement = blueprint.movement;
    body.height = blueprint.height;
    row.speed = blueprint.params.speed;
    row.attack_damage = blueprint.params.attack_damage;
    row.sight_range = blueprint.sensors.sight_range;
    row.attack_reach = blueprint.sensors.attack_reach;
    body.hurt_at = None;
    row.last_hurt = 0.0;
    row.last_player_hit = None;
    std::sync::Arc::make_mut(&mut row.appearance).health = row.health;
    state.appearance_changed.push(body.id);
}

pub(crate) fn interact(
    state: &mut TickState,
    player_id: EntityId,
    slot: EntitySlot,
    hand: Hand,
) -> bool {
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    if !body.alive
        || row
            .villager
            .as_ref()
            .is_none_or(|v| v.conversion_time != Some(-1))
        || !row.active_effects.has(EffectId::Weakness)
    {
        return false;
    }
    let Some(player) = state.players.get(&player_id) else {
        return false;
    };
    if player.spectator() {
        return false;
    }
    let held_slot = if hand == Hand::Off {
        40
    } else {
        usize::from(player.inventory.selected)
    };
    let Some(held) = player.inventory.slots.get(held_slot) else {
        return false;
    };
    let apple = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:golden_apple").expect("static item"));
    if held.is_empty() || Some(held.item_id) != apple {
        return false;
    }
    if crate::tick::held_debit_below_floor(state, player_id, hand, 1) {
        return false;
    }
    let uuid = state
        .entities
        .slot_of(player_id)
        .map(|slot| state.entities.row(slot).uuid);
    let creative = player.creative();
    let interaction_item = crate::advancements::item_snapshot(state, held);
    let duration = 3600 + ((state.tick ^ row.uuid.as_u128() as u64) % 2401) as i32;
    let player = state.players.get_mut(&player_id).expect("validated player");
    if !creative {
        player.inventory.slots[held_slot].count -= 1;
        if player.inventory.slots[held_slot].count == 0 {
            player.inventory.slots[held_slot] = mc_data::ItemStack::EMPTY;
        }
    }
    let change = player.inventory_contents();
    if let Some(connection) = state.connections.entity_connection(player_id) {
        state.push_player(connection, change);
    }
    let row = state.entities.row_mut(slot);
    row.active_effects.clear(Some(EffectId::Weakness));
    row.active_effects
        .add(ActiveEffect::new(EffectId::Strength, duration, 0));
    let villager = row.villager.as_mut().expect("validated zombie");
    villager.conversion_time = Some(duration);
    villager.conversion_player = uuid;
    super::sync_appearance(state, slot);
    let entity = crate::advancements::entity_snapshot(state, state.entities.body(slot).id);
    crate::advancements::trigger(
        state,
        player_id,
        "minecraft:player_interacted_with_entity",
        serde_json::json!({ "item": interaction_item, "entity": entity }),
    );
    true
}

pub(crate) fn tick(state: &mut TickState) {
    let Some(villager_type) = type_id(state, "minecraft:villager") else {
        return;
    };
    let zombie_villager_type = type_id(state, "minecraft:zombie_villager");
    let trader_type = type_id(state, "minecraft:wandering_trader");
    // Admission is the exact curing-component kind set; only the dynamic
    // alive/timer filter stays per slot. The merge restores slot order.
    let converting: Vec<_> = state
        .entities
        .mob_slots_matching(|id| {
            id == villager_type || Some(id) == zombie_villager_type || Some(id) == trader_type
        })
        .into_iter()
        .filter(|slot| {
            let body = state.entities.body(*slot);
            let row = state.entities.row(*slot);
            body.alive
                && row
                    .villager
                    .as_ref()
                    .and_then(|villager| villager.conversion_time)
                    .is_some_and(|remaining| remaining >= 0)
        })
        .collect();
    for slot in converting {
        let body = state.entities.body(slot);
        if !body.alive {
            continue;
        }
        let row = state.entities.row(slot);
        let Some(remaining) = row
            .villager
            .as_ref()
            .and_then(|villager| villager.conversion_time)
            .filter(|remaining| *remaining >= 0)
        else {
            continue;
        };
        let villager = state.entities.row_mut(slot).villager.as_mut().unwrap();
        let remaining = remaining - 1;
        villager.conversion_time = Some(remaining);
        if remaining > 0 {
            continue;
        }
        let responsible = villager.conversion_player;
        let zombie = crate::advancements::entity_snapshot(state, state.entities.body(slot).id);
        let row = state.entities.row_mut(slot);
        let villager = row.villager.as_mut().expect("converting villager");
        villager.conversion_time = None;
        if let Some(player) = villager.conversion_player.take() {
            for (kind, value) in [("major_positive", 20), ("minor_positive", 25)] {
                if let Some(gossip) = villager
                    .gossips
                    .iter_mut()
                    .find(|g| g.target == player && g.kind == kind)
                {
                    gossip.value = gossip
                        .value
                        .saturating_add(value)
                        .min(if kind == "major_positive" { 20 } else { 25 });
                } else {
                    villager.gossips.push(Gossip {
                        target: player,
                        kind: kind.to_owned(),
                        value,
                    });
                }
            }
        }
        row.active_effects.clear(Some(EffectId::Strength));
        row.active_effects
            .add(ActiveEffect::new(EffectId::Nausea, 200, 0));
        replace_type(state, slot, villager_type);
        super::sync_appearance(state, slot);
        if let Some(player_slot) = responsible.and_then(|uuid| state.entities.slot_of_uuid(uuid)) {
            let player = state.entities.body(player_slot).id;
            let villager =
                crate::advancements::entity_snapshot(state, state.entities.body(slot).id);
            crate::advancements::trigger(
                state,
                player,
                "minecraft:cured_zombie_villager",
                serde_json::json!({ "zombie": zombie, "villager": villager }),
            );
        }
    }
}
