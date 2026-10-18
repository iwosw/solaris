//! 26.1.2 ContainerEntity/ContainerHelper and minecart javap save-key contract.
use super::{VehicleKind, VehicleState, container_size};
use crate::config::CoreError;
use crate::registry::CoreRegistry;
use mc_data::item_nbt::{stack_from_fields, stack_to_fields};
use mc_data::{ItemStack, entity_nbt::VanillaEntity};
use mc_nbt::{ListTag, Tag, tag_type};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct VehicleImage {
    kind: VehicleKind,
    slots: Vec<ItemStack>,
    #[serde(default)]
    loot_table: Option<mc_data::Identifier>,
    #[serde(default)]
    loot_seed: i64,
    fuel: u16,
    push: [f64; 2],
    fuse: i32,
    explosion_power: f32,
    explosion_speed_factor: f32,
    enabled: bool,
    transfer_cooldown: u8,
    #[serde(default)]
    on_ground: bool,
}
impl TryFrom<VehicleImage> for VehicleState {
    type Error = String;
    fn try_from(v: VehicleImage) -> Result<Self, String> {
        if !matches!(v.slots.len(), 0 | 5 | 27) {
            return Err("vehicle slots: expected 0, 5 or 27 slots".into());
        }
        if v.fuel > 32000 {
            return Err("vehicle Fuel: expected 0..32000".into());
        }
        if v.push.iter().any(|v| !v.is_finite()) {
            return Err("vehicle PushX/PushZ: expected finite double".into());
        }
        if !(-1..=32767).contains(&v.fuse) {
            return Err("vehicle fuse: expected -1..32767".into());
        }
        if !v.explosion_power.is_finite() || !(0.0..=128.0).contains(&v.explosion_power) {
            return Err("vehicle explosion_power: expected 0..128 finite float".into());
        }
        if !v.explosion_speed_factor.is_finite()
            || !(0.0..=128.0).contains(&v.explosion_speed_factor)
        {
            return Err("vehicle explosion_speed_factor: expected 0..128 finite float".into());
        }
        if v.slots.iter().any(|s| s.count < 0 || s.count > 99) {
            return Err("vehicle Items: invalid stack count".into());
        }
        let state = Self {
            kind: v.kind,
            slots: v.slots,
            loot_table: v.loot_table,
            loot_seed: v.loot_seed,
            fuel: v.fuel,
            push: v.push,
            fuse: v.fuse,
            explosion_power: v.explosion_power,
            explosion_speed_factor: v.explosion_speed_factor,
            enabled: v.enabled,
            transfer_cooldown: v.transfer_cooldown,
            on_ground: v.on_ground,
            paddles: [false; 2],
        };
        super::loot::validate(&state)?;
        Ok(state)
    }
}
fn error(message: impl Into<String>) -> CoreError {
    CoreError::Storage(message.into())
}
fn unique<'a>(fields: &'a [(String, Tag)], key: &str) -> Result<Option<&'a Tag>, CoreError> {
    let mut matching = fields.iter().filter(|(name, _)| name == key);
    let first = matching.next().map(|(_, tag)| tag);
    if matching.next().is_some() {
        return Err(error(format!("vehicle {key}: duplicate field")));
    }
    Ok(first)
}
pub(crate) fn from_record(
    registry: &CoreRegistry,
    record: &VanillaEntity,
) -> Result<Option<VehicleState>, CoreError> {
    let Some(mut v) = VehicleState::new(&record.id) else {
        return Ok(None);
    };
    let fields = &record.extra_fields;
    v.on_ground = record.on_ground;
    if let Some(tag) = unique(fields, "LootTable")? {
        let Tag::String(name) = tag else {
            return Err(error("vehicle LootTable: expected identifier string"));
        };
        v.loot_table = Some(
            mc_data::Identifier::parse(name)
                .map_err(|_| error("vehicle LootTable: invalid identifier"))?,
        );
    }
    if let Some(tag) = unique(fields, "LootTableSeed")? {
        let Tag::Long(seed) = tag else {
            return Err(error("vehicle LootTableSeed: expected long"));
        };
        v.loot_seed = *seed;
    }
    if let Some(items) = unique(fields, "Items")? {
        let Tag::List(list) = items else {
            return Err(error("vehicle Items: expected compound list"));
        };
        if list.element_type != tag_type::COMPOUND
            && !(list.element_type == tag_type::END && list.elements.is_empty())
        {
            return Err(error("vehicle Items: expected compound list"));
        }
        let mut seen = vec![false; v.slots.len()];
        for entry in &list.elements {
            let Tag::Compound(fields) = entry else {
                return Err(error("vehicle Items: expected compound item"));
            };
            let index = match unique(fields, "Slot")? {
                Some(Tag::Byte(index)) => usize::try_from(*index).ok(),
                _ => None,
            }
            .filter(|i| *i < v.slots.len())
            .ok_or_else(|| error("vehicle Items.Slot: out of range or wrong type"))?;
            if seen[index] {
                return Err(error(format!("vehicle Items.Slot {index}: duplicate slot")));
            }
            seen[index] = true;
            v.slots[index] = stack_from_fields(&registry.items, fields)
                .map_err(|e| error(format!("vehicle Items.Slot {index}: {e}")))?
                .ok_or_else(|| error(format!("vehicle Items.Slot {index}: missing item id")))?;
            if v.slots[index].is_empty()
                || v.slots[index].count > registry.item_rules().max_stack(&v.slots[index]).min(99)
            {
                return Err(error(format!("vehicle Items.Slot {index}: invalid count")));
            }
        }
    }
    if let Some(tag) = unique(fields, "Fuel")? {
        let Tag::Short(fuel) = tag else {
            return Err(error("vehicle Fuel: expected short"));
        };
        v.fuel = u16::try_from(*fuel)
            .ok()
            .filter(|fuel| *fuel <= 32000)
            .ok_or_else(|| error("vehicle Fuel: expected 0..32000"))?;
    }
    for (axis, key) in [(0, "PushX"), (1, "PushZ")] {
        if let Some(tag) = unique(fields, key)? {
            let Tag::Double(value) = tag else {
                return Err(error(format!("vehicle {key}: expected finite double")));
            };
            if !value.is_finite() {
                return Err(error(format!("vehicle {key}: expected finite double")));
            }
            v.push[axis] = *value;
        }
    }
    if let Some(tag) = unique(fields, "fuse")? {
        let Tag::Int(fuse) = tag else {
            return Err(error("vehicle fuse: expected int"));
        };
        if !(-1..=32767).contains(fuse) {
            return Err(error("vehicle fuse: expected -1..32767"));
        }
        v.fuse = *fuse;
    }
    for (key, target) in [
        ("explosion_power", &mut v.explosion_power),
        ("explosion_speed_factor", &mut v.explosion_speed_factor),
    ] {
        if let Some(tag) = unique(fields, key)? {
            let Tag::Float(value) = tag else {
                return Err(error(format!(
                    "vehicle {key}: expected 0..128 finite float"
                )));
            };
            if !value.is_finite() || !(0.0..=128.0).contains(value) {
                return Err(error(format!(
                    "vehicle {key}: expected 0..128 finite float"
                )));
            }
            *target = *value;
        }
    }
    if let Some(tag) = unique(fields, "Enabled")? {
        let Tag::Byte(enabled) = tag else {
            return Err(error("vehicle Enabled: expected byte"));
        };
        if !matches!(enabled, 0 | 1) {
            return Err(error("vehicle Enabled: expected boolean byte"));
        }
        v.enabled = *enabled != 0;
    }
    if let Some(tag) = unique(fields, "solaris:VehicleTransferCooldown")? {
        let Tag::Int(cooldown) = tag else {
            return Err(error(
                "vehicle solaris:VehicleTransferCooldown: expected int",
            ));
        };
        v.transfer_cooldown = u8::try_from(*cooldown)
            .map_err(|_| error("vehicle solaris:VehicleTransferCooldown: expected u8"))?;
    }
    validate_kind(&v, &record.id).map_err(error)?;
    super::loot::validate(&v).map_err(error)?;
    super::loot::validate_items(&v, &registry.chest_loot, &registry.items)?;
    Ok(Some(v))
}
/// ItemRegistry is needed for canonical named stacks, never numeric proprietary inventory NBT.
pub(crate) fn to_fields(
    v: &VehicleState,
    items: &mc_data::items::ItemRegistry,
) -> Vec<(String, Tag)> {
    let mut fields = Vec::new();
    if let Some(id) = &v.loot_table {
        fields.push(("LootTable".into(), Tag::String(id.to_string())));
        if v.loot_seed != 0 {
            fields.push(("LootTableSeed".into(), Tag::Long(v.loot_seed)));
        }
    } else if !v.slots.is_empty() {
        let elements = v
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.is_empty())
            .map(|(index, stack)| {
                let mut fields =
                    stack_to_fields(items, stack).expect("registry-valid vehicle stack");
                fields.push(("Slot".into(), Tag::Byte(index as i8)));
                Tag::Compound(fields)
            })
            .collect();
        fields.push((
            "Items".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements,
            }),
        ));
    }
    // Save only variant-applicable fields; the variant is the vanilla entity id.
    if v.fuel != 0 || v.push != [0.0; 2] {
        fields.extend([
            ("Fuel".into(), Tag::Short(v.fuel as i16)),
            ("PushX".into(), Tag::Double(v.push[0])),
            ("PushZ".into(), Tag::Double(v.push[1])),
        ]);
    }
    if v.fuse >= 0 {
        fields.push(("fuse".into(), Tag::Int(v.fuse)));
    }
    if v.explosion_power != 4.0 {
        fields.push(("explosion_power".into(), Tag::Float(v.explosion_power)));
    }
    if v.explosion_speed_factor != 1.0 {
        fields.push((
            "explosion_speed_factor".into(),
            Tag::Float(v.explosion_speed_factor),
        ));
    }
    if v.slots.len() == 5 {
        fields.push(("Enabled".into(), Tag::Byte(i8::from(v.enabled))));
    }
    if v.transfer_cooldown != 0 {
        fields.push((
            "solaris:VehicleTransferCooldown".into(),
            Tag::Int(i32::from(v.transfer_cooldown)),
        ));
    }
    fields
}

