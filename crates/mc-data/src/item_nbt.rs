//! Canonical vanilla item-stack NBT codec: `id`/`count` plus the
//! `components` compound (`minecraft:damage`, `minecraft:custom_name`,
//! `minecraft:enchantments`, `minecraft:item_model`, stew effects, extended
//! components). One codec for every crate (playerdata, containers, entity
//! `Item`, Anvil block entities); `Slot` framing stays at the call site.

use std::sync::Arc;

use mc_nbt::Tag;

use super::item_stack::{
    ExtendedItemComponents, ItemStack, STEW_EFFECTS_COMPONENT, StewEffect, decode_stew_effects,
    encode_stew_effects,
};
use crate::{Identifier, items::ItemRegistry};

/// Codec failure (invalid vanilla NBT or unknown item). Structured so
/// callers map to their own error types without string parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemNbtError {
    /// `id` is not a valid identifier.
    InvalidId(String),
    /// `id` parses but is not in the item registry (encode or decode).
    UnknownItem(String),
    /// Bad enchantment entry or unknown enchantment.
    InvalidEnchantment(String),
    /// Any other malformed component NBT.
    InvalidComponent(String),
}

impl std::fmt::Display for ItemNbtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(f, "item NBT: invalid id {id}"),
            Self::UnknownItem(id) => write!(f, "item NBT: unknown item {id}"),
            Self::InvalidEnchantment(id) => write!(f, "item NBT: invalid enchantment {id}"),
            Self::InvalidComponent(reason) => write!(f, "item NBT: {reason}"),
        }
    }
}

impl std::error::Error for ItemNbtError {}

pub const DAMAGE_COMPONENT: &str = "minecraft:damage";
pub const CUSTOM_NAME_COMPONENT: &str = "minecraft:custom_name";
pub const ITEM_MODEL_COMPONENT: &str = "minecraft:item_model";
pub const ENCHANTMENTS_COMPONENT: &str = "minecraft:enchantments";

/// Find a field in an NBT compound field list.
#[must_use]
pub fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, tag)| tag)
}

fn field_mut<'a>(fields: &'a mut [(String, Tag)], name: &str) -> Option<&'a mut Tag> {
    fields
        .iter_mut()
        .find(|(key, _)| key == name)
        .map(|(_, tag)| tag)
}

fn set_field(fields: &mut Vec<(String, Tag)>, name: &str, value: Tag) {
    if let Some(slot) = fields.iter().position(|(key, _)| key == name) {
        fields[slot].1 = value;
    } else {
        fields.push((name.to_owned(), value));
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

fn string_field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a str> {
    match field(fields, name)? {
        Tag::String(value) => Some(value),
        _ => None,
    }
}

/// Decode one vanilla item compound (`id` + `count` + `components`).
/// Returns `Ok(None)` when `id` is absent (empty slot).
pub fn stack_from_fields(
    items: &ItemRegistry,
    fields: &[(String, Tag)],
) -> Result<Option<ItemStack>, ItemNbtError> {
    let Some(item_name) = string_field(fields, "id") else {
        return Ok(None);
    };
    let parsed = Identifier::parse(item_name.to_string())
        .map_err(|_| ItemNbtError::InvalidId(item_name.to_owned()))?;
    let item_id = items
        .id_of(&parsed)
        .ok_or_else(|| ItemNbtError::UnknownItem(item_name.to_owned()))?;
    let mut stack = ItemStack {
        count: int_field(fields, "count").unwrap_or(1).max(0),
        item_id,
        damage: damage_component(fields),
        enchantments: enchantments_component(fields)?,
        custom_name: custom_name_component(fields).map(String::into_boxed_str),
        item_model: item_model_component(fields)?,
        stew_effects: stew_effects_load(fields)?,
        components: stack_components_load(fields, items)?,
    };
    preserve_components(items, fields, &mut stack)?;
    Ok(Some(stack))
}

/// Admit a command's component patch through the same native codecs used by
/// playerdata, loot and wire preservation. Opaque components are shape-checked
/// before publishing them; unsupported values never become inert inventory data.
pub fn stack_from_component_patch(
    items: &ItemRegistry,
    item: &Identifier,
    components: &[(String, Tag)],
) -> Result<ItemStack, ItemNbtError> {
    crate::item_component_values::validate(items, components, 0)
        .map_err(ItemNbtError::InvalidComponent)?;
    stack_from_fields(
        items,
        &[
            ("id".into(), Tag::String(item.as_str().to_owned())),
            ("count".into(), Tag::Int(1)),
            ("components".into(), Tag::Compound(components.to_vec())),
        ],
    )?
    .ok_or_else(|| ItemNbtError::UnknownItem(item.as_str().to_owned()))
}

/// Encode one vanilla item compound (no `Slot` key — the container adds it).
pub fn stack_to_fields(
    items: &ItemRegistry,
    stack: &ItemStack,
) -> Result<Vec<(String, Tag)>, ItemNbtError> {
    let name = items
        .name_of(stack.item_id)
        .ok_or_else(|| ItemNbtError::UnknownItem(stack.item_id.to_string()))?;
    let mut fields = vec![
        ("id".into(), Tag::String(name.as_str().to_string())),
        ("count".into(), Tag::Int(stack.count)),
    ];
    if let Some(damage) = stack.damage {
        set_damage_component(&mut fields, damage);
    }
    if let Some(custom_name) = &stack.custom_name {
        set_custom_name_component(&mut fields, custom_name);
    }
    if let Some(item_model) = &stack.item_model {
        set_item_model_component(&mut fields, item_model);
    }
    if !stack.enchantments.is_empty() {
        set_enchantments_component(&mut fields, &stack.enchantments);
    }
    stew_effects_save(&mut fields, &stack.stew_effects);
    stack_components_save(&mut fields, stack.extended(), items)?;
    restore_components(&mut fields, &stack.extended().preserved_components)?;
    Ok(fields)
}

fn damage_component(fields: &[(String, Tag)]) -> Option<i32> {
    let Tag::Compound(components) = field(fields, "components")? else {
        return None;
    };
    int_field(components, DAMAGE_COMPONENT)
}

fn set_damage_component(fields: &mut Vec<(String, Tag)>, damage: i32) {
    set_component_field(fields, DAMAGE_COMPONENT, Tag::Int(damage));
}

fn set_component_field(fields: &mut Vec<(String, Tag)>, name: &str, value: Tag) {
    let components = field_mut(fields, "components").and_then(|tag| match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    });
    if let Some(components) = components {
        set_field(components, name, value);
    } else {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(name.to_owned(), value)]),
        );
    }
}

