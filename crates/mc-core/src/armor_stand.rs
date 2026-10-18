//! ArmorStandItem placement and the stand's player equipment/break loop.
use crate::boundary::{BlockPos, ChangeSet, EntityId, Hand};
use crate::entities::{ColdRow, EntityKind, EntitySlot};
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use mc_nbt::Tag;
use std::sync::Arc;

fn stand_slot(state: &TickState, target: EntityId) -> Option<EntitySlot> {
    let slot = state.entities.slot_of(target)?;
    is_stand(state, slot).then_some(slot)
}

pub(crate) fn is_stand(state: &TickState, slot: EntitySlot) -> bool {
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return false;
    };
    state
        .registry
        .blueprint(type_id)
        .is_some_and(|blueprint| blueprint.name == "minecraft:armor_stand")
}

#[derive(Default)]
struct Options {
    small: bool,
    arms: bool,
    marker: bool,
    disabled: u32,
}
fn options(row: &ColdRow) -> Options {
    if row.opaque_entity_fields.is_empty() {
        return Options::default();
    }
    let (_, Tag::Compound(fields)) = mc_nbt::read_named(&mut row.opaque_entity_fields.as_slice())
        .expect("validated retained entity fields")
    else {
        unreachable!("retained entity fields are compound")
    };
    let boolean = |key: &str| {
        fields
            .iter()
            .any(|(name, tag)| name == key && matches!(tag, Tag::Byte(1)))
    };
    let disabled = fields
        .iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("DisabledSlots", Tag::Int(value)) => Some(*value as u32),
            _ => None,
        })
        .unwrap_or(0);
    Options {
        small: boolean("Small"),
        arms: boolean("ShowArms"),
        marker: boolean("Marker"),
        disabled,
    }
}

pub(crate) fn use_on(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    clicked: BlockPos,
    face: u8,
) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let index = crate::tick::held_slot_index(player, hand);
    let stack = &player.inventory.slots[index];
    if stack.is_empty()
        || state
            .registry
            .items
            .name_of(stack.item_id)
            .is_none_or(|id| id.as_str() != "minecraft:armor_stand")
    {
        return false;
    }
    if face == 0 || player.spectator() || player.vitals.is_dead() {
        return true;
    }
    let Some(clicked_state) = state.world.block_at(clicked.x, clicked.y, clicked.z) else {
        return true;
    };
    let target = if crate::tick::replaceable(state, clicked_state) {
        clicked
    } else {
        let offset = crate::placement::Face::from_code(face).offset();
        BlockPos {
            x: clicked.x + offset[0],
            y: clicked.y + offset[1],
            z: clicked.z + offset[2],
        }
    };
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:armor_stand").unwrap())
        .expect("vanilla stand type");
    let blueprint = state.registry.blueprint(type_id).expect("stand blueprint");
    let (width, height, health) = (
        blueprint.half_width,
        blueprint.height,
        blueprint.params.max_health,
    );
    let pos = [
        f64::from(target.x) + 0.5,
        f64::from(target.y),
        f64::from(target.z) + 0.5,
    ];
    if !crate::spawn_egg::clear_body(state, pos, width, height)
        || state.entities.slots().any(|slot| {
            let body = state.entities.body(slot);
            let other = state.entities.pos(slot);
            body.alive
                && (pos[0] - other[0]).abs() < width + body.half_width
                && (pos[2] - other[2]).abs() < width + body.half_width
                && pos[1] < other[1] + body.height
                && pos[1] + height > other[1]
        })
    {
        return true;
    }
    let actor_slot = state.entities.slot_of(actor).expect("placing player");
    // ArmorStandItem.useOn: floor((wrapDegrees(playerYaw - 180) + 22.5) / 45) * 45.
    let yaw = ((state.entities.rot(actor_slot)[0].rem_euclid(360.0) - 180.0 + 22.5) / 45.0).floor()
        * 45.0;
    let name = stack.custom_name.as_deref().map(str::to_owned);
    let mut image = None;
    if let Some(component) = stack
        .extended()
        .preserved_components
        .iter()
        .find(|component| component.name == "minecraft:entity_data")
    {
        let mut bytes = component.value.as_slice();
        let Ok(tag) = mc_nbt::read_network_value(&mut bytes) else {
            return true;
        };
        if !bytes.is_empty() {
            return true;
        }
        let Ok(prepared) =
            crate::command_summon::prepare(state, type_id, "minecraft:armor_stand", pos, &tag)
        else {
            return true;
        };
        image = Some(prepared);
    }
    if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
        return true;
    }
    let id = state.spawn_mob(type_id, pos, health);
    if let Some(image) = image {
        crate::command_summon::apply(state, id, image);
    }
    let slot = state.entities.slot_of(id).expect("placed stand");
    state.entities.set_rot(slot, yaw, 0.0);
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    if name.is_some() {
        Arc::make_mut(&mut row.appearance).custom_name = name;
    }
    state.hostile.spawned.push(id);
    let effects = state
        .players
        .get_mut(&actor)
        .expect("placing player")
        .consume_held(hand, 1);
    if let Some(connection) = state.connections.entity_connection(actor) {
        for change in effects.changes {
            state.push_player(connection, change);
        }
    }
    crate::vibration::emit(
        state,
        pos,
        crate::vibration::GameEvent::EntityPlace,
        Some(actor),
    );
    true
}

