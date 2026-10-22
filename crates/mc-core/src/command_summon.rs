//! Summon preflight uses the same native entity codecs as checkpoint recovery.
//! No entity is allocated until every authored field has been validated.
use crate::boundary::{EntityDataValue, EntityId};
use crate::commands::CommandError;
use crate::journal::EntityStateImage;
use crate::tick::TickState;
use mc_data::entity_nbt::VanillaEntity;
use mc_nbt::{ListTag, Tag, tag_type};
use std::sync::Arc;

pub(crate) struct Image {
    native: VanillaEntity,
    state: EntityStateImage,
    villager: Option<crate::villager::VillagerState>,
    equipment: Option<[Option<mc_data::ItemStack>; 6]>,
}
fn error(text: impl Into<String>) -> CommandError {
    CommandError::Message(text.into())
}
fn number(tag: &Tag, key: &str) -> Result<f64, CommandError> {
    let value = match tag {
        Tag::Byte(v) => f64::from(*v),
        Tag::Short(v) => f64::from(*v),
        Tag::Int(v) => f64::from(*v),
        Tag::Long(v) => *v as f64,
        Tag::Float(v) => f64::from(*v),
        Tag::Double(v) => *v,
        _ => return Err(error(format!("Invalid {key}: expected number"))),
    };
    if !value.is_finite() {
        return Err(error(format!("Invalid {key}: non-finite number")));
    }
    Ok(value)
}
fn uuid(tag: &Tag) -> Result<uuid::Uuid, CommandError> {
    match tag {
        Tag::String(value) => uuid::Uuid::parse_str(value).map_err(|_| error("Invalid UUID")),
        Tag::IntArray(parts) if parts.len() == 4 => Ok(uuid::Uuid::from_u128(
            (parts[0] as u32 as u128) << 96
                | (parts[1] as u32 as u128) << 64
                | (parts[2] as u32 as u128) << 32
                | parts[3] as u32 as u128,
        )),
        _ => Err(error("Invalid UUID")),
    }
}
/// Item stack authored as `{id, count, components}`, admitted like `/give`.
fn parse_stack(
    items: &mc_data::items::ItemRegistry,
    fields: &[(String, Tag)],
) -> Result<mc_data::ItemStack, String> {
    for (index, (key, _)) in fields.iter().enumerate() {
        if !matches!(key.as_str(), "id" | "count" | "components")
            || fields[..index].iter().any(|(seen, _)| seen == key)
        {
            return Err(format!("invalid or duplicate item field {key}"));
        }
    }
    let Some(Tag::String(id)) = mc_data::item_nbt::field(fields, "id") else {
        return Err("expected item id string".into());
    };
    let id = mc_data::Identifier::parse(id.clone()).map_err(|e| e.to_string())?;
    let count = match mc_data::item_nbt::field(fields, "count") {
        None => 1,
        Some(Tag::Byte(v)) => i32::from(*v),
        Some(Tag::Short(v)) => i32::from(*v),
        Some(Tag::Int(v)) => *v,
        _ => return Err("expected integer item count".into()),
    };
    if count <= 0 {
        return Err("item count must be positive".into());
    }
    let components = match mc_data::item_nbt::field(fields, "components") {
        None => &[][..],
        Some(Tag::Compound(components)) => components.as_slice(),
        _ => return Err("expected components compound".into()),
    };
    for (index, (key, _)) in components.iter().enumerate() {
        if components[..index].iter().any(|(seen, _)| seen == key) {
            return Err(format!("duplicate item component {key}"));
        }
    }
    // The same component admission as /give.
    let mut stack = mc_data::item_nbt::stack_from_component_patch(items, &id, components)
        .map_err(|e| e.to_string())?;
    stack.count = count;
    Ok(stack)
}
fn prepare_stack(
    items: &mc_data::items::ItemRegistry,
    fields: &[(String, Tag)],
) -> Result<Vec<(String, Tag)>, String> {
    if fields.is_empty() {
        return Ok(Vec::new());
    }
    let stack = parse_stack(items, fields)?;
    mc_data::item_nbt::stack_to_fields(items, &stack).map_err(|e| e.to_string())
}

