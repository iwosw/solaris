use crate::boundary::{BlockPos, ChangeSet, ClickMode, EntityId};
use crate::entities::{EntityKind, EntitySlot, StackRow};
use crate::tick::TickState;
use mc_data::{
    ItemStack,
    inventory_semantics_26_1_2::{can_stack, decrement_stack},
};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct VehicleMenus {
    viewers: BTreeMap<EntityId, (EntityId, u8)>,
}
fn target(state: &TickState, player: EntityId) -> Option<EntityId> {
    let &(target, window) = state.vehicle_menus.viewers.get(&player)?;
    let menu = state.players.get(&player)?.open.as_ref()?;
    (menu.window == window
        && matches!(
            menu.kind,
            crate::inventory::MenuKind::Chest { .. } | crate::inventory::MenuKind::Hopper
        ))
    .then_some(target)
}
pub(crate) fn is_open(state: &TickState, player: EntityId) -> bool {
    target(state, player).is_some()
}

/// The actual currently viewed cargo, borrowed without custody mutation.
pub(crate) fn container_slots_for_player(
    state: &TickState,
    player: EntityId,
) -> Option<&[ItemStack]> {
    let vehicle = target(state, player)?;
    let slot = state.entities.slot_of(vehicle)?;
    Some(&state.entities.row(slot).entity_life.vehicle.as_ref()?.slots)
}
fn available(state: &TickState, player: EntityId, vehicle: EntityId) -> bool {
    let (Some(p), Some(v)) = (
        state.entities.slot_of(player),
        state.entities.slot_of(vehicle),
    ) else {
        return false;
    };
    let pos = state.entities.pos(p);
    let other = state.entities.pos(v);
    state.entities.body(v).alive
        && pos
            .into_iter()
            .zip(other)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            <= 64.0
}
pub(super) fn open(
    state: &mut TickState,
    player: EntityId,
    vehicle: EntityId,
    changes: &mut ChangeSet,
) {
    if !state.players.contains_key(&player) || !available(state, player, vehicle) {
        return;
    }
    let slot = state.entities.slot_of(vehicle).unwrap();
    super::loot::materialize_for_player(state, slot, player);
    let Some(v) = state.entities.row(slot).entity_life.vehicle.as_ref() else {
        return;
    };
    let slots = v.slots.clone();
    let kind = if slots.len() == 5 {
        crate::inventory::MenuKind::Hopper
    } else {
        crate::inventory::MenuKind::Chest { rows: 3 }
    };
    let registry = Arc::clone(&state.registry);
    let Some(p) = state.players.get_mut(&player) else {
        return;
    };
    let connection = p.connection;
    let effects = p.open_menu(&registry.item_rules(), kind, "Vehicle".into(), &slots);
    let window = p.open.as_ref().unwrap().window;
    state.open_containers.remove(&player);
    state
        .vehicle_menus
        .viewers
        .insert(player, (vehicle, window));
    crate::tick::apply_player_effects(state, connection, player, effects, changes);
}
fn close(state: &mut TickState, player: EntityId, changes: &mut ChangeSet) {
    state.vehicle_menus.viewers.remove(&player);
    let registry = Arc::clone(&state.registry);
    let Some(p) = state.players.get_mut(&player) else {
        return;
    };
    let connection = p.connection;
    let window = p.open.as_ref().map(|m| m.window);
    let mut effects = p.close(&registry.item_rules());
    if let Some(window) = window {
        effects
            .changes
            .insert(0, crate::boundary::PlayerChange::MenuClosed { window });
    }
    crate::tick::apply_player_effects(state, connection, player, effects, changes);
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn click(
    state: &mut TickState,
    player: EntityId,
    window: u8,
    state_id: i32,
    slot: i16,
    button: i8,
    mode: ClickMode,
    changes: &mut ChangeSet,
) {
    let Some(vehicle) = target(state, player) else {
        return;
    };
    if !available(state, player, vehicle) {
        close(state, player, changes);
        return;
    }
    let vehicle_slot = state.entities.slot_of(vehicle).unwrap();
    let mut slots = state
        .entities
        .row(vehicle_slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap()
        .slots
        .clone();
    let registry = Arc::clone(&state.registry);
    let p = state.players.get_mut(&player).unwrap();
    let connection = p.connection;
    let effects = p.click(
        &registry.item_rules(),
        window,
        state_id,
        slot,
        button,
        mode,
        &mut slots,
    );
    let changed = effects.container_changed;
    if changed {
        state
            .entities
            .row_mut(vehicle_slot)
            .entity_life
            .vehicle
            .as_mut()
            .unwrap()
            .slots = slots;
    }
    crate::tick::apply_player_effects(state, connection, player, effects, changes);
    if changed {
        sync(state, vehicle, Some(player));
    }
}
fn sync(state: &mut TickState, vehicle: EntityId, except: Option<EntityId>) {
    let Some(slot) = state.entities.slot_of(vehicle) else {
        return;
    };
    let slots = state
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap()
        .slots
        .clone();
    let viewers: Vec<_> = state
        .vehicle_menus
        .viewers
        .keys()
        .copied()
        .filter(|id| Some(*id) != except && target(state, *id) == Some(vehicle))
        .collect();
    for id in viewers {
        let p = state.players.get_mut(&id).unwrap();
        if let Some(menu) = p.open.as_mut() {
            menu.state_id = (menu.state_id + 1) & 0x7fff;
        }
        let connection = p.connection;
        let changes = p.sync_container_view(&slots);
        for change in changes {
            state.push_player(connection, change);
        }
    }
}
pub(super) fn maintain(state: &mut TickState, changes: &mut ChangeSet) {
    if state.vehicle_menus.viewers.is_empty() {
        return;
    }
    let viewers: Vec<_> = state.vehicle_menus.viewers.keys().copied().collect();
    for player in viewers {
        if let Some(vehicle) = target(state, player) {
            if !available(state, player, vehicle) {
                close(state, player, changes);
            }
        } else {
            state.vehicle_menus.viewers.remove(&player);
        }
    }
}
fn insert(
    rules: &crate::inventory::ItemRules<'_>,
    slots: &mut [ItemStack],
    incoming: &mut ItemStack,
) -> i32 {
    let before = incoming.count;
    for slot in slots {
        if incoming.is_empty() {
            break;
        }
        let max = rules.max_stack(incoming).min(99);
        if slot.is_empty() {
            let moved = incoming.count.min(max);
            *slot = incoming.clone();
            slot.count = moved;
            incoming.count -= moved;
        } else if can_stack(slot, incoming) {
            let moved = incoming.count.min((max - slot.count).max(0));
            slot.count += moved;
            incoming.count -= moved;
        }
    }
    if incoming.count == 0 {
        *incoming = ItemStack::EMPTY;
    }
    before - incoming.count
}
/// Block hoppers can pull chest boats/cart inventories through their native transfer owner.
pub(crate) fn pull_into_hopper(state: &mut TickState, above: BlockPos, hopper: BlockPos) -> bool {
    let Some(slot) = state.entities.slots().find(|slot| {
        let body = state.entities.body(*slot);
        let row = state.entities.row(*slot);
        let p = state.entities.pos(*slot);
        body.alive
            && row
                .entity_life
                .vehicle
                .as_ref()
                .is_some_and(|v| !v.slots.is_empty())
            && (p[0] - f64::from(above.x) - 0.5).abs() <= 0.75
            && (p[2] - f64::from(above.z) - 0.5).abs() <= 0.75
            && p[1] >= f64::from(above.y) - 0.5
            && p[1] <= f64::from(above.y) + 1.0
    }) else {
        return false;
    };
    super::loot::materialize(state, slot);
    let registry = Arc::clone(&state.registry);
    let count = state
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap()
        .slots
        .len();
    for index in 0..count {
        let source = &state
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .slots[index];
        if source.is_empty() {
            continue;
        }
        let mut moving = source.clone();
        moving.count = 1;
        let Some(destination) = state
            .block_entities
            .get_mut(&(hopper.x, hopper.y, hopper.z))
        else {
            return false;
        };
        if insert(&registry.item_rules(), &mut destination.slots, &mut moving) == 1 {
            decrement_stack(
                &mut state
                    .entities
                    .row_mut(slot)
                    .entity_life
                    .vehicle
                    .as_mut()
                    .unwrap()
                    .slots[index],
            );
            sync(state, state.entities.body(slot).id, None);
            return true;
        }
    }
    false
}
pub(super) fn tick_hopper(state: &mut TickState, slot: EntitySlot, _changes: &mut ChangeSet) {
    let position0 = state.entities.pos(slot);
    if !state.world.is_active(
        (position0[0].floor() as i32).div_euclid(16),
        (position0[2].floor() as i32).div_euclid(16),
    ) {
        return;
    }
    if super::name(state, slot) != Some("minecraft:hopper_minecart") {
        return;
    }
    let v = state
        .entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap();
    v.transfer_cooldown = v.transfer_cooldown.saturating_sub(1);
    if !v.enabled || v.transfer_cooldown != 0 {
        return;
    }
    super::loot::materialize(state, slot);
    let position = state.entities.pos(slot);
    let above = BlockPos {
        x: position[0].floor() as i32,
        y: (position[1] + 1.0).floor() as i32,
        z: position[2].floor() as i32,
    };
    let registry = Arc::clone(&state.registry);
    let mut slots = std::mem::take(
        &mut state
            .entities
            .row_mut(slot)
            .entity_life
            .vehicle
            .as_mut()
            .unwrap()
            .slots,
    );
    // Block extraction is shared with block hoppers, including furnace/brewing slot policy.
    let mut moved = crate::redstone::containers::take_for_vehicle(state, above, &mut slots);
    if !moved {
        for index in 0..state.entities.len() {
            let item_slot = EntitySlot(index as u32);
            let body = state.entities.body(item_slot);
            let row = state.entities.row(item_slot);
            let p = state.entities.pos(item_slot);
            if !body.alive
                || !matches!(body.kind, EntityKind::Item { .. })
                || (p[0] - position[0]).abs() > 1.0
                || (p[2] - position[2]).abs() > 1.0
                || p[1] < position[1] - 0.5
                || p[1] > position[1] + 1.5
            {
                continue;
            }
            let Some(item) = row.item.as_ref() else {
                continue;
            };
            let mut incoming = item.full_stack();
            if insert(&registry.item_rules(), &mut slots, &mut incoming) == 0 {
                continue;
            }
            moved = true;
            let (body, row) = state.entities.parts_mut(item_slot);
            if incoming.is_empty() {
                body.alive = false;
                row.item.as_mut().unwrap().count = 0;
                state.merge_consumed_items.insert(item_slot);
            } else {
                crate::entities::set_boxed(
                    &mut row.item,
                    StackRow {
                        item_id: incoming.item_id,
                        count: incoming.count,
                        stack: Some(incoming.clone()),
                    },
                );
                Arc::make_mut(&mut row.appearance).item = Some(incoming);
                state.appearance_changed.push(body.id);
            }
            break;
        }
    }
    let v = state
        .entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap();
    v.slots = slots;
    if moved {
        v.transfer_cooldown = 4;
        sync(state, state.entities.body(slot).id, None);
    }
}

pub(crate) fn insert_one(
    rules: &crate::inventory::ItemRules<'_>,
    slots: &mut [ItemStack],
    stack: &ItemStack,
) -> bool {
    let mut incoming = stack.clone();
    incoming.count = 1;
    insert(rules, slots, &mut incoming) == 1
}
