//! Neutral vanilla entity-record NBT codec: the shared per-entity keys
//! (`id`, `UUID`, `Pos`, `Motion`, `Rotation`, `OnGround`, `Health`, `Age`,
//! `Item` + `PickupDelay` for item entities). One codec for the Anvil
//! entity regions and the old checkpoint file; crate-specific extras stay in
//! their crates.

use mc_nbt::{ListTag, Tag, tag_type};

use crate::item_nbt::{ItemNbtError, field, stack_from_fields, stack_to_fields};
use crate::{ItemStack, items::ItemRegistry};

/// Neutral vanilla entity record (mob or item entity).
#[derive(Debug, Clone, PartialEq)]
pub struct VanillaEntity {
    /// Namespaced entity id, e.g. `minecraft:cow`.
    pub id: String,
    pub uuid: uuid::Uuid,
    pub pos: [f64; 3],
    pub motion: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub health: f32,
    pub age: i32,
    pub breed_cooldown: i32,
    pub pickup_delay: i32,
    /// Item-entity stack (`Item` compound).
    pub item: Option<ItemStack>,
    /// Vanilla experience orb `Value` and `Count`.
    pub experience_orb: Option<(i32, i32)>,
    /// Sheep color (low four bits) and sheared flag (`0x10`), persisted as
    /// vanilla `Color` and `Sheared` rather than a Solaris-specific key.
    pub sheep_wool: Option<u8>,
    /// Vanilla effect compounds retained without depending on gameplay types.
    pub active_effects: Vec<Tag>,
    pub absorption_amount: f32,
    /// Solaris wire id (`solaris:WireId`; vanilla ignores unknown keys).
    /// Lets journal tail entries keyed by wire id match after recovery.
    pub wire_id: i32,
    /// Species-owned vanilla fields, retained through neutral region codecs.
    /// `Age` may override the generic timer for villagers; other shared keys
    /// cannot be replaced. The gameplay owner validates these fields.
    pub extra_fields: Vec<(String, Tag)>,
}

fn shared_field(name: &str) -> bool {
    matches!(
        name,
        "id" | "UUID"
            | "Pos"
            | "Motion"
            | "Rotation"
            | "OnGround"
            | "Health"
            | "Age"
            | "PickupDelay"
            | "Item"
            | "Value"
            | "Count"
            | "Color"
            | "Sheared"
            | "active_effects"
            | "AbsorptionAmount"
            | "solaris:WireId"
            | "solaris:BreedCooldown"
    )
}

fn float_field(fields: &[(String, Tag)], name: &str) -> Option<f32> {
    match field(fields, name)? {
        Tag::Float(value) => Some(*value),
        Tag::Double(value) => Some(*value as f32),
        Tag::Byte(value) => Some(f32::from(*value)),
        Tag::Short(value) => Some(f32::from(*value)),
        Tag::Int(value) => Some(*value as f32),
        Tag::Long(value) => Some(*value as f32),
        _ => None,
    }
}

fn int_field(fields: &[(String, Tag)], name: &str) -> Option<i32> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(i32::from(*value)),
        Tag::Short(value) => Some(i32::from(*value)),
        Tag::Int(value) => Some(*value),
        _ => None,
    }
}

fn double_list(tag: &Tag) -> Option<[f64; 3]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != 3 {
        return None;
    }
    let mut out = [0.0; 3];
    for (index, element) in list.elements.iter().enumerate() {
        let Tag::Double(value) = element else {
            return None;
        };
        out[index] = *value;
    }
    Some(out)
}

fn float_pair(tag: &Tag) -> Option<[f32; 2]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != 2 {
        return None;
    }
    let mut out = [0.0; 2];
    for (index, element) in list.elements.iter().enumerate() {
        let Tag::Float(value) = element else {
            return None;
        };
        out[index] = *value;
    }
    Some(out)
}

fn uuid_field(fields: &[(String, Tag)]) -> Option<uuid::Uuid> {
    match field(fields, "UUID")? {
        Tag::IntArray(values) if values.len() == 4 => {
            let mut bits: u128 = 0;
            for value in values {
                bits = (bits << 32) | (*value as u32 as u128);
            }
            Some(uuid::Uuid::from_u128(bits))
        }
        _ => None,
    }
}

fn uuid_tag(uuid: uuid::Uuid) -> Tag {
    let bits = uuid.as_u128();
    Tag::IntArray(vec![
        (bits >> 96) as i32,
        ((bits >> 64) & 0xffff_ffff) as i32,
        ((bits >> 32) & 0xffff_ffff) as i32,
        (bits & 0xffff_ffff) as i32,
    ])
}