/// Project literal names for typed consumers; the complete component is preserved.
fn custom_name_component(fields: &[(String, Tag)]) -> Option<String> {
    let Tag::Compound(components) = field(fields, "components")? else {
        return None;
    };
    match field(components, CUSTOM_NAME_COMPONENT)? {
        Tag::String(value) => Some(value.clone()),
        Tag::Compound(component) => string_field(component, "text").map(str::to_owned),
        _ => None,
    }
}

fn set_custom_name_component(fields: &mut Vec<(String, Tag)>, custom_name: &str) {
    let value = Tag::Compound(vec![("text".into(), Tag::String(custom_name.to_owned()))]);
    set_component_field(fields, CUSTOM_NAME_COMPONENT, value);
}

fn item_model_component(fields: &[(String, Tag)]) -> Result<Option<Arc<Identifier>>, ItemNbtError> {
    let Some(Tag::Compound(components)) = field(fields, "components") else {
        return Ok(None);
    };
    let Some(Tag::String(item_model)) = field(components, ITEM_MODEL_COMPONENT) else {
        return Ok(None);
    };
    Identifier::parse(item_model.clone())
        .map(Arc::new)
        .map(Some)
        .map_err(|_| ItemNbtError::InvalidComponent(format!("item model: {item_model}")))
}

fn set_item_model_component(fields: &mut Vec<(String, Tag)>, item_model: &Identifier) {
    set_component_field(
        fields,
        ITEM_MODEL_COMPONENT,
        Tag::String(item_model.as_str().to_owned()),
    );
}

fn enchantments_component(
    fields: &[(String, Tag)],
) -> Result<Vec<crate::ItemEnchantment>, ItemNbtError> {
    let Some(Tag::Compound(components)) = field(fields, "components") else {
        return Ok(Vec::new());
    };
    let Some(Tag::Compound(enchantments)) = field(components, ENCHANTMENTS_COMPONENT) else {
        return Ok(Vec::new());
    };
    let mut result = Vec::with_capacity(enchantments.len());
    for (id, level) in enchantments {
        let Tag::Int(level) = level else {
            return Err(ItemNbtError::InvalidEnchantment(id.clone()));
        };
        let parsed = Identifier::parse(id.clone())
            .map_err(|_| ItemNbtError::InvalidEnchantment(id.clone()))?;
        if !(1..=255).contains(level)
            || crate::required_registry_entry_id("enchantment", &parsed).is_none()
        {
            return Err(ItemNbtError::InvalidEnchantment(id.clone()));
        }
        result.push(crate::ItemEnchantment {
            id: parsed,
            level: *level,
        });
    }
    result.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    Ok(result)
}

