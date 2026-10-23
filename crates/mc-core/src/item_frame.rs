//! Hanging displays: native entity custody, with no idle per-tick work.
use crate::boundary::{BlockPos, ChangeSet, EntityDataValue, EntityId, Hand};
use crate::entities::{EntityKind, EntitySlot};
use crate::placement::Face;
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use std::sync::Arc;

// Local 26.1.2 javap: Entity declares 0..7, HangingEntity direction 8,
// ItemFrame ITEM_STACK 9 and INT rotation 10 (also inherited by glow frames).
const ITEM: u8 = 9;
const ROTATION: u8 = 10;

fn name(state: &TickState, slot: EntitySlot) -> Option<&'static str> {
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return None;
    };
    match mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)?.name {
        "minecraft:item_frame" => Some("minecraft:item_frame"),
        "minecraft:glow_item_frame" => Some("minecraft:glow_item_frame"),
        _ => None,
    }
}
fn cell(state: &TickState, slot: EntitySlot) -> BlockPos {
    let pos = state.entities.pos(slot);
    BlockPos {
        x: pos[0].floor() as i32,
        y: pos[1].floor() as i32,
        z: pos[2].floor() as i32,
    }
}
fn displayed(state: &TickState, slot: EntitySlot) -> Option<&ItemStack> {
    state
        .entities
        .row(slot)
        .appearance
        .extra
        .iter()
        .find_map(|(index, value)| match (index, value) {
            (&ITEM, EntityDataValue::Item(item)) => item.as_ref(),
            _ => None,
        })
}
fn rotation(state: &TickState, slot: EntitySlot) -> i32 {
    state
        .entities
        .row(slot)
        .appearance
        .extra
        .iter()
        .find_map(|(index, value)| match (index, value) {
            (&ROTATION, EntityDataValue::VarInt(value)) => Some(*value),
            _ => None,
        })
        .unwrap_or(0)
}
fn support(state: &TickState, position: BlockPos, face: u8) -> bool {
    if face > 5 {
        return false;
    }
    let offset = Face::from_code(face).offset();
    let Some(id) = state.world.block_at(
        position.x - offset[0],
        position.y - offset[1],
        position.z - offset[2],
    ) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
        return false;
    };
    use mc_data::block_facts::{SturdyFace, has_full_sturdy_face};
    let sturdy = [
        SturdyFace::Down,
        SturdyFace::Up,
        SturdyFace::North,
        SturdyFace::South,
        SturdyFace::West,
        SturdyFace::East,
    ][usize::from(face)];
    has_full_sturdy_face(id, &block.block.id, &block.properties, sturdy)
}
fn frame_at(state: &TickState, position: BlockPos, face: u8) -> Option<EntitySlot> {
    let mut found = None;
    state.entities.for_each_slot_in_chunk(
        crate::boundary::ChunkPos {
            x: position.x.div_euclid(16),
            z: position.z.div_euclid(16),
        },
        |slot| {
            if state.entities.body(slot).alive
                && name(state, slot).is_some()
                && cell(state, slot) == position
                && state.entities.row(slot).appearance.spawn_data == i32::from(face)
            {
                found = Some(slot);
            }
        },
    );
    found
}

/// Shared placement/generation API. `position` is the air cell, `face` points
/// away from its backing block. Custody is the entity's journaled metadata.
pub(crate) fn spawn(
    state: &mut TickState,
    position: BlockPos,
    face: u8,
    glow: bool,
    mut item: ItemStack,
) -> Option<EntityId> {
    if !support(state, position, face)
        || frame_at(state, position, face).is_some()
        || !state
            .world
            .block_at(position.x, position.y, position.z)
            .is_some_and(|id| crate::tick::replaceable(state, id))
    {
        return None;
    }
    let kind = if glow {
        "minecraft:glow_item_frame"
    } else {
        "minecraft:item_frame"
    };
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(kind).unwrap())?;
    let offset = Face::from_code(face).offset();
    let pos = [
        f64::from(position.x) + 0.5 - f64::from(offset[0]) * 0.46875,
        f64::from(position.y) + 0.5 - f64::from(offset[1]) * 0.46875,
        f64::from(position.z) + 0.5 - f64::from(offset[2]) * 0.46875,
    ];
    let id = state.spawn_mob(type_id, pos, 1.0);
    let slot = state.entities.slot_of(id).expect("new frame");
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.spawn_data = i32::from(face);
    let occupied = !item.is_empty();
    item.count = 1;
    appearance.extra = vec![
        (ITEM, EntityDataValue::Item(occupied.then_some(item))),
        (ROTATION, EntityDataValue::VarInt(0)),
    ];
    restore(state, slot);
    state.hostile.spawned.push(id);
    crate::redstone::wake(state, position);
    Some(id)
}

pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    if name(state, slot).is_none() {
        return;
    }
    let face = state.entities.row(slot).appearance.spawn_data;
    let (yaw, pitch) = match face {
        0 => (0.0, 90.0),
        1 => (0.0, -90.0),
        2 => (180.0, 0.0),
        3 => (0.0, 0.0),
        4 => (90.0, 0.0),
        _ => (270.0, 0.0),
    };
    state.entities.set_rot(slot, yaw, pitch);
    state.entities.set_half_width(slot, 0.375);
    let (body, row) = state.entities.parts_mut(slot);
    body.movement = crate::blueprint::MovementClass::Stationary;
    body.height = if face < 2 { 0.0625 } else { 0.75 };
    row.controls.no_ai = true;
    row.item = None;
}

pub(crate) fn capture(
    record: &mut mc_data::entity_nbt::VanillaEntity,
    appearance: &crate::boundary::EntityAppearance,
) {
    if !matches!(
        record.id.as_str(),
        "minecraft:item_frame" | "minecraft:glow_item_frame"
    ) {
        return;
    }
    use mc_nbt::Tag;
    record
        .extra_fields
        .retain(|(name, _)| !matches!(name.as_str(), "Facing" | "ItemRotation"));
    record
        .extra_fields
        .push(("Facing".into(), Tag::Byte(appearance.spawn_data as i8)));
    let mut rotation = 0;
    for (index, value) in &appearance.extra {
        match (*index, value) {
            (ITEM, EntityDataValue::Item(item)) => record.item.clone_from(item),
            (ROTATION, EntityDataValue::VarInt(value)) => rotation = *value,
            _ => {}
        }
    }
    record
        .extra_fields
        .push(("ItemRotation".into(), Tag::Byte(rotation as i8)));
}

pub(crate) fn native_appearance(
    record: &mc_data::entity_nbt::VanillaEntity,
    image: &mut crate::journal::EntityAppearanceImage,
) -> Result<(), crate::CoreError> {
    if !matches!(
        record.id.as_str(),
        "minecraft:item_frame" | "minecraft:glow_item_frame"
    ) {
        return Ok(());
    }
    use mc_nbt::Tag;
    let invalid =
        || crate::CoreError::Storage("invalid item frame Facing/ItemRotation/Item".into());
    let value = |name: &str, max: i8| {
        let mut fields = record.extra_fields.iter().filter(|(key, _)| key == name);
        let value = match fields.next() {
            None => 0,
            Some((_, Tag::Byte(value))) if (0..=max).contains(value) => i32::from(*value),
            _ => return Err(invalid()),
        };
        if fields.next().is_some() {
            return Err(invalid());
        }
        Ok(value)
    };
    image.spawn_data = value("Facing", 5)?;
    if record.item.as_ref().is_some_and(|item| item.count != 1) {
        return Err(invalid());
    }
    image.extra.push((
        ITEM,
        crate::journal::EntityDataValueImage::Item(record.item.clone()),
    ));
    image.extra.push((
        ROTATION,
        crate::journal::EntityDataValueImage::VarInt(value("ItemRotation", 7)?),
    ));
    Ok(())
}