/// Decode the shared entity keys; missing keys keep `base` motion/health.
pub fn entity_from_fields(
    items: &ItemRegistry,
    base: VanillaEntity,
    fields: &[(String, Tag)],
) -> Result<VanillaEntity, ItemNbtError> {
    let mut out = base;
    if let Some(Tag::String(id)) = field(fields, "id") {
        out.id = id.clone();
    }
    if let Some(uuid) = uuid_field(fields) {
        out.uuid = uuid;
    }
    if let Some(pos) = field(fields, "Pos").and_then(double_list) {
        out.pos = pos;
    }
    if let Some(motion) = field(fields, "Motion").and_then(double_list) {
        out.motion = motion;
    }
    if let Some(rot) = field(fields, "Rotation").and_then(float_pair) {
        out.yaw = rot[0];
        out.pitch = rot[1];
    }
    if let Some(Tag::Byte(ground)) = field(fields, "OnGround") {
        out.on_ground = *ground != 0;
    }
    if let Some(health) = float_field(fields, "Health") {
        out.health = health.max(0.0);
    }
    if let Some(age) = int_field(fields, "Age") {
        out.age = age;
    }
    if let Some(delay) = int_field(fields, "PickupDelay") {
        out.pickup_delay = delay.max(0);
    }
    if out.id == "minecraft:experience_orb" {
        out.health = int_field(fields, "Health").unwrap_or(5).max(0) as f32;
        out.experience_orb = Some((
            int_field(fields, "Value").unwrap_or(0).max(0),
            int_field(fields, "Count").unwrap_or(1).max(1),
        ));
    }
    match field(fields, "solaris:WireId") {
        Some(Tag::Int(wire)) if *wire >= 0 => out.wire_id = *wire,
        Some(_) => {
            return Err(ItemNbtError::InvalidComponent(
                "solaris:WireId: expected nonnegative int".into(),
            ));
        }
        None => {}
    }
    if let Some(cooldown) = int_field(fields, "solaris:BreedCooldown") {
        out.breed_cooldown = cooldown.max(0);
    }
    if let Some(Tag::List(list)) = field(fields, "active_effects") {
        out.active_effects = list.elements.clone();
    }
    if let Some(absorption) = float_field(fields, "AbsorptionAmount") {
        out.absorption_amount = absorption;
    }
    if let Some(Tag::Compound(item)) = field(fields, "Item") {
        out.item = stack_from_fields(items, item)?;
    }
    if out.id == "minecraft:sheep" {
        let color = match field(fields, "Color") {
            Some(Tag::Byte(value)) => *value as u8 & 0x0f,
            _ => 0,
        };
        let sheared = matches!(field(fields, "Sheared"), Some(Tag::Byte(value)) if *value != 0);
        out.sheep_wool = Some(color | if sheared { 0x10 } else { 0 });
    }
    let villager = matches!(
        out.id.as_str(),
        "minecraft:villager" | "minecraft:zombie_villager" | "minecraft:wandering_trader"
    );
    out.extra_fields = fields
        .iter()
        .filter(|(key, _)| !shared_field(key) || (villager && key == "Age"))
        .cloned()
        .collect();
    Ok(out)
}