/// `summon minecraft:item` payload: the stack plus the authored item-entity fields.
pub(crate) struct ItemSummon {
    pub(crate) stack: mc_data::ItemStack,
    pub(crate) pickup_delay: Option<u32>,
    pub(crate) age: Option<u32>,
    pub(crate) motion: Option<[f64; 3]>,
}
/// Validate item-entity summon NBT (`Item`, `PickupDelay`, `Age`, `Motion`) before any spawn.
pub(crate) fn prepare_item(state: &TickState, nbt: &Tag) -> Result<ItemSummon, CommandError> {
    let Tag::Compound(authored) = nbt else {
        return Err(error("Summon NBT must be a compound"));
    };
    let mut stack = None;
    let (mut pickup_delay, mut age, mut motion) = (None, None, None);
    for (index, (key, value)) in authored.iter().enumerate() {
        if authored[..index].iter().any(|(seen, _)| seen == key) {
            return Err(error(format!("Duplicate summon NBT field {key}")));
        }
        match key.as_str() {
            "id" | "Pos" => {}
            "Item" => {
                let Tag::Compound(fields) = value else {
                    return Err(error("Invalid Item: expected compound"));
                };
                stack = Some(
                    parse_stack(&state.registry.items, fields)
                        .map_err(|e| error(format!("Invalid Item: {e}")))?,
                );
            }
            "PickupDelay" => {
                // Vanilla stores a short; 32767 means "never".
                pickup_delay = Some(number(value, key)?.clamp(0.0, 32767.0) as u32);
            }
            "Age" => {
                age = Some(number(value, key)?.clamp(0.0, 6000.0) as u32);
            }
            "Motion" => {
                let Tag::List(list) = value else {
                    return Err(error("Invalid Motion"));
                };
                if list.elements.len() != 3 {
                    return Err(error("Motion must contain 3 values"));
                }
                let mut values = [0.0; 3];
                for (slot, element) in values.iter_mut().zip(&list.elements) {
                    *slot = number(element, key)?;
                }
                motion = Some(values);
            }
            _ => {
                return Err(error(format!(
                    "Unknown or unsupported summon NBT field {key}"
                )));
            }
        }
    }
    let stack = stack.ok_or_else(|| error("Item entity summon requires an Item"))?;
    Ok(ItemSummon {
        stack,
        pickup_delay,
        age,
        motion,
    })
}
pub(crate) fn prepare(
    state: &TickState,
    type_id: u32,
    name: &str,
    pos: [f64; 3],
    nbt: &Tag,
) -> Result<Image, CommandError> {
    let Tag::Compound(authored) = nbt else {
        return Err(error("Summon NBT must be a compound"));
    };
    let living = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
        .is_some_and(|c| c.behavior.archetype == mc_data::entity_types::EntityArchetype::Living);
    let horse = crate::taming::horse(name);
    let ageable = state
        .registry
        .blueprint(type_id)
        .is_some_and(|b| b.offspring.is_some());
    let ageable = ageable || crate::entity_life::old_ageable(name);
    let mut fields = Vec::with_capacity(authored.len());
    for (key, value) in authored {
        if fields.iter().any(|(seen, _)| seen == key) {
            return Err(error(format!("Duplicate summon NBT field {key}")));
        }
        let supported = match key.as_str() {
            "Health"
            | "NoAI"
            | "PersistenceRequired"
            | "CanPickUpLoot"
            | "leash"
            | "drop_chances" => living,
            "Owner" | "Tame" => {
                horse
                    || matches!(
                        name,
                        "minecraft:wolf" | "minecraft:cat" | "minecraft:parrot"
                    )
            }
            "Sitting" => matches!(
                name,
                "minecraft:wolf" | "minecraft:cat" | "minecraft:parrot"
            ),
            "Temper" => horse,
            "CollarColor" => matches!(name, "minecraft:wolf" | "minecraft:cat"),
            "equipment" => living || name == "minecraft:armor_stand",
            "Age" => ageable || matches!(name, "minecraft:villager" | "minecraft:tadpole"),
            "IsBaby" => name == "minecraft:zombie_villager",
            "VillagerData" => matches!(name, "minecraft:villager" | "minecraft:zombie_villager"),
            "InLove" | "LoveCause" => ageable,
            "Color" | "Sheared" => name == "minecraft:sheep",
            "Type" | "stew_effects" => name == "minecraft:mooshroom",
            "Trusting" => name == "minecraft:ocelot",
            "Trusted" => name == "minecraft:fox",
            "Small" | "ShowArms" | "NoBasePlate" | "Marker" | "Invisible" | "DisabledSlots"
            | "Pose" => name == "minecraft:armor_stand",
            "Size" => matches!(name, "minecraft:slime" | "minecraft:magma_cube"),
            "CustomName" | "CustomNameVisible" | "Invulnerable" | "Motion" | "Rotation"
            | "OnGround" | "Fire" | "FallDistance" | "UUID" | "id" | "Pos" => true,
            _ => {
                return Err(error(format!(
                    "Unknown or unsupported summon NBT field {key}"
                )));
            }
        };
        if !supported {
            return Err(error(format!(
                "Entity does not support summon NBT field {key}"
            )));
        }
        let value = match key.as_str() {
            "id" | "Pos" => continue,
            "NoAI"
            | "PersistenceRequired"
            | "CanPickUpLoot"
            | "IsBaby"
            | "Invulnerable"
            | "CustomNameVisible"
            | "OnGround"
            | "Tame"
            | "Sitting"
            | "Trusting"
            | "Small"
            | "ShowArms"
            | "NoBasePlate"
            | "Marker"
            | "Invisible"
            | "Sheared" => Tag::Byte(i8::from(number(value, key)? != 0.0)),
            "Health" => {
                let v = number(value, key)?;
                if v <= 0.0 || v > f64::from(f32::MAX) {
                    return Err(error("Invalid Health"));
                }
                Tag::Float(v as f32)
            }
            "FallDistance" => {
                let v = number(value, key)?;
                if v.abs() > f64::from(f32::MAX) {
                    return Err(error("Invalid FallDistance"));
                }
                Tag::Float(v as f32)
            }
            "Age" => Tag::Int(number(value, key)?.clamp(i32::MIN as f64, i32::MAX as f64) as i32),
            // Vanilla Slime.setSize clamps the stored zero-based Size to 0..=126.
            "Size" => Tag::Int(number(value, key)?.clamp(0.0, 126.0) as i32),
            "DisabledSlots" => {
                Tag::Int(number(value, key)?.clamp(i32::MIN as f64, i32::MAX as f64) as i32)
            }
            "InLove" => Tag::Int(number(value, key)?.clamp(0.0, u16::MAX as f64) as i32),
            "Fire" => {
                Tag::Short(number(value, key)?.clamp(i16::MIN as f64, i16::MAX as f64) as i16)
            }
            "Temper" => Tag::Int(number(value, key)?.clamp(0.0, 100.0) as i32),
            "CollarColor" => Tag::Byte(number(value, key)?.clamp(0.0, 15.0) as i8),
            "Color" => {
                let v = number(value, key)?;
                if !(0.0..=15.0).contains(&v) || v.fract() != 0.0 {
                    return Err(error("Invalid sheep color"));
                }
                Tag::Byte(v as i8)
            }
            "Motion" | "Rotation" => {
                let Tag::List(list) = value else {
                    return Err(error(format!("Invalid {key}")));
                };
                let count = if key == "Motion" { 3 } else { 2 };
                if list.elements.len() != count {
                    return Err(error(format!("{key} must contain {count} values")));
                }
                let numbers = list
                    .elements
                    .iter()
                    .map(|v| number(v, key))
                    .collect::<Result<Vec<_>, _>>()?;
                if key == "Motion" {
                    if numbers.iter().any(|v| !(v * 20.0).is_finite()) {
                        return Err(error("Invalid Motion"));
                    }
                    Tag::List(ListTag {
                        element_type: tag_type::DOUBLE,
                        elements: numbers.into_iter().map(Tag::Double).collect(),
                    })
                } else {
                    if numbers.iter().any(|v| v.abs() > f64::from(f32::MAX)) {
                        return Err(error("Invalid Rotation"));
                    }
                    Tag::List(ListTag {
                        element_type: tag_type::FLOAT,
                        elements: numbers.into_iter().map(|v| Tag::Float(v as f32)).collect(),
                    })
                }
            }
            "UUID" => {
                let value = uuid(value)?;
                if state.dimension_globals.reserved_uuids.contains(&value)
                    || std::iter::once(state)
                        .chain(state.other_dimensions.values().map(|world| &**world))
                        .any(|world| {
                            world
                                .entities
                                .slots()
                                .any(|slot| world.entities.row(slot).uuid == value)
                                || world.offline_players.contains_key(&value)
                                || world.connections.all_connections().any(|connection| {
                                    world.connections.uuid_of(connection) == Some(value)
                                })
                        })
                {
                    return Err(error("Summon UUID already exists"));
                }
                Tag::IntArray(
                    value
                        .as_u128()
                        .to_be_bytes()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| i32::from_be_bytes(*p))
                        .collect(),
                )
            }
            // Validated (profession/type/level) by `villager_from_fields` below.
            "VillagerData" => {
                let Tag::Compound(_) = value else {
                    return Err(error("Invalid VillagerData: expected compound"));
                };
                value.clone()
            }
            "equipment" => {
                let Tag::Compound(slots) = value else {
                    return Err(error("Invalid equipment: expected compound"));
                };
                let mut normalized = Vec::with_capacity(slots.len());
                for (slot, stack) in slots {
                    if !matches!(
                        slot.as_str(),
                        "mainhand"
                            | "offhand"
                            | "head"
                            | "chest"
                            | "legs"
                            | "feet"
                            | "body"
                            | "saddle"
                    ) || (slot == "saddle"
                        && !(horse
                            || matches!(
                                name,
                                "minecraft:pig" | "minecraft:strider" | "minecraft:armor_stand"
                            )))
                    {
                        return Err(error(format!("Unsupported summon equipment slot {slot}")));
                    }
                    if normalized.iter().any(|(seen, _)| seen == slot) {
                        return Err(error(format!("Duplicate equipment slot {slot}")));
                    }
                    let Tag::Compound(stack) = stack else {
                        return Err(error(format!(
                            "Invalid equipment {slot}: expected item compound"
                        )));
                    };
                    let parsed = prepare_stack(&state.registry.items, stack)
                        .map_err(|e| error(format!("Invalid equipment {slot}: {e}")))?;
                    normalized.push((slot.clone(), Tag::Compound(parsed)));
                }
                Tag::Compound(normalized)
            }
            "drop_chances" => {
                let Tag::Compound(slots) = value else {
                    return Err(error("Invalid drop_chances: expected compound"));
                };
                let mut normalized = Vec::with_capacity(slots.len());
                for (slot, value) in slots {
                    if !matches!(
                        slot.as_str(),
                        "mainhand"
                            | "offhand"
                            | "head"
                            | "chest"
                            | "legs"
                            | "feet"
                            | "body"
                            | "saddle"
                    ) || normalized.iter().any(|(seen, _)| seen == slot)
                    {
                        return Err(error(format!("Invalid drop_chances slot {slot}")));
                    }
                    let chance = number(value, "drop_chances")?;
                    if chance < 0.0 || chance > f64::from(f32::MAX) {
                        return Err(error(format!("Invalid drop_chances {slot}")));
                    }
                    normalized.push((slot.clone(), Tag::Float(chance as f32)));
                }
                Tag::Compound(normalized)
            }
            "CustomName" => match value {
                Tag::String(_) => value.clone(),
                Tag::Compound(component)
                    if component.len() == 1
                        && component[0].0 == "text"
                        && matches!(component[0].1, Tag::String(_)) =>
                {
                    component[0].1.clone()
                }
                _ => {
                    return Err(error(
                        "Unsupported CustomName text component: expected literal string or {text:string}",
                    ));
                }
            },
            "Type" => {
                if !matches!(value, Tag::String(_)) {
                    return Err(error(format!("Invalid {key}: expected string")));
                }
                value.clone()
            }
            "stew_effects" => {
                let Tag::List(list) = value else {
                    return Err(error("Invalid stew_effects"));
                };
                for effect in &list.elements {
                    let Tag::Compound(fields) = effect else {
                        return Err(error("Invalid stew_effects entry"));
                    };
                    let id = fields.iter().find(|(k, _)| k == "id").map(|(_, v)| v);
                    if !matches!(id, Some(Tag::String(raw)) if mc_data::Identifier::parse(raw).is_ok())
                    {
                        return Err(error("Invalid stew_effects.id"));
                    }
                    if let Some((_, duration)) = fields.iter().find(|(k, _)| k == "duration") {
                        number(duration, "stew_effects.duration")?;
                    }
                }
                value.clone()
            }
            _ => value.clone(),
        };
        fields.push((key.clone(), value));
    }
    let base = VanillaEntity {
        id: name.into(),
        uuid: uuid::Uuid::new_v4(),
        pos,
        health: state
            .registry
            .blueprint(type_id)
            .map_or(20.0, |b| b.params.max_health),
        ..Default::default()
    };
    let native = mc_data::entity_nbt::entity_from_fields(&state.registry.items, base, &fields)
        .map_err(|e| error(e.to_string()))?;
    let image = EntityStateImage::from_record(&state.registry, &native)
        .map_err(|e| error(e.to_string()))?;
    let equipment = if authored.iter().any(|(key, _)| key == "equipment") {
        Some(
            crate::entity_loot::equipment_from_fields(&state.registry.items, &native.extra_fields)
                .map_err(|e| error(e.to_string()))?,
        )
    } else {
        None
    };
    let villager = if matches!(
        name,
        "minecraft:villager" | "minecraft:zombie_villager" | "minecraft:wandering_trader"
    ) {
        let mut villager = crate::villager::persistence::villager_from_fields(
            &state.registry.items,
            &fields,
            state.tick,
        )
        .map_err(|e| error(e.to_string()))?;
        villager.wandering = name == "minecraft:wandering_trader";
        if name == "minecraft:zombie_villager" {
            villager.conversion_time = Some(-1);
        }
        Some(villager)
    } else {
        None
    };
    Ok(Image {
        native,
        state: image,
        villager,
        equipment,
    })
}
pub(crate) fn apply(state: &mut TickState, id: EntityId, mut image: Image) {
    let slot = state.entities.slot_of(id).expect("new summon");
    let native = image.native;
    state.entities.set_uuid(slot, native.uuid);
    if state.dimension_globals.active {
        state.dimension_globals.reserved_uuids.insert(native.uuid);
    }
    state
        .entities
        .set_vel(slot, native.motion.map(|v| v * 20.0));
    state
        .entities
        .set_rot(slot, native.yaw, (native.pitch % 360.0).clamp(-90.0, 90.0));
    state.entities.set_ground(slot, native.on_ground);
    let row = state.entities.row_mut(slot);
    // Spawn initialization owns defaults. Only authored equipment/chances replace them.
    if image.equipment.is_none() {
        image.state.loot.body_armor = row.loot.body_armor.clone();
        image.state.loot.saddle = row.loot.saddle.clone();
    }
    let authored_chances = native.extra_fields.iter().find_map(|(key, value)| {
        if key == "drop_chances"
            && let Tag::Compound(slots) = value
        {
            return Some(slots);
        }
        None
    });
    for (index, name) in [
        "mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle",
    ]
    .into_iter()
    .enumerate()
    {
        if !authored_chances.is_some_and(|slots| slots.iter().any(|(key, _)| key == name)) {
            if index < 6 {
                image.state.loot.drop_chances[index] = row.loot.drop_chances[index];
            } else {
                image.state.loot.extra_drop_chances[index - 6] =
                    row.loot.extra_drop_chances[index - 6];
            }
        }
    }
    if !native
        .extra_fields
        .iter()
        .any(|(key, _)| key == "CanPickUpLoot")
    {
        image.state.loot.can_pick_up_loot = row.loot.can_pick_up_loot;
    }
    state
        .entities
        .set_entity_life(slot, image.state.entity_life);
    let (body, row) = state.entities.parts_mut(slot);
    row.health = native.health;
    row.age = image.state.age;
    row.villager = image.villager.map(Box::new);
    row.controls = image.state.controls;
    body.burn_ticks = image.state.burn_ticks;
    row.love = image.state.love;
    row.breed_cooldown = image.state.breed_cooldown;
    row.loot = image.state.loot;
    row.persistent = image.state.persistent;
    if native.id == "minecraft:armor_stand" {
        row.opaque_entity_fields = image.state.opaque_entity_fields;
    }
    if row.entity_life.child_ticks != 0 {
        body.height *= 0.5;
    }
    let appearance = Arc::make_mut(&mut row.appearance);
    if let Some(equipment) = image.equipment {
        appearance.equipment = equipment;
    }
    appearance.health = native.health;
    appearance.baby = row.entity_life.child_ticks != 0;
    if body.burn_ticks > 0 {
        appearance.shared_flags |= 1;
    }
    if let Some(wool) = native.sheep_wool {
        appearance
            .extra
            .retain(|(index, _)| *index != crate::blueprint::SHEEP_WOOL_DATA_INDEX);
        appearance.extra.push((
            crate::blueprint::SHEEP_WOOL_DATA_INDEX,
            EntityDataValue::Byte(wool as i8),
        ));
    }
    if let Some((_, Tag::String(name))) = native
        .extra_fields
        .iter()
        .find(|(key, _)| key == "CustomName")
    {
        appearance.custom_name = Some(name.clone());
    }
    appearance.custom_name_visible = native.extra_fields.iter().any(
        |(key, value)| matches!((key.as_str(), value), ("CustomNameVisible", Tag::Byte(v)) if *v != 0),
    );
    crate::entity_life::restore_appearance(body, row);
    if row.entity_life.child_ticks != 0 {
        let half_width = body.half_width * 0.5;
        state.entities.set_half_width(slot, half_width);
    }
    crate::entity_loot::restore(state, slot);
    crate::armor_stand::restore(state, slot);
    crate::taming::sync(state, slot);
    crate::villager::sync_appearance(state, slot);
    state.appearance_changed.push(id);
}

#[cfg(test)]
#[path = "command_summon_tests.rs"]
mod tests;
