use crate::boundary::{BlockPos, ChangeSet, ConnectionId, EntityId};
use crate::tick::{
    TickState, apply_player_effects, container_cells, container_slots,
    inventory_transition_refused, merchant, stations, store_container_slots,
};
use std::sync::Arc;

pub(in crate::tick) fn select_slot(
    state: &mut TickState,
    (connection, slot): (&ConnectionId, &u8),
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if state.entities.slot_of(entity).is_none() {
        return;
    }
    if let Some(player) = state.players.get_mut(&entity) {
        let changed = *slot <= 8 && *slot != player.inventory.selected;
        player.select_slot(*slot);
        if changed {
            crate::projectiles::weapons::cancel_main(state, entity);
        }
    }
}

pub(in crate::tick) fn container_click(
    state: &mut TickState,
    (connection, window, state_id, slot, button, mode): (
        &ConnectionId,
        &u8,
        &i32,
        &i16,
        &i8,
        &crate::boundary::ClickMode,
    ),
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if state.entities.slot_of(entity).is_none() {
        return;
    }
    if crate::ender_chest::is_open(state, entity) {
        crate::ender_chest::click(
            state,
            entity,
            *connection,
            *window,
            *state_id,
            *slot,
            *button,
            *mode,
            changes,
        );
        return;
    }
    if crate::vehicles::is_open(state, entity) {
        crate::vehicles::click(
            state, entity, *window, *state_id, *slot, *button, *mode, changes,
        );
        return;
    }
    if merchant::is_open(state, entity) {
        merchant::click(
            state,
            entity,
            *connection,
            *window,
            *state_id,
            *slot,
            *button,
            *mode,
            changes,
        );
        return;
    }
    if crate::maps::cartography::is_open(state, entity) {
        crate::maps::cartography::click(state, entity, (*window, *state_id, *slot, *button, *mode), changes);
        return;
    }
    if state
        .players
        .get(&entity)
        .and_then(|player| player.open.as_ref())
        .is_some_and(|menu| menu.kind.is_station())
    {
        let registry = Arc::clone(&state.registry);
        let bookshelves = stations::bookshelves_for_player(state, entity);
        let native = |player: &mut crate::player::PlayerState, _: &mut [mc_data::ItemStack]| {
            player.station_click(
                &registry.enchantments,
                &registry.item_rules(),
                *window,
                *state_id,
                *slot,
                *button,
                *mode,
                bookshelves,
            )
        };
        let effects = if crate::plugin_inventories::has_reservations_for_player(state, entity) {
            let Some(player) = state.players.get(&entity) else {
                return;
            };
            let plan = player.plan_inventory_transition(&[], native);
            if crate::plugin_inventories::below_player_floors(state, entity, plan.player_slots()) {
                player.inventory_transition_refused(&[])
            } else {
                state
                    .players
                    .get_mut(&entity)
                    .expect("planned player")
                    .apply_inventory_transition(plan, &mut [])
            }
        } else {
            state
                .players
                .get_mut(&entity)
                .map(|player| native(player, &mut []))
                .unwrap_or_default()
        };
        apply_player_effects(state, *connection, entity, effects, changes);
        return;
    }
    // Container clicks edit the open block entity's storage
    // (writeback when container_changed); window 0 uses no
    // container.
    let open_cells = state
        .open_containers
        .get(&entity)
        .map(|open| container_cells(state, *open))
        .unwrap_or_default();
    let registry = Arc::clone(&state.registry);
    let rules = registry.item_rules();
    let mut container = container_slots(state, &open_cells);
    // An open menu retains its original shape, but unloaded double-chest
    // partners (or missing storage) can shrink the reconstructed backing slice.
    // Close/refund custody before either click execution or reservation planning.
    let invalid_window = state
        .players
        .get(&entity)
        .and_then(|player| player.open.as_ref())
        .filter(|menu| {
            menu.kind.container_size() != container.len()
                || open_cells
                    .iter()
                    .any(|&(x, y, z)| state.world.block_at(x, y, z).is_none())
        })
        .map(|menu| menu.window);
    if let Some(window) = invalid_window {
        container_close(state, connection, changes);
        state.push_player(
            *connection,
            crate::boundary::PlayerChange::MenuClosed { window },
        );
        return;
    }
    let guarded = crate::plugin_inventories::has_reservations_for_player(state, entity)
        || open_cells.iter().any(|&(x, y, z)| {
            crate::plugin_inventories::has_reservations_for_block(state, BlockPos { x, y, z })
        });
    let native = |player: &mut crate::player::PlayerState, container: &mut [mc_data::ItemStack]| {
        player.click(&rules, *window, *state_id, *slot, *button, *mode, container)
    };
    let effects = if guarded {
        let Some(player) = state.players.get(&entity) else {
            return;
        };
        let plan = player.plan_inventory_transition(&container, native);
        // One concatenated post-image covers BOTH double-chest
        // halves; testing a half against the other's old image
        // would incorrectly reject quantity-preserving reorders.
        let below_container = state.open_containers.get(&entity).is_some_and(|pos| {
            crate::plugin_inventories::below_floors(
                state,
                &mc_script::ScriptInventoryEndpoint::Container {
                    dimension: state.dimension.name().into(),
                    position: mc_script::ScriptBlockPosition::new(pos.0, pos.1, pos.2),
                },
                plan.container_slots(),
            )
        });
        if below_container
            || crate::plugin_inventories::below_player_floors(state, entity, plan.player_slots())
        {
            let effects = player.inventory_transition_refused(&container);
            apply_player_effects(state, *connection, entity, effects, changes);
            return;
        }
        state
            .players
            .get_mut(&entity)
            .expect("planned player")
            .apply_inventory_transition(plan, &mut container)
    } else {
        state
            .players
            .get_mut(&entity)
            .map(|player| native(player, &mut container))
            .unwrap_or_default()
    };
    if effects.container_changed {
        crate::block_log::note_container(state, entity, &open_cells, &container);
        store_container_slots(state, &open_cells, &container);
    }
    if let Some(pos) = state.open_containers.get(&entity)
        && let Some(progress) = state
            .block_entities
            .get_mut(pos)
            .and_then(|record| record.furnace.as_mut())
    {
        if let Some((before, after)) = &effects.furnace_input_change {
            progress.input_replaced(before, after);
        }
        if effects.furnace_output_taken {
            let award = progress.take_experience(&rules, state.tick);
            if let Some(slot) = state.entities.slot_of(entity) {
                let pos = state.entities.pos(slot);
                crate::experience_orbs::award(state, pos, award, &mut changes.entities_spawned);
            }
        }
    }
    // The actor keeps its own click diff (slot/cursor deltas);
    // drops ride `changes.entities_spawned` via apply_player_effects.
    apply_player_effects(state, *connection, entity, effects, changes);
    // Shared-container fan-out: every OTHER viewer of this
    // container gets the full fresh image (with their own
    // connection id and a bumped state-id so a stale click can
    // never silently take an already-taken item). The actor is
    // excluded: it already got its diff above.
    sync_container_viewers(state, entity, &open_cells, &container);
}