/// Encode the shared entity keys (vanilla names).
pub fn entity_to_fields(
    items: &ItemRegistry,
    entity: &VanillaEntity,
) -> Result<Vec<(String, Tag)>, ItemNbtError> {
    let mut fields = vec![
        ("id".into(), Tag::String(entity.id.clone())),
        ("UUID".into(), uuid_tag(entity.uuid)),
        (
            "Pos".into(),
            Tag::List(ListTag {
                element_type: tag_type::DOUBLE,
                elements: entity.pos.map(Tag::Double).to_vec(),
            }),
        ),
        (
            "Motion".into(),
            Tag::List(ListTag {
                element_type: tag_type::DOUBLE,
                elements: entity.motion.map(Tag::Double).to_vec(),
            }),
        ),
        (
            "Rotation".into(),
            Tag::List(ListTag {
                element_type: tag_type::FLOAT,
                elements: vec![Tag::Float(entity.yaw), Tag::Float(entity.pitch)],
            }),
        ),
        ("OnGround".into(), Tag::Byte(i8::from(entity.on_ground))),
        (
            "Health".into(),
            if entity.experience_orb.is_some() {
                Tag::Short(entity.health as i16)
            } else {
                Tag::Float(entity.health)
            },
        ),
        (
            "Age".into(),
            if entity.experience_orb.is_some() {
                Tag::Short(entity.age as i16)
            } else {
                Tag::Int(entity.age)
            },
        ),
    ];
    fields.push((
        "active_effects".into(),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: entity.active_effects.clone(),
        }),
    ));
    fields.push((
        "AbsorptionAmount".into(),
        Tag::Float(entity.absorption_amount),
    ));
    if entity.id == "minecraft:sheep" {
        let wool = entity.sheep_wool.unwrap_or(0);
        fields.push(("Color".into(), Tag::Byte((wool & 0x0f) as i8)));
        fields.push(("Sheared".into(), Tag::Byte(i8::from(wool & 0x10 != 0))));
    }
    if entity.pickup_delay != 0 {
        fields.push(("PickupDelay".into(), Tag::Short(entity.pickup_delay as i16)));
    }
    if let Some((value, count)) = entity.experience_orb {
        fields.push(("Value".into(), Tag::Short(value as i16)));
        fields.push(("Count".into(), Tag::Int(count)));
    }
    // Solaris-only bookkeeping (vanilla ignores unknown keys).
    if entity.wire_id > 0 {
        fields.push(("solaris:WireId".into(), Tag::Int(entity.wire_id)));
    } else if entity.wire_id < 0 {
        return Err(ItemNbtError::InvalidComponent(
            "solaris:WireId: expected nonnegative int".into(),
        ));
    }
    if entity.breed_cooldown != 0 {
        fields.push((
            "solaris:BreedCooldown".into(),
            Tag::Int(entity.breed_cooldown),
        ));
    }
    if let Some(stack) = &entity.item
        && !stack.is_empty()
    {
        fields.push(("Item".into(), Tag::Compound(stack_to_fields(items, stack)?)));
    }
    let mut seen = std::collections::BTreeSet::new();
    for (key, value) in &entity.extra_fields {
        if !seen.insert(key.as_str()) {
            return Err(ItemNbtError::InvalidComponent(format!(
                "duplicate entity field {key}"
            )));
        }
        if shared_field(key) {
            if key == "Age"
                && matches!(
                    entity.id.as_str(),
                    "minecraft:villager"
                        | "minecraft:zombie_villager"
                        | "minecraft:wandering_trader"
                )
                && matches!(value, Tag::Int(_))
            {
                let age = fields
                    .iter_mut()
                    .find(|(key, _)| key == "Age")
                    .expect("shared age");
                age.1 = value.clone();
            } else {
                return Err(ItemNbtError::InvalidComponent(format!(
                    "species field replaces shared {key}"
                )));
            }
        } else {
            fields.push((key.clone(), value.clone()));
        }
    }
    Ok(fields)
}

impl Default for VanillaEntity {
    fn default() -> Self {
        Self {
            id: "minecraft:pig".to_owned(),
            uuid: uuid::Uuid::nil(),
            pos: [0.0, 80.0, 0.0],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            health: 20.0,
            age: 0,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            experience_orb: None,
            sheep_wool: None,
            active_effects: Vec::new(),
            absorption_amount: 0.0,
            wire_id: 0,
            extra_fields: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> ItemRegistry {
        crate::items::solaris_required_items()
    }

    #[test]
    fn entity_round_trip_keeps_vanilla_keys() {
        let items = items();
        let entity = VanillaEntity {
            id: "minecraft:cow".to_owned(),
            uuid: uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0),
            pos: [10.0, 80.0, 10.0],
            motion: [0.5, -1.0, 0.0],
            yaw: 1.0,
            pitch: 0.5,
            on_ground: true,
            health: 14.0,
            age: 3,
            breed_cooldown: 100,
            pickup_delay: 0,
            item: None,
            experience_orb: None,
            sheep_wool: None,
            active_effects: Vec::new(),
            absorption_amount: 0.0,
            wire_id: 7,
            extra_fields: Vec::new(),
        };
        let fields = entity_to_fields(&items, &entity).expect("encode");
        for key in [
            "id", "UUID", "Pos", "Motion", "Rotation", "OnGround", "Health", "Age",
        ] {
            assert!(fields.iter().any(|(name, _)| name == key), "has {key}");
        }
        let back = entity_from_fields(&items, VanillaEntity::default(), &fields).expect("decode");
        assert_eq!(back, entity);
    }

    #[test]
    fn experience_orb_round_trip_uses_vanilla_value_count_and_age() {
        let items = items();
        let entity = VanillaEntity {
            id: "minecraft:experience_orb".into(),
            uuid: uuid::Uuid::from_u128(88),
            health: 5.0,
            age: 5999,
            experience_orb: Some((7, 4)),
            ..Default::default()
        };
        let fields = entity_to_fields(&items, &entity).expect("encode orb");
        assert!(fields.contains(&("Value".into(), Tag::Short(7))));
        assert!(fields.contains(&("Count".into(), Tag::Int(4))));
        assert!(fields.contains(&("Age".into(), Tag::Short(5999))));
        let back =
            entity_from_fields(&items, VanillaEntity::default(), &fields).expect("decode orb");
        assert_eq!(back, entity);
    }
}

#[cfg(test)]
#[path = "entity_nbt_import_tests.rs"]
mod import_tests;
