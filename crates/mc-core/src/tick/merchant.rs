//! Merchant windows are player-owned; the offer and both payment debits are
//! committed together on the single tick owner, never on an I/O task.
use super::{TickState, apply_player_effects, within_entity_reach};
use crate::boundary::{ChangeSet, ClickMode, ConnectionId, EntityId, Hand, PlayerChange};
use crate::inventory::{MenuKind, MerchantMenu};
use crate::player::PlayerEffects;
use crate::villager::{VillagerState, trades};
use std::sync::Arc;

/// Vanilla passes the display name only; MerchantScreen adds the level label.
fn menu_title(
    state: &TickState,
    slot: crate::entities::EntitySlot,
    villager: &VillagerState,
) -> String {
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    if let Some(name) = &row.appearance.custom_name {
        return serde_json::to_string(name).expect("string serialization cannot fail");
    }
    if matches!(
        body.kind,
        crate::entities::EntityKind::Mob { type_id }
            if mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .is_some_and(|kind| kind.name == "minecraft:wandering_trader")
    ) {
        return r#"{"translate":"entity.minecraft.wandering_trader"}"#.into();
    }
    format!(
        r#"{{"translate":"entity.minecraft.villager.{}"}}"#,
        villager.data.profession.title_name()
    )
}

pub(super) fn is_open(state: &TickState, player: EntityId) -> bool {
    state
        .players
        .get(&player)
        .and_then(|p| p.open.as_ref())
        .is_some_and(|m| m.kind == MenuKind::Merchant)
}

fn target(state: &TickState, player: EntityId) -> Option<EntityId> {
    state
        .players
        .get(&player)?
        .open
        .as_ref()?
        .merchant
        .map(|m| m.entity)
}

fn available(state: &TickState, player: EntityId, merchant: EntityId) -> bool {
    within_entity_reach(state, player, merchant)
        && state.entities.slot_of(merchant).is_some_and(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            body.alive && row.health > 0.0 && row.villager.as_deref().is_some_and(trades::can_trade)
        })
}

fn contents(state: &TickState, player: EntityId) -> Option<PlayerChange> {
    let p = state.players.get(&player)?;
    let menu = p.open.as_ref()?;
    Some(PlayerChange::MenuContents {
        window: menu.window,
        state_id: menu.state_id,
        slots: menu
            .contents(&p.inventory, &[])
            .into_iter()
            .map(|s| (!s.is_empty()).then_some(s))
            .collect(),
        cursor: (!menu.carried.is_empty()).then(|| menu.carried.clone()),
    })
}

fn offers(
    villager: &VillagerState,
    window: u8,
    customer: uuid::Uuid,
    hero: Option<u8>,
) -> PlayerChange {
    let reputation = trades::reputation(villager, customer);
    let offers = trades::displayed_offers(villager)
        .map(|(_, offer)| {
            let mut offer = offer.clone();
            offer.special_price = offer
                .special_price
                .saturating_sub((reputation as f32 * offer.price_multiplier).floor() as i32);
            offer.special_price = offer
                .special_price
                .saturating_sub(trades::hero_discount(offer.cost_a.count, hero));
            offer
        })
        .collect();
    PlayerChange::MerchantOffers {
        window,
        offers,
        level: villager.data.level,
        xp: villager.xp,
        can_restock: villager.job_site.is_some(),
    }
}

fn publish(state: &mut TickState, player: EntityId, connection: ConnectionId) {
    let Some(customer) = state.connections.uuid_of(connection) else {
        return;
    };
    let Some(merchant) = target(state, player) else {
        return;
    };
    let Some(slot) = state.entities.slot_of(merchant) else {
        return;
    };
    let Some(villager) = state.entities.row(slot).villager.as_ref() else {
        return;
    };
    let Some(window) = state
        .players
        .get(&player)
        .and_then(|p| p.open.as_ref())
        .map(|m| m.window)
    else {
        return;
    };
    let hero = state.players[&player]
        .active_effects
        .get(crate::effects::EffectId::HeroOfTheVillage)
        .map(|e| e.amplifier);
    let offer_change = offers(villager, window, customer, hero);
    let last_restock = villager.last_restock;
    let reputation = trades::reputation(villager, customer);
    if let Some(observer) = state
        .players
        .get_mut(&player)
        .and_then(|p| p.open.as_mut())
        .and_then(|m| m.merchant.as_mut())
    {
        observer.last_restock = last_restock;
        observer.reputation = reputation;
        observer.hero = hero;
    }
    if let Some(change) = contents(state, player) {
        state.push_player(connection, change);
    }
    state.push_player(connection, offer_change);
}