fn set_enchantments_component(
    fields: &mut Vec<(String, Tag)>,
    enchantments: &[crate::ItemEnchantment],
) {
    let value = Tag::Compound(
        enchantments
            .iter()
            .map(|enchantment| {
                (
                    enchantment.id.as_str().to_string(),
                    Tag::Int(enchantment.level),
                )
            })
            .collect(),
    );
    set_component_field(fields, ENCHANTMENTS_COMPONENT, value);
}

const STACK_COMPONENT_KEYS: [&str; 10] = [
    "minecraft:max_damage",
    "minecraft:custom_data",
    "minecraft:potion_contents",
    "minecraft:trim",
    "minecraft:banner_patterns",
    "minecraft:repair_cost",
    "minecraft:stored_enchantments",
    "minecraft:charged_projectiles",
    "minecraft:fireworks",
    "minecraft:ominous_bottle_amplifier",
];

fn stack_components_load(
    fields: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<Option<Box<ExtendedItemComponents>>, ItemNbtError> {
    match field(fields, "components") {
        None => Ok(None),
        Some(Tag::Compound(components)) => ExtendedItemComponents::from_nbt(components, items)
            .map(ExtendedItemComponents::into_optional)
            .map_err(|reason| ItemNbtError::InvalidComponent(reason.to_owned())),
        Some(_) => Err(ItemNbtError::InvalidComponent(
            "components must be a compound".to_owned(),
        )),
    }
}

fn stack_components_save(
    fields: &mut Vec<(String, Tag)>,
    components: &ExtendedItemComponents,
    items: &ItemRegistry,
) -> Result<(), ItemNbtError> {
    let encoded = components
        .to_nbt(items)
        .map_err(|reason| ItemNbtError::InvalidComponent(reason.to_owned()))?;
    if let Some(Tag::Compound(previous)) = field_mut(fields, "components") {
        previous.retain(|(name, _)| !STACK_COMPONENT_KEYS.contains(&name.as_str()));
        previous.extend(encoded);
    } else if !encoded.is_empty() {
        set_field(fields, "components", Tag::Compound(encoded));
    }
    Ok(())
}

fn stew_effects_load(fields: &[(String, Tag)]) -> Result<Vec<StewEffect>, ItemNbtError> {
    let Some(Tag::Compound(components)) = field(fields, "components") else {
        return Ok(Vec::new());
    };
    field(components, STEW_EFFECTS_COMPONENT)
        .map(|tag| {
            decode_stew_effects(tag)
                .map_err(|reason| ItemNbtError::InvalidComponent(format!("stew: {reason}")))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn stew_effects_save(fields: &mut Vec<(String, Tag)>, effects: &[StewEffect]) {
    if let Some(Tag::Compound(components)) = field_mut(fields, "components") {
        if effects.is_empty() {
            components.retain(|(name, _)| name != STEW_EFFECTS_COMPONENT);
        } else {
            set_field(
                components,
                STEW_EFFECTS_COMPONENT,
                encode_stew_effects(effects),
            );
        }
    } else if !effects.is_empty() {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(
                STEW_EFFECTS_COMPONENT.into(),
                encode_stew_effects(effects),
            )]),
        );
    }
}

fn component_bytes(tag: &Tag) -> Result<Vec<u8>, ItemNbtError> {
    let mut bytes = Vec::new();
    mc_nbt::write_network_value(&mut bytes, tag)
        .map_err(|error| ItemNbtError::InvalidComponent(error.to_string()))?;
    Ok(bytes)
}

fn preserve_components(
    items: &ItemRegistry,
    fields: &[(String, Tag)],
    stack: &mut ItemStack,
) -> Result<(), ItemNbtError> {
    let Some(Tag::Compound(original)) = field(fields, "components") else {
        return Ok(());
    };
    if original.is_empty() {
        return Ok(());
    }
    let projected_fields = stack_to_fields(items, stack)?;
    let projected = match field(&projected_fields, "components") {
        Some(Tag::Compound(projected)) => projected.as_slice(),
        _ => &[],
    };
    for (name, value) in original {
        let projected_name = name.strip_prefix('!').unwrap_or(name);
        if name.starts_with('!') {
            let parsed = Identifier::parse(projected_name).ok();
            if parsed
                .as_ref()
                .and_then(crate::data_component_types_26_1_2::id)
                .is_none()
                || !matches!(value, Tag::Compound(fields) if fields.is_empty())
            {
                return Err(ItemNbtError::InvalidComponent(format!(
                    "invalid removed component {name}"
                )));
            }
            if original.iter().any(|(other, _)| other == projected_name) {
                return Err(ItemNbtError::InvalidComponent(format!(
                    "duplicate item component {projected_name}"
                )));
            }
        }
        let baseline = field(projected, projected_name);
        if baseline == Some(value) {
            continue;
        }
        let component = super::item_stack::PreservedItemComponent {
            name: name.clone(),
            value: component_bytes(value)?,
            baseline: baseline.map(component_bytes).transpose()?,
        };
        stack.extended_mut().preserved_components.push(component);
    }
    Ok(())
}

fn restore_components(
    fields: &mut Vec<(String, Tag)>,
    components: &[super::item_stack::PreservedItemComponent],
) -> Result<(), ItemNbtError> {
    for component in components {
        let projected = match field(fields, "components") {
            Some(Tag::Compound(values)) => field(
                values,
                component.name.strip_prefix('!').unwrap_or(&component.name),
            ),
            _ => None,
        };
        if projected.map(component_bytes).transpose()? != component.baseline {
            continue;
        }
        let mut remaining = component.value.as_slice();
        let value = mc_nbt::read_network_value(&mut remaining)
            .map_err(|error| ItemNbtError::InvalidComponent(error.to_string()))?;
        if !remaining.is_empty() {
            return Err(ItemNbtError::InvalidComponent(
                "preserved component contains trailing bytes".into(),
            ));
        }
        set_component_field(fields, &component.name, value);
    }
    Ok(())
}

#[cfg(test)]
#[path = "item_nbt_preservation_tests.rs"]
mod preservation_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ItemRegistry;

    fn items() -> ItemRegistry {
        crate::items::solaris_required_items()
    }

    fn sword(items: &ItemRegistry) -> ItemStack {
        let id = mc_data_item_id(items, "minecraft:diamond_sword");
        ItemStack {
            item_id: id,
            count: 1,
            damage: Some(37),
            enchantments: vec![crate::ItemEnchantment {
                id: Identifier::parse("minecraft:sharpness").expect("enchant"),
                level: 3,
            }],
            custom_name: Some(" quests blade ".into()),
            ..ItemStack::EMPTY
        }
    }

    fn mc_data_item_id(items: &ItemRegistry, name: &str) -> u32 {
        items
            .id_of(&Identifier::parse(name).expect("id"))
            .expect("item")
    }

    #[test]
    fn stack_round_trip_keeps_components() {
        let items = items();
        let stack = sword(&items);
        let fields = stack_to_fields(&items, &stack).expect("encode");
        // Vanilla keys, no JSON smuggling.
        assert!(fields.iter().any(|(key, _)| key == "id"));
        assert!(fields.iter().any(|(key, _)| key == "count"));
        assert!(fields.iter().all(|(key, _)| key != "Inventory"));
        let back = stack_from_fields(&items, &fields).expect("decode");
        assert_eq!(back.as_ref(), Some(&stack));
    }

    #[test]
    fn absent_id_decodes_empty() {
        let items = items();
        assert_eq!(stack_from_fields(&items, &[]).expect("decode"), None);
    }

    #[test]
    fn components_preserve_unknown_keys_and_reject_bad_shapes() {
        use crate::item_stack::{ExtendedItemComponents, FireworkExplosion, Fireworks};
        use mc_nbt::Tag;
        let items = crate::items::solaris_required_items();
        let arrow = mc_data_item_id(&items, "minecraft:arrow");
        let components = ExtendedItemComponents {
            charged_projectiles: vec![ItemStack::new(arrow, 1)],
            fireworks: Some(Fireworks {
                flight_duration: 3,
                explosions: vec![FireworkExplosion {
                    shape: 2,
                    colors: vec![0x123456, 0xabcdef],
                    fade_colors: vec![0x000102],
                    has_trail: true,
                    has_twinkle: true,
                }],
            }),
            ominous_bottle_amplifier: Some(4),
            ..ExtendedItemComponents::EMPTY
        };
        let stack = ItemStack {
            item_id: arrow,
            count: 1,
            components: Some(Box::new(components.clone())),
            ..ItemStack::EMPTY
        };
        let mut fields = stack_to_fields(&items, &stack).expect("encode");
        // Unknown keys inside components survive the round trip.
        if let Some(Tag::Compound(components)) = fields
            .iter_mut()
            .find(|(key, _)| key == "components")
            .map(|(_, tag)| tag)
        {
            components.push(("example:retained".into(), Tag::Int(42)));
        }
        let back = stack_from_fields(&items, &fields)
            .expect("decode")
            .expect("stack");
        let mut expected = components;
        expected.preserved_components = back.extended().preserved_components.clone();
        assert_eq!(back.components.as_deref(), Some(&expected));
        assert_eq!(stack_to_fields(&items, &back).expect("re-encode"), fields);
        // Bad shapes fail instead of becoming empty items.
        assert!(matches!(
            stack_from_fields(
                &items,
                &[
                    ("components".into(), Tag::Int(1)),
                    ("id".into(), Tag::String("minecraft:arrow".into())),
                    ("count".into(), Tag::Int(1))
                ],
            ),
            Err(ItemNbtError::InvalidComponent(_))
        ));
    }
}