pub(crate) fn validate_image(
    record: &crate::journal::ReplayEntity,
) -> Result<(), crate::CoreError> {
    use crate::journal::EntityDataValueImage;
    let invalid = || crate::CoreError::Storage("invalid item frame metadata image".into());
    if !(0..=5).contains(&record.appearance.spawn_data) {
        return Err(invalid());
    }
    let mut item = false;
    let mut rotation = false;
    for (index, value) in &record.appearance.extra {
        match (*index, value) {
            (ITEM, EntityDataValueImage::Item(stack))
                if !item
                    && stack
                        .as_ref()
                        .is_none_or(|stack| !stack.is_empty() && stack.count == 1) =>
            {
                item = true
            }
            (ROTATION, EntityDataValueImage::VarInt(value))
                if !rotation && (0..=7).contains(value) =>
            {
                rotation = true
            }
            (ITEM | ROTATION, _) => return Err(invalid()),
            _ => {}
        }
    }
    if !item || !rotation {
        return Err(invalid());
    }
    Ok(())
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
    let held = &player.inventory.slots[crate::tick::held_slot_index(player, hand)];
    let glow = match state
        .registry
        .items
        .name_of(held.item_id)
        .map(|id| id.as_str())
    {
        Some("minecraft:item_frame") if !held.is_empty() => false,
        Some("minecraft:glow_item_frame") if !held.is_empty() => true,
        _ => return false,
    };
    if player.spectator()
        || player.vitals.is_dead()
        || face > 5
        || crate::tick::held_debit_below_floor(state, actor, hand, 1)
    {
        return true;
    }
    let offset = Face::from_code(face).offset();
    let position = BlockPos {
        x: clicked.x + offset[0],
        y: clicked.y + offset[1],
        z: clicked.z + offset[2],
    };
    if spawn(state, position, face, glow, ItemStack::EMPTY).is_none() {
        return true;
    }
    let effects = state
        .players
        .get_mut(&actor)
        .expect("frame placer")
        .consume_held(hand, 1);
    if let Some(connection) = state.connections.entity_connection(actor) {
        for change in effects.changes {
            state.push_player(connection, change);
        }
    }
    true
}

pub(crate) fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
) -> bool {
    let Some(slot) = state
        .entities
        .slot_of(target)
        .filter(|slot| name(state, *slot).is_some())
    else {
        return false;
    };
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    if player.spectator() || player.vitals.is_dead() || !state.entities.body(slot).alive {
        return true;
    }
    if displayed(state, slot).is_some() {
        let next = (rotation(state, slot) + 1) % 8;
        let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
        appearance.extra.retain(|(index, _)| *index != ROTATION);
        appearance
            .extra
            .push((ROTATION, EntityDataValue::VarInt(next)));
    } else {
        let mut held = player.inventory.slots[crate::tick::held_slot_index(player, hand)].clone();
        if held.is_empty() || crate::tick::held_debit_below_floor(state, actor, hand, 1) {
            return true;
        }
        held.count = 1;
        let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
        appearance.extra.retain(|(index, _)| *index != ITEM);
        appearance
            .extra
            .push((ITEM, EntityDataValue::Item(Some(held))));
        let effects = state
            .players
            .get_mut(&actor)
            .expect("frame user")
            .consume_held(hand, 1);
        if let Some(connection) = state.connections.entity_connection(actor) {
            for change in effects.changes {
                state.push_player(connection, change);
            }
        }
    }
    state.appearance_changed.push(target);
    crate::redstone::wake(state, cell(state, slot));
    true
}

fn drop_display(
    state: &mut TickState,
    slot: EntitySlot,
    drop: bool,
    spawned: &mut Vec<EntityId>,
) -> bool {
    let Some(item) = displayed(state, slot).cloned() else {
        return false;
    };
    let pos = state.entities.pos(slot);
    let appearance = Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
    appearance
        .extra
        .retain(|(index, _)| *index != ITEM && *index != ROTATION);
    appearance.extra.push((ITEM, EntityDataValue::Item(None)));
    appearance
        .extra
        .push((ROTATION, EntityDataValue::VarInt(0)));
    if drop && crate::command_rules::boolean(state, "entity_drops") {
        spawned.push(crate::tick::spawn_item_entity(state, pos, item));
    }
    state.appearance_changed.push(state.entities.body(slot).id);
    crate::redstone::wake(state, cell(state, slot));
    true
}
fn break_frame(state: &mut TickState, slot: EntitySlot, drop: bool, spawned: &mut Vec<EntityId>) {
    let kind = name(state, slot).expect("frame");
    drop_display(state, slot, drop, spawned);
    if drop && crate::command_rules::boolean(state, "entity_drops") {
        let item = state
            .registry
            .items
            .id_of(&Identifier::parse(kind).unwrap())
            .expect("frame item");
        spawned.push(crate::tick::spawn_item_entity(
            state,
            state.entities.pos(slot),
            ItemStack::new(item, 1),
        ));
    }
    let position = cell(state, slot);
    state.hostile.removed.push(state.entities.body(slot).id);
    state.remove_entity(slot);
    crate::redstone::wake(state, position);
}
pub(crate) fn attack(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    changes: &mut ChangeSet,
) -> bool {
    let Some(slot) = state
        .entities
        .slot_of(target)
        .filter(|slot| name(state, *slot).is_some())
    else {
        return false;
    };
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    if player.spectator()
        || player.vitals.is_dead()
        || player.game_mode == crate::commands::GameMode::Adventure
    {
        return true;
    }
    let drop = !player.creative();
    if drop_display(state, slot, drop, &mut changes.entities_spawned) {
        return true;
    }
    break_frame(state, slot, drop, &mut changes.entities_spawned);
    true
}