/// Returns true for any villager interaction, even an ineligible one, so that
/// babies, unemployed villagers, nitwits and zombies cannot fall through.
pub(super) fn open(
    state: &mut TickState,
    player: EntityId,
    merchant: EntityId,
    connection: ConnectionId,
    hand: Hand,
    changes: &mut ChangeSet,
) -> bool {
    let Some(slot) = state.entities.slot_of(merchant) else {
        return false;
    };
    if state.entities.row(slot).villager.is_none() {
        return false;
    }
    if !available(state, player, merchant)
        || state.players.get(&player).is_none_or(|p| p.spectator())
        || state.players.iter().any(|(&id, p)| {
            id != player
                && p.open
                    .as_ref()
                    .and_then(|m| m.merchant)
                    .is_some_and(|m| m.entity == merchant)
        })
    {
        return true;
    }
    let registry = Arc::clone(&state.registry);
    if let Some(villager) = state.entities.row_mut(slot).villager.as_mut() {
        trades::generate(villager, &registry);
    }
    let Some(villager) = state.entities.row(slot).villager.as_ref() else {
        return true;
    };
    let selected = trades::displayed_offers(villager)
        .next()
        .map_or(villager.offers.len(), |(index, _)| index);
    let title = menu_title(state, slot, villager);
    let held_slot = if hand == Hand::Off {
        40
    } else {
        usize::from(state.players[&player].inventory.selected)
    };
    let item = crate::advancements::item_snapshot(
        state,
        &state.players[&player].inventory.slots[held_slot],
    );
    state.open_containers.remove(&player);
    let Some(p) = state.players.get_mut(&player) else {
        return true;
    };
    let effects = p.open_menu(&registry.item_rules(), MenuKind::Merchant, title, &[]);
    if let Some(menu) = p.open.as_mut() {
        menu.merchant = Some(MerchantMenu {
            entity: merchant,
            selected,
            last_restock: 0,
            reputation: 0,
            hero: p
                .active_effects
                .get(crate::effects::EffectId::HeroOfTheVillage)
                .map(|e| e.amplifier),
        });
    }
    if hand == Hand::Main {
        p.statistics
            .increment_custom(mc_data::statistics::CustomStat::TalkedToVillager, 1);
    }
    apply_player_effects(state, connection, player, effects, changes);
    publish(state, player, connection);
    let entity = crate::advancements::entity_snapshot(state, merchant);
    crate::advancements::trigger(
        state,
        player,
        "minecraft:player_interacted_with_entity",
        serde_json::json!({ "item": item, "entity": entity }),
    );
    true
}