pub(crate) fn validate_kind(v: &VehicleState, name: &str) -> Result<(), String> {
    if super::name_kind(name) != Some(v.kind) || container_size(name) != v.slots.len() {
        return Err(format!("vehicle {name}: kind/container size mismatch"));
    }
    if name != "minecraft:furnace_minecart" && (v.fuel != 0 || v.push != [0.0; 2]) {
        return Err(format!(
            "vehicle {name}: furnace state on non-furnace vehicle"
        ));
    }
    if name != "minecraft:tnt_minecart"
        && (v.fuse != -1 || v.explosion_power != 4.0 || v.explosion_speed_factor != 1.0)
    {
        return Err(format!("vehicle {name}: TNT state on non-TNT vehicle"));
    }
    if name != "minecraft:hopper_minecart" && (!v.enabled || v.transfer_cooldown != 0) {
        return Err(format!(
            "vehicle {name}: hopper state on non-hopper vehicle"
        ));
    }
    Ok(())
}

pub(crate) fn validate_replay(
    registry: &CoreRegistry,
    record: &crate::journal::ReplayEntity,
) -> Result<(), CoreError> {
    let Some(v) = &record.state.entity_life.vehicle else {
        return Ok(());
    };
    let name = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(record.type_id)
        .map(|c| c.name)
        .ok_or_else(|| error("vehicle type_id: unknown entity type"))?;
    validate_kind(v, name).map_err(error)?;
    super::loot::validate(v).map_err(error)?;
    super::loot::validate_items(v, &registry.chest_loot, &registry.items)?;
    if record.pos.iter().chain(&record.vel).any(|v| !v.is_finite())
        || !record.yaw.is_finite()
        || !record.pitch.is_finite()
    {
        return Err(error(format!("vehicle {name}: nonfinite motion")));
    }
    let rules = registry.item_rules();
    for (index, stack) in v.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        if registry.items.name_of(stack.item_id).is_none() {
            return Err(error(format!(
                "vehicle Items.Slot {index}: unknown item id {}",
                stack.item_id
            )));
        }
        if stack.count <= 0 || stack.count > rules.max_stack(stack).min(99) {
            return Err(error(format!("vehicle Items.Slot {index}: invalid count")));
        }
    }
    Ok(())
}