fn sync_container_viewers(
    state: &mut TickState,
    entity: EntityId,
    open_cells: &[(i32, i32, i32)],
    container: &[mc_data::ItemStack],
) {
    if !open_cells.is_empty() && container_slots(state, open_cells) == container {
        let viewers: Vec<(EntityId, ConnectionId)> = state
            .open_containers
            .iter()
            .filter(|(viewer, viewer_pos)| {
                **viewer != entity && container_cells(state, **viewer_pos) == open_cells
            })
            .filter_map(|(viewer, _)| {
                state
                    .connections
                    .entity_connection(*viewer)
                    .map(|viewer_conn| (*viewer, viewer_conn))
            })
            .collect();
        for (viewer, viewer_conn) in viewers {
            let Some(viewer_state) = state.players.get_mut(&viewer) else {
                continue;
            };
            // Bump the viewer's menu revision so their next
            // click carries a fresh state-id.
            if let Some(menu) = viewer_state.open.as_mut() {
                menu.state_id = (menu.state_id + 1) & 0x7FFF;
            }
            let sync = viewer_state.sync_container_view(&container);
            for change in sync {
                state.push_player(viewer_conn, change);
            }
        }
    }
}

pub(in crate::tick) fn select_trade(
    state: &mut TickState,
    (connection, offer): (&ConnectionId, &i32),
    changes: &mut ChangeSet,
) {
    if let Some(entity) = state.connections.by_connection.get(connection).copied() {
        merchant::select(state, entity, *connection, *offer, changes);
    }
}

pub(in crate::tick) fn container_slot_state(
    state: &mut TickState,
    (connection, window, slot, enabled): (&ConnectionId, &u8, &i32, &bool),
) {
    if let Some(entity) = state.connections.by_connection.get(connection).copied() {
        crate::crafter::slot_state(state, entity, *window, *slot, *enabled);
    }
}