fn clicked_slot(equipment: &[Option<ItemStack>; 6], y: f64, small: bool) -> usize {
    let has = |slot: usize| {
        equipment[slot]
            .as_ref()
            .is_some_and(|stack| !stack.is_empty())
    };
    let y = y / if small { 0.5 } else { 1.0 };
    if y >= 0.1 && y < 0.1 + if small { 0.8 } else { 0.45 } && has(2) {
        2
    } else if y >= 0.9 + if small { 0.3 } else { 0.0 }
        && y < 0.9 + if small { 1.0 } else { 0.7 }
        && has(4)
    {
        4
    } else if y >= 0.4 && y < 0.4 + if small { 1.0 } else { 0.8 } && has(3) {
        3
    } else if y >= 1.6 && has(5) {
        5
    } else if !has(0) && has(1) {
        1
    } else {
        0
    }
}

pub(crate) fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
    location: [f64; 3],
) -> bool {
    let Some(slot) = stand_slot(state, target) else {
        return false;
    };
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let opts = options(row);
    let held = &player.inventory.slots[crate::tick::held_slot_index(player, hand)];
    // Vanilla delegates markers/name tags to LivingEntity's item interaction.
    if opts.marker
        || state
            .registry
            .items
            .name_of(held.item_id)
            .is_some_and(|id| id.as_str() == "minecraft:name_tag")
    {
        return false;
    }
    if player.spectator()
        || player.vitals.is_dead()
        || location.iter().any(|coordinate| !coordinate.is_finite())
        || location[1] < -0.1
        || location[1] > body.height + 0.1
        || location[0].abs() > body.half_width + 0.1
        || location[2].abs() > body.half_width + 0.1
    {
        return true;
    }
    let equipment_slot = if held.is_empty() {
        clicked_slot(&row.appearance.equipment, location[1], opts.small)
    } else {
        match state
            .registry
            .item_facts
            .facts_for_stack(held, &state.registry.items)
            .and_then(|facts| facts.equippable_slot.as_deref())
        {
            Some("feet") => 2,
            Some("legs") => 3,
            Some("chest") => 4,
            Some("head") => 5,
            Some("body" | "saddle") => return true,
            _ => 0,
        }
    };
    // EquipmentSlot.getFilterBit uses armor indices 1..4 and hand indices 0/5.
    let bit = match equipment_slot {
        0 => 0,
        1 => 5,
        slot => slot - 1,
    };
    let old = row.appearance.equipment[equipment_slot]
        .as_ref()
        .filter(|stack| !stack.is_empty());
    if (equipment_slot < 2 && !opts.arms)
        || opts.disabled & (1 << bit) != 0
        || opts.disabled & (1 << (bit + if old.is_some() { 8 } else { 16 })) != 0
        || (!held.is_empty() && held.count > 1 && old.is_some())
    {
        return true;
    }
    let creative_copy = player.creative() && old.is_none() && !held.is_empty();
    let mut replacement = held.clone();
    if !replacement.is_empty() {
        replacement.count = 1;
    }
    let new_hand = if creative_copy {
        held.clone()
    } else if held.count > 1 {
        let mut remainder = held.clone();
        remainder.count -= 1;
        remainder
    } else {
        old.cloned().unwrap_or_default()
    };
    if held.is_empty() && old.is_none() {
        return true;
    }
    let replacement_slot = crate::tick::held_slot_index(player, hand);
    if crate::plugin_inventories::below_player_replacement_floors(
        state,
        actor,
        &[(replacement_slot, &new_hand)],
    ) {
        return false;
    }
    if !crate::plugin_inventories::can_synchronize_entity_custody(state, slot) {
        crate::plugin::fail_native(state);
        return false;
    }
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[equipment_slot] =
        (!replacement.is_empty()).then_some(replacement);
    crate::plugin_inventories::synchronize_entity_custody(state, slot)
        .expect("preflighted native stand equipment custody");
    state.appearance_changed.push(target);
    let changes = state
        .players
        .get_mut(&actor)
        .expect("stand player")
        .replace_held(hand, new_hand);
    if let Some(connection) = state.connections.entity_connection(actor) {
        for change in changes {
            state.push_player(connection, change);
        }
    }
    true
}