pub(super) fn select(
    state: &mut TickState,
    player: EntityId,
    connection: ConnectionId,
    offer: i32,
    changes: &mut ChangeSet,
) {
    let Some(merchant) = target(state, player) else {
        return;
    };
    if !available(state, player, merchant) {
        close(state, player, connection, changes);
        return;
    }
    let Some(customer) = state.connections.uuid_of(connection) else {
        return;
    };
    let Some(slot) = state.entities.slot_of(merchant) else {
        return;
    };
    let Some(villager) = state.entities.row(slot).villager.as_ref() else {
        return;
    };
    let Ok(index) = usize::try_from(offer) else {
        return;
    };
    let Some((index, _)) = trades::displayed_offers(villager).nth(index) else {
        return;
    };
    let registry = Arc::clone(&state.registry);
    let Some(p) = state.players.get(&player).filter(|p| !p.spectator()) else {
        return;
    };
    if p.open.is_none() {
        return;
    }
    let hero = p
        .active_effects
        .get(crate::effects::EffectId::HeroOfTheVillage)
        .map(|effect| effect.amplifier);
    let native = |p: &mut crate::player::PlayerState, _: &mut [mc_data::ItemStack]| {
        let menu = p.open.as_mut().expect("validated merchant menu");
        if let Some(merchant) = menu.merchant.as_mut() {
            merchant.hero = hero;
        }
        trades::select(
            menu,
            &mut p.inventory,
            villager,
            &registry.item_rules(),
            customer,
            index,
        );
        menu.state_id = (menu.state_id + 1) & 0x7fff;
        PlayerEffects::default()
    };
    if crate::plugin_inventories::has_reservations_for_player(state, player) {
        let plan = p.plan_inventory_transition(&[], native);
        if crate::plugin_inventories::below_player_floors(state, player, plan.player_slots()) {
            let effects = p.inventory_transition_refused(&[]);
            apply_player_effects(state, connection, player, effects, changes);
            return;
        }
        state
            .players
            .get_mut(&player)
            .expect("planned player")
            .apply_inventory_transition(plan, &mut []);
    } else {
        native(
            state.players.get_mut(&player).expect("validated player"),
            &mut [],
        );
    }
    publish(state, player, connection);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn click(
    state: &mut TickState,
    player: EntityId,
    connection: ConnectionId,
    window: u8,
    revision: i32,
    slot: i16,
    button: i8,
    mode: ClickMode,
    changes: &mut ChangeSet,
) {
    let Some(merchant) = target(state, player) else {
        return;
    };
    if !available(state, player, merchant) {
        close(state, player, connection, changes);
        return;
    }
    let Some(customer) = state.connections.uuid_of(connection) else {
        return;
    };
    let Some(merchant_slot) = state.entities.slot_of(merchant) else {
        return;
    };
    let registry = Arc::clone(&state.registry);
    let Some(p) = state.players.get(&player) else {
        return;
    };
    if p.open.as_ref().is_none_or(|menu| menu.window != window) {
        return;
    }
    // Vanilla executes stale-state clicks against current custody, then sends
    // the full authoritative image. Refusal must not publish a candidate.
    if p.spectator() {
        publish(state, player, connection);
        return;
    }
    let hero = p
        .active_effects
        .get(crate::effects::EffectId::HeroOfTheVillage)
        .map(|effect| effect.amplifier);
    let guarded = crate::plugin_inventories::has_reservations_for_player(state, player);
    let (effects, experience, traded_item) = if slot == 2 {
        if guarded {
            let Some(villager) = state.entities.row(merchant_slot).villager.as_ref() else {
                return;
            };
            // Only the actual merchant's offers/XP/gossip RNG are staged.
            // No entity store, tick state or world is cloned.
            let mut candidate_villager = villager.clone();
            let mut experience = 0;
            let mut traded_item = None;
            let plan = p.plan_inventory_transition(&[], |candidate, _| {
                let (effects, award, item) = result_click(
                    candidate,
                    &mut candidate_villager,
                    &registry,
                    customer,
                    hero,
                    button,
                    mode,
                );
                experience = award;
                traded_item = item;
                effects
            });
            if crate::plugin_inventories::below_player_floors(state, player, plan.player_slots()) {
                let effects = p.inventory_transition_refused(&[]);
                apply_player_effects(state, connection, player, effects, changes);
                return;
            }
            let effects = state
                .players
                .get_mut(&player)
                .expect("planned player")
                .apply_inventory_transition(plan, &mut []);
            state.entities.row_mut(merchant_slot).villager = Some(candidate_villager);
            (effects, experience, traded_item)
        } else {
            let Some(villager) = state.entities.row_mut(merchant_slot).villager.as_mut() else {
                return;
            };
            let p = state.players.get_mut(&player).expect("validated player");
            result_click(p, villager, &registry, customer, hero, button, mode)
        }
    } else {
        let Some(villager) = state.entities.row(merchant_slot).villager.as_ref() else {
            return;
        };
        let native = |p: &mut crate::player::PlayerState, _: &mut [mc_data::ItemStack]| {
            menu_click(
                p, villager, &registry, customer, hero, window, revision, slot, button, mode,
            )
        };
        let effects = if guarded {
            let plan = p.plan_inventory_transition(&[], native);
            if crate::plugin_inventories::below_player_floors(state, player, plan.player_slots()) {
                let effects = p.inventory_transition_refused(&[]);
                apply_player_effects(state, connection, player, effects, changes);
                return;
            }
            state
                .players
                .get_mut(&player)
                .expect("planned player")
                .apply_inventory_transition(plan, &mut [])
        } else {
            native(
                state.players.get_mut(&player).expect("validated player"),
                &mut [],
            )
        };
        (effects, 0, None)
    };
    if experience > 0 {
        let position = state.entities.pos(merchant_slot);
        crate::experience_orbs::award(state, position, experience, &mut changes.entities_spawned);
    }
    apply_player_effects(state, connection, player, effects, changes);
    if let Some(stack) = traded_item {
        // A customer changed the offer's uses now; old dormant time must
        // never restock that newly accepted trade.
        let pos = state.entities.pos(merchant_slot);
        state
            .world
            .cancel_simulation_debt(crate::boundary::ChunkPos {
                x: (pos[0].floor() as i32).div_euclid(16),
                z: (pos[2].floor() as i32).div_euclid(16),
            });
        let item = crate::advancements::item_snapshot(state, &stack);
        let villager = crate::advancements::entity_snapshot(state, merchant);
        crate::advancements::trigger(
            state,
            player,
            "minecraft:villager_trade",
            serde_json::json!({ "item": item, "villager": villager }),
        );
    }
    publish(state, player, connection);
}

#[allow(clippy::too_many_arguments)]
fn result_click(
    player: &mut crate::player::PlayerState,
    villager: &mut VillagerState,
    registry: &crate::registry::CoreRegistry,
    customer: uuid::Uuid,
    hero: Option<u8>,
    button: i8,
    mode: ClickMode,
) -> (PlayerEffects, i32, Option<mc_data::ItemStack>) {
    let menu = player.open.as_mut().expect("validated merchant menu");
    if let Some(merchant) = menu.merchant.as_mut() {
        merchant.hero = hero;
    }
    trades::refresh(menu, villager, &registry.item_rules(), customer);
    let crafted = (menu.result.item_id, menu.result.count);
    let outcome = trades::execute(
        menu,
        &mut player.inventory,
        villager,
        registry,
        customer,
        button,
        mode,
    );
    let mut traded_item = None;
    if outcome.trades > 0 {
        traded_item = menu
            .merchant
            .and_then(|m| villager.offers.get(m.selected))
            .map(|offer| offer.result.clone());
        menu.state_id = (menu.state_id + 1) & 0x7fff;
        player.statistics.increment_custom(
            mc_data::statistics::CustomStat::TradedWithVillager,
            outcome.trades,
        );
        player.statistics.increment(
            mc_data::statistics::StatType::Crafted,
            crafted.0,
            crafted.1.saturating_mul(outcome.trades),
        );
    }
    (
        PlayerEffects {
            dropped: outcome.dropped,
            ..PlayerEffects::default()
        },
        outcome.experience,
        traded_item,
    )
}

#[allow(clippy::too_many_arguments)]
fn menu_click(
    player: &mut crate::player::PlayerState,
    villager: &VillagerState,
    registry: &crate::registry::CoreRegistry,
    customer: uuid::Uuid,
    hero: Option<u8>,
    window: u8,
    revision: i32,
    slot: i16,
    button: i8,
    mode: ClickMode,
) -> PlayerEffects {
    let menu = player.open.as_mut().expect("validated merchant menu");
    if let Some(merchant) = menu.merchant.as_mut() {
        merchant.hero = hero;
    }
    trades::refresh(menu, villager, &registry.item_rules(), customer);
    let mut effects = player.click(
        &registry.item_rules(),
        window,
        revision,
        slot,
        button,
        mode,
        &mut [],
    );
    if let Some(menu) = player.open.as_mut() {
        trades::refresh(menu, villager, &registry.item_rules(), customer);
    }
    effects.changes.clear();
    effects
}

fn close(
    state: &mut TickState,
    player: EntityId,
    connection: ConnectionId,
    changes: &mut ChangeSet,
) {
    let registry = Arc::clone(&state.registry);
    let Some(p) = state.players.get_mut(&player) else {
        return;
    };
    let Some(window) = p.open.as_ref().map(|menu| menu.window) else {
        return;
    };
    let mut effects = p.close(&registry.item_rules());
    effects
        .changes
        .insert(0, PlayerChange::MenuClosed { window });
    apply_player_effects(state, connection, player, effects, changes);
}

/// Called after villager AI. Close invalid customers and refresh result/offers
/// when a work restock changes an open merchant's authoritative projection.
pub(crate) fn maintain(state: &mut TickState, changes: &mut ChangeSet) {
    let customers: Vec<_> = state
        .players
        .iter()
        .filter_map(|(&id, p)| {
            p.open
                .as_ref()
                .and_then(|m| m.merchant)
                .map(|m| (id, p.connection, m.entity))
        })
        .collect();
    for (player, connection, merchant) in customers {
        if !available(state, player, merchant) {
            close(state, player, connection, changes);
            continue;
        }
        let Some(customer) = state.connections.uuid_of(connection) else {
            continue;
        };
        let Some(slot) = state.entities.slot_of(merchant) else {
            continue;
        };
        let Some(villager) = state.entities.row(slot).villager.as_ref() else {
            continue;
        };
        let Some(p) = state.players.get_mut(&player) else {
            continue;
        };
        let hero = p
            .active_effects
            .get(crate::effects::EffectId::HeroOfTheVillage)
            .map(|e| e.amplifier);
        let Some(menu) = p.open.as_mut() else {
            continue;
        };
        let hero_changed = menu.merchant.is_some_and(|m| m.hero != hero);
        if let Some(merchant) = menu.merchant.as_mut() {
            merchant.hero = hero;
        }
        let old = menu.result.clone();
        trades::refresh(menu, villager, &state.registry.item_rules(), customer);
        let changed = menu.merchant.is_some_and(|m| {
            m.last_restock != villager.last_restock
                || m.reputation != trades::reputation(villager, customer)
        });
        if old != menu.result || changed || hero_changed {
            menu.state_id = (menu.state_id + 1) & 0x7fff;
            publish(state, player, connection);
        }
    }
}

#[cfg(test)]
#[path = "merchant_tests.rs"]
mod tests;