pub(in crate::tick) fn container_button(
    state: &mut TickState,
    (connection, window, button): (&ConnectionId, &u8, &i32),
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if state.entities.slot_of(entity).is_none() {
        return;
    }
    let bookshelves = stations::bookshelves_for_player(state, entity);
    let registry = Arc::clone(&state.registry);
    let native = |player: &mut crate::player::PlayerState, _: &mut [mc_data::ItemStack]| {
        player.station_button(
            &registry.enchantments,
            &registry.item_rules(),
            *window,
            *button,
            bookshelves,
        )
    };
    let effects = if crate::plugin_inventories::has_reservations_for_player(state, entity) {
        let Some(player) = state.players.get(&entity) else {
            return;
        };
        let plan = player.plan_inventory_transition(&[], native);
        if crate::plugin_inventories::below_player_floors(state, entity, plan.player_slots()) {
            player.inventory_transition_refused(&[])
        } else {
            state
                .players
                .get_mut(&entity)
                .expect("planned player")
                .apply_inventory_transition(plan, &mut [])
        }
    } else {
        state
            .players
            .get_mut(&entity)
            .map(|player| native(player, &mut []))
            .unwrap_or_default()
    };
    apply_player_effects(state, *connection, entity, effects, changes);
}

pub(in crate::tick) fn rename_item(
    state: &mut TickState,
    (connection, name): (&ConnectionId, &String),
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if state.entities.slot_of(entity).is_none() {
        return;
    }
    let registry = Arc::clone(&state.registry);
    let native = |player: &mut crate::player::PlayerState, _: &mut [mc_data::ItemStack]| {
        player.rename_item(&registry.enchantments, &registry.item_rules(), name)
    };
    let effects = if crate::plugin_inventories::has_reservations_for_player(state, entity) {
        let Some(player) = state.players.get(&entity) else {
            return;
        };
        let plan = player.plan_inventory_transition(&[], native);
        if crate::plugin_inventories::below_player_floors(state, entity, plan.player_slots()) {
            player.inventory_transition_refused(&[])
        } else {
            state
                .players
                .get_mut(&entity)
                .expect("planned player")
                .apply_inventory_transition(plan, &mut [])
        }
    } else {
        state
            .players
            .get_mut(&entity)
            .map(|player| native(player, &mut []))
            .unwrap_or_default()
    };
    apply_player_effects(state, *connection, entity, effects, changes);
}

pub(in crate::tick) fn container_close(
    state: &mut TickState,
    connection: &ConnectionId,
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if state.entities.slot_of(entity).is_none() {
        return;
    }
    let rules = state.registry.item_rules();
    let effects = state
        .players
        .get_mut(&entity)
        .map(|player| player.close(&rules))
        .unwrap_or_default();
    state.open_containers.remove(&entity);
    apply_player_effects(state, *connection, entity, effects, changes);
}

pub(in crate::tick) fn place_recipe(
    state: &mut TickState,
    (connection, window, recipe, make_all): (&ConnectionId, &u8, &u32, &bool),
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if state.entities.slot_of(entity).is_none() {
        return;
    }
    let registry = Arc::clone(&state.registry);
    let rules = registry.item_rules();
    let open_cells = state
        .open_containers
        .get(&entity)
        .map(|pos| container_cells(state, *pos))
        .unwrap_or_default();
    let mut container = container_slots(state, &open_cells);
    if state
        .players
        .get(&entity)
        .and_then(|p| p.open.as_ref())
        .is_some_and(|menu| menu.kind.container_size() != container.len())
        || open_cells
            .iter()
            .any(|&(x, y, z)| state.world.block_at(x, y, z).is_none())
    {
        container_close(state, connection, changes);
        state.push_player(
            *connection,
            crate::boundary::PlayerChange::MenuClosed { window: *window },
        );
        return;
    }
    let native = |player: &mut crate::player::PlayerState, container: &mut [mc_data::ItemStack]| {
        player.place_recipe(&rules, *window, *recipe, *make_all, container)
    };
    let guarded = crate::plugin_inventories::has_reservations_for_player(state, entity)
        || open_cells.iter().any(|&(x, y, z)| {
            crate::plugin_inventories::has_reservations_for_block(state, BlockPos { x, y, z })
        });
    let effects = if guarded {
        let Some(player) = state.players.get(&entity) else {
            return;
        };
        let plan = player.plan_inventory_transition(&container, native);
        let below_container = state.open_containers.get(&entity).is_some_and(|pos| {
            crate::plugin_inventories::below_floors(
                state,
                &mc_script::ScriptInventoryEndpoint::Container {
                    dimension: state.dimension.name().into(),
                    position: mc_script::ScriptBlockPosition::new(pos.0, pos.1, pos.2),
                },
                plan.container_slots(),
            )
        });
        if below_container
            || crate::plugin_inventories::below_player_floors(state, entity, plan.player_slots())
        {
            player.inventory_transition_refused(&container)
        } else {
            state
                .players
                .get_mut(&entity)
                .expect("planned player")
                .apply_inventory_transition(plan, &mut container)
        }
    } else {
        state
            .players
            .get_mut(&entity)
            .map(|player| native(player, &mut container))
            .unwrap_or_default()
    };
    if effects.container_changed {
        crate::block_log::note_container(state, entity, &open_cells, &container);
        store_container_slots(state, &open_cells, &container);
        if let Some(pos) = state.open_containers.get(&entity)
            && let Some(progress) = state
                .block_entities
                .get_mut(pos)
                .and_then(|record| record.furnace.as_mut())
            && let Some((before, after)) = &effects.furnace_input_change
        {
            progress.input_replaced(before, after);
        }
    }
    apply_player_effects(state, *connection, entity, effects, changes);
    sync_container_viewers(state, entity, &open_cells, &container);
}