pub(crate) fn attack(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    changes: &mut ChangeSet,
) -> bool {
    let Some(slot) = stand_slot(state, target) else {
        return false;
    };
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    if player.spectator()
        || player.vitals.is_dead()
        || player.game_mode == crate::commands::GameMode::Adventure
        || options(state.entities.row(slot)).marker
    {
        return true;
    }
    let creative = player.creative();
    if state.entities.row(slot).controls.invulnerable && !creative {
        return true;
    }
    if !creative
        && state
            .armor_stand_hits
            .get(&target)
            .is_none_or(|at| state.tick.saturating_sub(*at) > 5)
    {
        state.armor_stand_hits.insert(target, state.tick);
        changes
            .entity_events
            .push(crate::boundary::EntityEvent::Status {
                entity: target,
                event: 32,
            });
        return true;
    }
    if !creative {
        let mut spawned = std::mem::take(&mut state.hostile.spawned);
        drop_items(state, slot, true, &mut spawned);
        state.hostile.spawned = spawned;
    }
    state.remove_entity(slot);
    changes.entities_removed.push(target);
    true
}

/// ArmorStand's brokenByAnything releases all equipment, unlike mob loot chances.
fn drop_items(
    state: &mut TickState,
    slot: EntitySlot,
    stand_item: bool,
    spawned: &mut Vec<EntityId>,
) {
    if !crate::command_rules::boolean(state, "entity_drops") {
        return;
    }
    let pos = state.entities.pos(slot);
    let row = state.entities.row_mut(slot);
    let appearance = Arc::make_mut(&mut row.appearance);
    let equipment = std::mem::take(&mut appearance.equipment);
    let extra = [
        row.loot.body_armor.take().map(|stack| *stack),
        row.loot.saddle.take().map(|stack| *stack),
    ];
    appearance.body_armor = None;
    appearance.saddle = None;
    let name = if stand_item {
        appearance.custom_name.as_deref().map(Box::from)
    } else {
        None
    };
    if stand_item {
        let stand = state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:armor_stand").unwrap())
            .expect("stand item");
        let mut stack = ItemStack::new(stand, 1);
        stack.custom_name = name;
        spawned.push(crate::tick::spawn_item_entity(state, pos, stack));
    }
    for stack in equipment
        .into_iter()
        .chain(extra)
        .flatten()
        .filter(|stack| !stack.is_empty())
    {
        if stack
            .enchantments
            .iter()
            .any(|entry| entry.level > 0 && entry.id.as_str() == "minecraft:vanishing_curse")
        {
            continue;
        }
        spawned.push(crate::tick::spawn_item_entity(state, pos, stack));
    }
}

pub(crate) fn death(
    state: &mut TickState,
    slot: EntitySlot,
    cause: &mc_data::loot::entity_26_1_2::EntityDeathCause,
    spawned: &mut Vec<EntityId>,
) -> bool {
    if !is_stand(state, slot) {
        return false;
    }
    let (body, row) = state.entities.parts_mut(slot);
    row.loot.death_handled = true;
    state.armor_stand_hits.remove(&body.id);
    let tagged = |name| cause.tags().iter().any(|tag| tag.as_str() == name);
    // ArmorStand.kill (creative/bypassing damage) has no item/equipment drops.
    if !tagged("minecraft:bypasses_invulnerability") {
        let player = cause
            .direct_attacker()
            .is_some_and(|entity| entity.entity_type.as_str() == "minecraft:player");
        let stand_item =
            !tagged("minecraft:is_explosion") && (tagged("minecraft:is_projectile") || player);
        drop_items(state, slot, stand_item, spawned);
    }
    true
}

