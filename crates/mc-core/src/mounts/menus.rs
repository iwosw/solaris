//! Mount menus borrow equipment and cargo from the entity, never from a menu snapshot.
use crate::boundary::{ChangeSet, ClickMode, EntityId, PlayerChange};
use crate::entities::EntitySlot;
use crate::inventory::{MenuKind, MountArmor};
use crate::tick::TickState;
use mc_data::ItemStack;
use std::sync::Arc;

pub(super) fn kind(state: &TickState, slot: EntitySlot) -> Option<MenuKind> {
    let name = super::name(state, slot);
    let armor = match name {
        "minecraft:horse" => MountArmor::Horse,
        "minecraft:llama" | "minecraft:trader_llama" => MountArmor::Llama,
        "minecraft:donkey" | "minecraft:mule" | "minecraft:camel" => MountArmor::None,
        _ => return None,
    };
    let mount = &state.entities.row(slot).entity_life.mount;
    let columns = if !mount.chested {
        0
    } else if armor == MountArmor::Llama {
        mount.strength.clamp(1, 5)
    } else {
        5
    };
    Some(MenuKind::Mount {
        entity: state.entities.body(slot).id,
        columns,
        saddle: armor != MountArmor::Llama,
        armor,
    })
}
pub(crate) fn target(state: &TickState, actor: EntityId) -> Option<EntityId> {
    match state.players.get(&actor)?.open.as_ref()?.kind {
        MenuKind::Mount { entity, .. } => Some(entity),
        _ => None,
    }
}
fn available(state: &TickState, actor: EntityId, target: EntityId) -> bool {
    let (Some(actor), Some(slot)) = (
        state.entities.slot_of(actor),
        state.entities.slot_of(target),
    ) else {
        return false;
    };
    let row = state.entities.row(slot);
    state.entities.body(slot).alive
        && row.entity_life.tamed
        && crate::entity_life::adult(row)
        && kind(state, slot).is_some()
        && state
            .entities
            .pos(actor)
            .into_iter()
            .zip(state.entities.pos(slot))
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            <= 64.0
}
fn slots(state: &TickState, slot: EntitySlot) -> Vec<ItemStack> {
    let row = state.entities.row(slot);
    let size = kind(state, slot).unwrap().container_size();
    let mut result = Vec::with_capacity(size);
    result.push(
        row.loot
            .saddle
            .as_deref()
            .cloned()
            .unwrap_or(ItemStack::EMPTY),
    );
    result.push(
        row.loot
            .body_armor
            .as_deref()
            .cloned()
            .unwrap_or(ItemStack::EMPTY),
    );
    result.extend(row.entity_life.mount.cargo.iter().cloned());
    result.resize(size, ItemStack::EMPTY);
    result
}
pub(super) fn open(state: &mut TickState, actor: EntityId, target: EntityId) {
    if !available(state, actor, target) {
        return;
    }
    let slot = state.entities.slot_of(target).unwrap();
    let kind = kind(state, slot).unwrap();
    let contents = slots(state, slot);
    let registry = Arc::clone(&state.registry);
    let Some(player) = state.players.get_mut(&actor) else {
        return;
    };
    if player.spectator() || player.vitals.is_dead() {
        return;
    }
    let connection = player.connection;
    let effects = player.open_menu(&registry.item_rules(), kind, String::new(), &contents);
    state.open_containers.remove(&actor);
    for change in effects.changes {
        state.push_player(connection, change);
    }
    let position = state.entities.pos(state.entities.slot_of(actor).unwrap());
    state
        .merge_life_drops
        .extend(effects.dropped.into_iter().map(|stack| (position, stack)));
}
fn close(state: &mut TickState, actor: EntityId, changes: &mut ChangeSet) {
    let registry = Arc::clone(&state.registry);
    let Some(player) = state.players.get_mut(&actor) else {
        return;
    };
    let connection = player.connection;
    let window = player.open.as_ref().unwrap().window;
    let mut effects = player.close(&registry.item_rules());
    effects
        .changes
        .insert(0, PlayerChange::MenuClosed { window });
    crate::tick::apply_player_effects(state, connection, actor, effects, changes);
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn click(
    state: &mut TickState,
    actor: EntityId,
    window: u8,
    state_id: i32,
    index: i16,
    button: i8,
    mode: ClickMode,
    changes: &mut ChangeSet,
) {
    let Some(target) = target(state, actor) else {
        return;
    };
    if !available(state, actor, target) {
        close(state, actor, changes);
        return;
    }
    let slot = state.entities.slot_of(target).unwrap();
    let mut contents = slots(state, slot);
    let registry = Arc::clone(&state.registry);
    let player = state.players.get(&actor).unwrap();
    let connection = player.connection;
    let native = |player: &mut crate::player::PlayerState, contents: &mut [ItemStack]| {
        player.click(
            &registry.item_rules(),
            window,
            state_id,
            index,
            button,
            mode,
            contents,
        )
    };
    let effects = if crate::plugin_inventories::has_reservations_for_player(state, actor) {
        let plan = player.plan_inventory_transition(&contents, native);
        if crate::plugin_inventories::below_player_floors(state, actor, plan.player_slots()) {
            player.inventory_transition_refused(&contents)
        } else {
            state
                .players
                .get_mut(&actor)
                .unwrap()
                .apply_inventory_transition(plan, &mut contents)
        }
    } else {
        native(state.players.get_mut(&actor).unwrap(), &mut contents)
    };
    let changed = effects.container_changed;
    if changed {
        let row = state.entities.row_mut(slot);
        let mut contents = contents.into_iter();
        let saddle = contents.next().unwrap();
        let armor = contents.next().unwrap();
        row.loot.saddle = (!saddle.is_empty()).then(|| Box::new(saddle));
        row.loot.body_armor = (!armor.is_empty()).then(|| Box::new(armor));
        row.entity_life.mount.cargo = contents.collect();
        let appearance = Arc::make_mut(&mut row.appearance);
        appearance.saddle = row.loot.saddle.as_deref().cloned();
        appearance.body_armor = row.loot.body_armor.as_deref().cloned();
        state.appearance_changed.push(target);
    }
    crate::tick::apply_player_effects(state, connection, actor, effects, changes);
    if changed {
        sync(state, target);
    }
}
pub(super) fn sync(state: &mut TickState, mount: EntityId) {
    let Some(slot) = state.entities.slot_of(mount) else {
        return;
    };
    let viewers: Vec<_> = state
        .players
        .keys()
        .copied()
        .filter(|id| target(state, *id) == Some(mount))
        .collect();
    if viewers.is_empty() {
        return;
    }
    let contents = slots(state, slot);
    for id in viewers {
        let player = state.players.get_mut(&id).unwrap();
        let connection = player.connection;
        let menu = player.open.as_mut().unwrap();
        menu.state_id = (menu.state_id + 1) & 0x7fff;
        let updates = player.sync_container_view(&contents);
        for update in updates {
            state.push_player(connection, update);
        }
    }
}
pub(crate) fn maintain(state: &mut TickState, changes: &mut ChangeSet) {
    let viewers: Vec<_> = state
        .players
        .keys()
        .copied()
        .filter_map(|id| target(state, id).map(|target| (id, target)))
        .collect();
    for (actor, target) in viewers {
        if !available(state, actor, target) {
            close(state, actor, changes);
        }
    }
}