pub(in crate::tick) fn swap_hands(
    state: &mut TickState,
    connection: &ConnectionId,
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if state.entities.slot_of(entity).is_none() {
        return;
    }
    if crate::plugin_inventories::has_reservations_for_player(state, entity)
        && state
            .players
            .get(&entity)
            .and_then(|player| player.hand_swap_replacements())
            .is_some_and(|replacements| {
                crate::plugin_inventories::below_player_replacement_floors(
                    state,
                    entity,
                    &replacements,
                )
            })
    {
        let effects = inventory_transition_refused(state, entity);
        apply_player_effects(state, *connection, entity, effects, changes);
        return;
    }
    let effects = state
        .players
        .get_mut(&entity)
        .map(|player| player.swap_hands())
        .unwrap_or_default();
    crate::projectiles::weapons::cancel(state, entity);
    apply_player_effects(state, *connection, entity, effects, changes);
}

pub(in crate::tick) fn creative_slot(
    state: &mut TickState,
    (connection, slot, stack): (&ConnectionId, &i16, &mc_data::ItemStack),
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if state.entities.slot_of(entity).is_none() {
        return;
    }
    let rules = state.registry.item_rules();
    if crate::plugin_inventories::has_reservations_for_player(state, entity)
        && state
            .players
            .get(&entity)
            .and_then(|player| player.creative_slot_replacement(&rules, *slot, stack))
            .is_some_and(|replacement| {
                crate::plugin_inventories::below_player_replacement_floors(
                    state,
                    entity,
                    &[replacement],
                )
            })
    {
        let effects = inventory_transition_refused(state, entity);
        apply_player_effects(state, *connection, entity, effects, changes);
        return;
    }
    let effects = state
        .players
        .get_mut(&entity)
        .map(|player| player.creative_slot(&rules, *slot, stack.clone()))
        .unwrap_or_default();
    apply_player_effects(state, *connection, entity, effects, changes);
}

pub(in crate::tick) fn drop_selected(
    state: &mut TickState,
    (connection, whole_stack): (&ConnectionId, &bool),
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    if state.entities.slot_of(entity).is_none() {
        return;
    }
    if crate::plugin_inventories::has_reservations_for_player(state, entity)
        && state
            .players
            .get(&entity)
            .and_then(|player| player.drop_selected_debit(*whole_stack))
            .is_some_and(|debit| {
                crate::plugin_inventories::below_player_debit_floors(state, entity, &[debit], None)
            })
    {
        let effects = inventory_transition_refused(state, entity);
        apply_player_effects(state, *connection, entity, effects, changes);
        return;
    }
    let effects = state
        .players
        .get_mut(&entity)
        .map(|player| player.drop_selected(*whole_stack))
        .unwrap_or_default();
    let first_drop = changes.entities_spawned.len();
    apply_player_effects(state, *connection, entity, effects, changes);
    crate::item_motion::throw_selected(state, entity, &changes.entities_spawned[first_drop..]);
}

#[cfg(test)]
pub(in crate::tick) fn test_give(
    state: &mut TickState,
    (connection, slot, stack): (&ConnectionId, &usize, &mc_data::ItemStack),
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    let effects = state
        .players
        .get_mut(&entity)
        .map(|player| player.test_set_slot(*slot, stack.clone()))
        .unwrap_or_default();
    apply_player_effects(state, *connection, entity, effects, changes);
}