pub(crate) fn death(state: &mut TickState, slot: EntitySlot, spawned: &mut Vec<EntityId>) -> bool {
    let Some(kind) = name(state, slot) else {
        return false;
    };
    state.entities.row_mut(slot).loot.death_handled = true;
    drop_display(state, slot, true, spawned);
    if crate::command_rules::boolean(state, "entity_drops") {
        let item = state
            .registry
            .items
            .id_of(&Identifier::parse(kind).unwrap())
            .expect("frame item");
        spawned.push(crate::tick::spawn_item_entity(
            state,
            state.entities.pos(slot),
            ItemStack::new(item, 1),
        ));
    }
    true
}

pub(crate) fn analog_output(state: &TickState, position: BlockPos, face: u8) -> Option<u8> {
    let slot = frame_at(state, position, face)?;
    Some(if displayed(state, slot).is_some() {
        (rotation(state, slot) % 8 + 1) as u8
    } else {
        0
    })
}

/// Event-driven support invalidation, querying only chunks touched by edits.
pub(crate) fn blocks_changed(
    state: &mut TickState,
    writes: &[(BlockPos, u32)],
    spawned: &mut Vec<EntityId>,
) {
    static TYPES: std::sync::LazyLock<[u32; 2]> = std::sync::LazyLock::new(|| {
        use mc_data::entity_types::entity_type_contract_26_1_2_by_name;
        ["minecraft:item_frame", "minecraft:glow_item_frame"].map(|name| {
            entity_type_contract_26_1_2_by_name(name)
                .expect("frame type")
                .protocol_id
        })
    });
    if writes.is_empty()
        || TYPES
            .iter()
            .all(|id| state.entities.mob_slots(*id).next().is_none())
    {
        return;
    }
    let mut affected = rustc_hash::FxHashSet::default();
    let mut chunks = rustc_hash::FxHashSet::default();
    for (changed, _) in writes {
        for offset in [
            [0, 0, 0],
            [0, 1, 0],
            [0, -1, 0],
            [1, 0, 0],
            [-1, 0, 0],
            [0, 0, 1],
            [0, 0, -1],
        ] {
            let position = BlockPos {
                x: changed.x + offset[0],
                y: changed.y + offset[1],
                z: changed.z + offset[2],
            };
            affected.insert(position);
            chunks.insert(crate::boundary::ChunkPos {
                x: position.x.div_euclid(16),
                z: position.z.div_euclid(16),
            });
        }
    }
    let mut invalid = Vec::new();
    for chunk in chunks {
        state.entities.for_each_slot_in_chunk(chunk, |slot| {
            let position = cell(state, slot);
            if name(state, slot).is_some()
                && affected.contains(&position)
                && (!support(
                    state,
                    position,
                    state.entities.row(slot).appearance.spawn_data as u8,
                ) || !state
                    .world
                    .block_at(position.x, position.y, position.z)
                    .is_some_and(|id| crate::tick::replaceable(state, id)))
            {
                invalid.push(state.entities.body(slot).id);
            }
        });
    }
    invalid.sort_unstable();
    for id in invalid {
        if let Some(slot) = state.entities.slot_of(id) {
            break_frame(state, slot, true, spawned);
        }
    }
}

#[cfg(test)]
#[path = "item_frame_tests.rs"]
mod tests;
