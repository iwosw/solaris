//! Cartography output is planned without allocating; taking it commits map+items.
use super::{SavedMap, map_id, set_map_id};
use crate::boundary::{ChangeSet, ClickMode, EntityId};
use crate::inventory::{ItemRules, MenuKind};
use crate::tick::TickState;
use mc_data::ItemStack;
use std::sync::Arc;

pub(crate) fn allowed(rules: &ItemRules<'_>, slot: usize, stack: &ItemStack) -> bool {
    if stack.is_empty() {
        return true;
    }
    let Some(name) = crate::inventory::anvil::logical_item(rules, stack) else {
        return false;
    };
    if rules.items.name_of(stack.item_id) != Some(name) {
        return false;
    }
    match slot {
        0 => name.as_str() == "minecraft:filled_map" && map_id(stack).is_some(),
        1 => matches!(
            name.as_str(),
            "minecraft:paper" | "minecraft:map" | "minecraft:glass_pane"
        ),
        _ => false,
    }
}
pub(crate) fn is_open(state: &TickState, actor: EntityId) -> bool {
    state
        .players
        .get(&actor)
        .and_then(|p| p.open.as_ref())
        .is_some_and(|m| m.kind == MenuKind::Cartography)
}
fn output(state: &TickState, actor: EntityId) -> Option<(ItemStack, Option<SavedMap>)> {
    let menu = state.players.get(&actor)?.open.as_ref()?;
    let rules = state.registry.item_rules();
    if menu.grid.len() != 2
        || menu
            .grid
            .iter()
            .enumerate()
            .any(|(i, s)| s.is_empty() || !allowed(&rules, i, s))
    {
        return None;
    }
    let id = map_id(&menu.grid[0])?;
    let map = state.maps.records.get(&id)?;
    let ingredient = state.registry.items.name_of(menu.grid[1].item_id)?.as_str();
    let mut result = menu.grid[0].clone();
    result.count = 1;
    let next = match ingredient {
        "minecraft:map" => {
            result.count = 2;
            None
        }
        "minecraft:glass_pane" if !map.locked => {
            let mut next = (**map).clone();
            next.locked = true;
            Some(next)
        }
        "minecraft:paper" if !map.locked && map.scale < 4 && map.decorations.is_empty() => {
            let mut next = SavedMap::new(map.center, map.scale + 1, map.dimension).ok()?;
            next.tracking = map.tracking;
            next.unlimited_tracking = map.unlimited_tracking;
            Some(next)
        }
        _ => return None,
    };
    if next.is_some() {
        let id = i32::try_from(state.maps.next_id).ok()?;
        set_map_id(&mut result, id);
    }
    Some((result, next))
}
pub(crate) fn click(
    state: &mut TickState,
    actor: EntityId,
    (window, revision, slot, button, mode): (u8, i32, i16, i8, ClickMode),
    changes: &mut ChangeSet,
) {
    let candidate = output(state, actor);
    if let Some(menu) = state.players.get_mut(&actor).and_then(|p| p.open.as_mut()) {
        menu.result = candidate
            .as_ref()
            .map(|(item, _)| item.clone())
            .unwrap_or(ItemStack::EMPTY);
    }
    let registry = Arc::clone(&state.registry);
    let player = state.players.get(&actor).expect("cartography owner");
    let connection = player.connection;
    let plan = player.plan_inventory_transition(&[], |player, _| {
        player.station_click(
            &registry.enchantments,
            &registry.item_rules(),
            window,
            revision,
            slot,
            button,
            mode,
            0,
        )
    });
    if crate::plugin_inventories::below_player_floors(state, actor, plan.player_slots()) {
        let effects = player.inventory_transition_refused(&[]);
        crate::tick::apply_player_effects(state, connection, actor, effects, changes);
        return;
    }
    let mut effects = state
        .players
        .get_mut(&actor)
        .unwrap()
        .apply_inventory_transition(plan, &mut []);
    if effects.station_output_taken
        && let Some((item, Some(map))) = candidate
    {
        let id = state
            .maps
            .allocate(map)
            .expect("validated cartography allocation");
        debug_assert_eq!(map_id(&item), Some(id));
    }
    let result = output(state, actor)
        .map(|(item, _)| item)
        .unwrap_or(ItemStack::EMPTY);
    let player = state.players.get_mut(&actor).unwrap();
    if let Some(menu) = &mut player.open {
        menu.result = result;
    }
    effects.changes = player.inventory_transition_refused(&[]).changes;
    crate::tick::apply_player_effects(state, connection, actor, effects, changes);
}