const POSE_PARTS: [&str; 6] = ["Head", "Body", "LeftArm", "RightArm", "LeftLeg", "RightLeg"];

pub(crate) fn validate_fields(fields: &[(String, Tag)]) -> Result<(), crate::CoreError> {
    let invalid = |key: &str| crate::CoreError::Storage(format!("armor stand {key}: invalid NBT"));
    for (key, tag) in fields {
        match key.as_str() {
            "Small" | "ShowArms" | "NoBasePlate" | "Marker" | "Invisible" => {
                if !matches!(tag, Tag::Byte(0 | 1)) {
                    return Err(invalid(key));
                }
            }
            "DisabledSlots" => {
                if !matches!(tag, Tag::Int(_)) {
                    return Err(invalid(key));
                }
            }
            "Pose" => {
                let Tag::Compound(parts) = tag else {
                    return Err(invalid(key));
                };
                for (name, value) in parts {
                    if !POSE_PARTS.contains(&name.as_str()) {
                        continue;
                    }
                    let Tag::List(list) = value else {
                        return Err(invalid(name));
                    };
                    if list.element_type != mc_nbt::tag_type::FLOAT
                        || list.elements.len() != 3
                        || list
                            .elements
                            .iter()
                            .any(|value| !matches!(value, Tag::Float(v) if v.is_finite()))
                    {
                        return Err(invalid(name));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The 26.1.2 javap defineId sequence is Entity 0..7, LivingEntity 8..14,
/// ArmorStand flags 15 and the six ROTATIONS accessors 16..21.
pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    let id = state.entities.body(slot).id;
    if stand_slot(state, id).is_none() {
        return;
    }
    let opts = options(state.entities.row(slot));
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return;
    };
    let blueprint = state.registry.blueprint(type_id).expect("stand blueprint");
    let scale = if opts.marker {
        0.0
    } else if opts.small {
        0.5
    } else {
        1.0
    };
    let (width, height) = (blueprint.half_width * scale, blueprint.height * scale);
    state.entities.set_half_width(slot, width);
    let (body, row) = state.entities.parts_mut(slot);
    body.height = height;
    if row.opaque_entity_fields.is_empty() {
        return;
    }
    let (_, Tag::Compound(fields)) = mc_nbt::read_named(&mut row.opaque_entity_fields.as_slice())
        .expect("validated retained entity fields")
    else {
        unreachable!()
    };
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance
        .extra
        .retain(|(index, _)| !(15..=21).contains(index));
    let no_base = fields
        .iter()
        .any(|(key, value)| key == "NoBasePlate" && matches!(value, Tag::Byte(1)));
    let flags = u8::from(opts.small)
        | (u8::from(opts.arms) << 2)
        | (u8::from(no_base) << 3)
        | (u8::from(opts.marker) << 4);
    appearance
        .extra
        .push((15, crate::boundary::EntityDataValue::Byte(flags as i8)));
    if fields
        .iter()
        .any(|(key, value)| key == "Invisible" && matches!(value, Tag::Byte(1)))
    {
        appearance.shared_flags |= 0x20;
    }
    if let Some((_, Tag::Compound(parts))) = fields.iter().find(|(key, _)| key == "Pose") {
        for (index, name) in POSE_PARTS.into_iter().enumerate() {
            if let Some((_, Tag::List(list))) = parts.iter().find(|(key, _)| key == name)
                && let [Tag::Float(x), Tag::Float(y), Tag::Float(z)] = list.elements.as_slice()
            {
                appearance.extra.push((
                    16 + index as u8,
                    crate::boundary::EntityDataValue::Rotations([*x, *y, *z]),
                ));
            }
        }
    }
}

#[cfg(test)]
#[path = "armor_stand_tests.rs"]
mod tests;
