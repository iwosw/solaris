//! 26.1.2 stream codecs for block-loot components retained by item NBT.
//! Layouts are from DataComponents and each component's javap stream codec.
use super::{read_bounded_vec, write_count};
use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;
use bytes::{Buf, BufMut};
use mc_data::data_component_types_26_1_2::ITEM_COMPONENT_CODECS;
use mc_data::{Identifier, ItemStack};
use mc_nbt::{ListTag, Tag, tag_type};
use std::sync::LazyLock;

mod book;
mod container;
mod profile;
mod templates;

pub(super) const MAX_ITEM_COMPONENT_DEPTH: usize = 8;

pub(super) fn items() -> &'static mc_data::items::ItemRegistry {
    static ITEMS: LazyLock<mc_data::items::ItemRegistry> =
        LazyLock::new(mc_data::items::solaris_required_items);
    &ITEMS
}

fn invalid() -> CodecError {
    CodecError::NotSupported("invalid preserved item component")
}

pub(super) fn contains(components: &[(i32, Tag)], id: i32) -> bool {
    components
        .iter()
        .any(|(candidate, _)| *candidate == id || *candidate == -id - 1)
}

fn typed(id: i32) -> bool {
    matches!(
        id,
        0 | 2 | 3 | 6 | 10 | 13 | 19 | 42 | 49 | 51 | 53 | 56 | 63 | 69 | 72
    )
}

/// Use the same baseline guard as durable item NBT; edited typed values never
/// resurrect an older preserved component. Plain stacks allocate nothing here.
pub(super) fn prepare(stack: &ItemStack) -> Result<Vec<(i32, Tag)>, CodecError> {
    let preserved = &stack.extended().preserved_components;
    if preserved.is_empty() {
        return Ok(Vec::new());
    }
    for (index, component) in preserved.iter().enumerate() {
        let name = component.name.strip_prefix('!').unwrap_or(&component.name);
        if component.name.starts_with('!') {
            if Identifier::parse(name)
                .ok()
                .as_ref()
                .and_then(mc_data::data_component_types_26_1_2::id)
                .is_none()
            {
                return Err(CodecError::NotSupported(
                    "unsupported removed item component",
                ));
            }
        } else if !ITEM_COMPONENT_CODECS
            .iter()
            .any(|(supported, _)| *supported == name)
        {
            return Err(CodecError::NotSupported(
                "unsupported preserved item component",
            ));
        }
        if preserved[..index]
            .iter()
            .any(|old| old.name.trim_start_matches('!') == name)
        {
            return Err(CodecError::NotSupported("duplicate item component"));
        }
    }
    let projected = preserved.iter().any(|component| {
        component.baseline.is_some()
            || ITEM_COMPONENT_CODECS.iter().any(|(name, id)| {
                *name == component.name.strip_prefix('!').unwrap_or(&component.name) && typed(*id)
            })
    });
    let mut values = if projected {
        let fields = mc_data::item_nbt::stack_to_fields(items(), stack).map_err(|_| invalid())?;
        fields
            .into_iter()
            .find_map(|(name, value)| {
                if name == "components" {
                    if let Tag::Compound(values) = value {
                        Some(values)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .unwrap_or_default()
    } else {
        let mut values = Vec::with_capacity(preserved.len());
        for component in preserved {
            let mut remaining = component.value.as_slice();
            let value = mc_nbt::read_network_value(&mut remaining)?;
            if !remaining.is_empty() {
                return Err(CodecError::NotSupported(
                    "preserved component contains trailing bytes",
                ));
            }
            values.push((component.name.clone(), value));
        }
        values
    };
    values.retain(|(name, _)| preserved.iter().any(|component| &component.name == name));
    let mut output = Vec::with_capacity(values.len());
    for (name, value) in values {
        let removed = name.starts_with('!');
        let canonical = name.strip_prefix('!').unwrap_or(&name);
        let id = if removed {
            if !matches!(&value, Tag::Compound(fields) if fields.is_empty()) {
                return Err(CodecError::NotSupported("invalid removed item component"));
            }
            let name = Identifier::parse(canonical).map_err(|_| invalid())?;
            mc_data::data_component_types_26_1_2::id(&name).ok_or_else(invalid)? as i32
        } else {
            ITEM_COMPONENT_CODECS
                .iter()
                .find_map(|(name, id)| (*name == canonical).then_some(*id))
                .ok_or(CodecError::NotSupported(
                    "unsupported preserved item component",
                ))?
        };
        if contains(&output, id) {
            return Err(CodecError::NotSupported("duplicate item component"));
        }
        output.push((if removed { -id - 1 } else { id }, value));
    }
    Ok(output)
}

pub(super) fn encode<B: BufMut>(
    buf: &mut B,
    mut components: Vec<(i32, Tag)>,
    depth: usize,
) -> Result<(), CodecError> {
    // extract_if retains removal markers in place: no second patch allocation
    // or NBT clone is needed to emit the mandated added-then-removed ordering.
    for (id, mut value) in components.extract_if(.., |(id, _)| *id >= 0) {
        if matches!(id, 49 | 51 | 56 | 69) {
            let name = ITEM_COMPONENT_CODECS
                .iter()
                .find(|(_, candidate)| *candidate == id)
                .unwrap()
                .0;
            let c = mc_data::item_stack::ExtendedItemComponents::from_nbt(
                &[(name.into(), value)],
                items(),
            )
            .map_err(CodecError::NotSupported)?;
            if id == 49 && c.charged_projectiles.is_empty() {
                buf.write_varint(id);
                buf.write_varint(0);
            } else if id == 51 && !c.has_potion_contents() {
                buf.write_varint(id);
                buf.write_bool(false);
                buf.write_bool(false);
                buf.write_varint(0);
                buf.write_bool(false);
            } else {
                super::station_components::encode(buf, &c, &[], depth)?;
            }
            continue;
        }
        buf.write_varint(id);
        match id {
            0 => {
                if !matches!(value, Tag::Compound(_)) {
                    return Err(invalid());
                }
                mc_nbt::write_network(buf, &value)?;
            }
            2 | 3 | 19 | 63 => buf.write_varint(integer(&value)?),
            36 => {
                mc_data::item_stack::validate_death_protection(&value)
                    .map_err(CodecError::NotSupported)?;
                // DeathProtection.STREAM_CODEC is ConsumeEffect's list codec.
                buf.write_varint(0);
            }
            6 | 9 => write_text(buf, &value)?,
            10 | 71 => {
                buf.write_identifier(&Identifier::parse(string(&value)?).map_err(|_| invalid())?)?
            }
            11 => {
                let lines = list(&value)?;
                super::write_bounded_count(buf, lines.len(), 256)?;
                for line in lines {
                    write_text(buf, line)?;
                }
            }
            12 => buf.write_varint(match string(&value)? {
                "common" => 0,
                "uncommon" => 1,
                "rare" => 2,
                "epic" => 3,
                _ => return Err(invalid()),
            }),
            13 | 42 => {
                let enchantments = compound(&value)?;
                write_count(buf, enchantments.len())?;
                for (name, level) in enchantments {
                    buf.write_varint(registry_id("enchantment", name)?);
                    buf.write_varint(integer(level)?);
                }
            }
            18 => {
                let fields = compound(&value)?;
                buf.write_bool(
                    field(fields, "hide_tooltip")
                        .map(integer)
                        .transpose()?
                        .unwrap_or(0)
                        != 0,
                );
                let hidden = field(fields, "hidden_components")
                    .map(list)
                    .transpose()?
                    .unwrap_or(&[]);
                write_count(buf, hidden.len())?;
                for component in hidden {
                    buf.write_varint(registry_id("data_component_type", string(component)?)?);
                }
            }
            21 => buf.write_bool(integer(&value)? != 0),
            // DyedItemColor.STREAM_CODEC: ByteBufCodecs.INT, not VAR_INT.
            44 => buf
                .write_i32(mc_data::item_stack::decode_dyed_color(&value).map_err(|_| invalid())?),
            25 => templates::encode(buf, &value, depth)?,
            50 => templates::encode_list(buf, &value, depth)?,
            53 => {
                let effects = list(&value)?;
                write_count(buf, effects.len())?;
                for effect in effects {
                    let fields = compound(effect)?;
                    buf.write_varint(registry_id("mob_effect", string(required(fields, "id")?)?)?);
                    buf.write_varint(
                        field(fields, "duration")
                            .map(integer)
                            .transpose()?
                            .unwrap_or(160),
                    );
                }
            }
            54 | 55 => book::encode(buf, &value, id == 55)?,
            70 => profile::encode(buf, &value)?,
            72 => {
                let layers = list(&value)?;
                super::write_bounded_count(buf, layers.len(), 20)?;
                for layer in layers {
                    let fields = compound(layer)?;
                    buf.write_varint(
                        registry_id("banner_pattern", string(required(fields, "pattern")?)?)? + 1,
                    );
                    buf.write_varint(color(string(required(fields, "color")?)?)?);
                }
            }
            74 => {
                let decorations = list(&value)?;
                if decorations.len() > 4 {
                    return Err(invalid());
                }
                buf.write_varint(4);
                for index in 0..4 {
                    let name = decorations
                        .get(index)
                        .map(string)
                        .transpose()?
                        .unwrap_or("minecraft:brick");
                    buf.write_varint(registry_id("item", name)?);
                }
            }
            75 => container::encode(buf, &value, depth)?,
            76 => {
                let properties = compound(&value)?;
                write_count(buf, properties.len())?;
                for (name, value) in properties {
                    buf.write_string(name, 32767)?;
                    buf.write_string(string(value)?, 32767)?;
                }
            }
            77 => encode_bees(buf, &mut value)?,
            78 => mc_nbt::write_network_value(buf, &value)?,
            79 => {
                let Tag::Compound(fields) = &mut value else {
                    return Err(invalid());
                };
                if matches!(field(fields, "seed"), Some(Tag::Long(0))) {
                    fields.retain(|(name, _)| name != "seed");
                }
                mc_nbt::write_network_value(buf, &value)?;
            }
            _ => {
                return Err(CodecError::NotSupported(
                    "unsupported preserved item component",
                ));
            }
        }
    }
    for (id, _) in components {
        buf.write_varint(-id - 1);
    }
    Ok(())
}

pub(super) fn decode<B: Buf>(
    buf: &mut B,
    id: i32,
    depth: usize,
) -> Result<Option<(String, Tag)>, CodecError> {
    let value = match id {
        54 | 55 => book::decode(buf, id == 55)?,
        6 | 9 => read_text(buf)?,
        11 => make_list(read_bounded_vec(buf, 256, 1, read_text)?),
        12 => Tag::String(
            match buf.read_varint()? {
                0 => "common",
                1 => "uncommon",
                2 => "rare",
                3 => "epic",
                _ => return Err(invalid()),
            }
            .into(),
        ),
        18 => {
            let hide = buf.read_bool()?;
            let hidden = read_bounded_vec(buf, 110, 1, |buf| {
                Ok(Tag::String(registry_name(
                    "data_component_type",
                    buf.read_varint()?,
                )?))
            })?;
            let mut fields = Vec::new();
            if hide {
                fields.push(("hide_tooltip".into(), Tag::Byte(1)));
            }
            if !hidden.is_empty() {
                fields.push(("hidden_components".into(), make_list(hidden)));
            }
            Tag::Compound(fields)
        }
        21 => Tag::Byte(i8::from(buf.read_bool()?)),
        25 => templates::decode(buf, depth)?,
        36 => {
            if buf.read_varint()? != 0 {
                return Err(CodecError::NotSupported(
                    "nonempty custom death_protection death_effects are unsupported",
                ));
            }
            Tag::Compound(vec![("death_effects".into(), make_list(Vec::new()))])
        }
        44 => Tag::Int(buf.read_i32()?),
        50 => templates::decode_list(buf, depth)?,
        70 => profile::decode(buf)?,
        71 => Tag::String(buf.read_identifier()?.as_str().into()),
        72 => make_list(read_bounded_vec(buf, 20, 2, |buf| {
            let raw = buf.read_varint()?.checked_sub(1).ok_or_else(invalid)?;
            let pattern = registry_name("banner_pattern", raw)?;
            let color = color_name(buf.read_varint()?)?;
            Ok(Tag::Compound(vec![
                ("pattern".into(), Tag::String(pattern)),
                ("color".into(), Tag::String(color.into())),
            ]))
        })?),
        74 => make_list(read_bounded_vec(buf, 4, 1, |buf| {
            Ok(Tag::String(registry_name("item", buf.read_varint()?)?))
        })?),
        75 => container::decode(buf, depth)?,
        76 => {
            let entries = read_bounded_vec(buf, 256, 2, |buf| {
                Ok((
                    buf.read_string(32767)?,
                    Tag::String(buf.read_string(32767)?),
                ))
            })?;
            Tag::Compound(entries)
        }
        77 => decode_bees(buf)?,
        78 | 79 => mc_nbt::read_network_value(buf)?,
        _ => return Ok(None),
    };
    let name = ITEM_COMPONENT_CODECS
        .iter()
        .find(|(_, candidate)| *candidate == id)
        .unwrap()
        .0;
    Ok(Some((name.into(), value)))
}

pub(super) fn finish(
    mut stack: ItemStack,
    mut fields: Vec<(String, Tag)>,
) -> Result<ItemStack, CodecError> {
    if fields.is_empty() {
        return Ok(stack);
    }
    let has_name = field(&fields, "minecraft:custom_name").is_some();
    let has_banner = field(&fields, "minecraft:banner_patterns").is_some();
    // Compact literal text is a wire representation, not an extra durable override.
    if let Some((_, value)) = fields
        .iter_mut()
        .find(|(name, _)| name == "minecraft:custom_name")
        && let Tag::String(text) = value
    {
        let text = std::mem::take(text);
        *value = Tag::Compound(vec![("text".into(), Tag::String(text))]);
    }
    // Only project components here; the enclosing registry owns item identity.
    let mut parsed = mc_data::item_nbt::stack_from_fields(
        items(),
        &[
            ("id".into(), Tag::String("minecraft:stone".into())),
            ("count".into(), Tag::Int(stack.count)),
            ("components".into(), Tag::Compound(fields)),
        ],
    )
    .map_err(|_| invalid())?
    .ok_or_else(invalid)?;
    if has_name {
        stack.custom_name = parsed.custom_name.take();
    }
    if let Some(mut extra) = parsed.components.take() {
        if has_banner {
            stack.extended_mut().banner_patterns = std::mem::take(&mut extra.banner_patterns);
        }
        if !extra.preserved_components.is_empty() {
            stack
                .extended_mut()
                .preserved_components
                .extend(extra.preserved_components);
        }
    }
    Ok(stack)
}

fn encode_bees<B: BufMut>(buf: &mut B, value: &mut Tag) -> Result<(), CodecError> {
    let Tag::List(bees) = value else {
        return Err(invalid());
    };
    super::write_bounded_count(buf, bees.elements.len(), 1024)?;
    for occupant in &mut bees.elements {
        let Tag::Compound(fields) = occupant else {
            return Err(invalid());
        };
        let ticks = integer(required(fields, "ticks_in_hive")?)?;
        let minimum = integer(required(fields, "min_ticks_in_hive")?)?;
        let (_, entity) = fields
            .iter_mut()
            .find(|(name, _)| name == "entity_data")
            .ok_or_else(invalid)?;
        let Tag::Compound(data) = entity else {
            return Err(invalid());
        };
        let index = data
            .iter()
            .position(|(name, _)| name == "id")
            .ok_or_else(invalid)?;
        let (_, id) = data.remove(index);
        buf.write_varint(registry_id("entity_type", string(&id)?)?);
        mc_nbt::write_network(buf, entity)?;
        buf.write_varint(ticks);
        buf.write_varint(minimum);
    }
    Ok(())
}

fn decode_bees<B: Buf>(buf: &mut B) -> Result<Tag, CodecError> {
    Ok(make_list(read_bounded_vec(buf, 1024, 5, |buf| {
        let id = registry_name("entity_type", buf.read_varint()?)?;
        let Tag::Compound(mut entity) = mc_nbt::read_network(buf)? else {
            return Err(invalid());
        };
        entity.retain(|(name, _)| name != "id");
        entity.insert(0, ("id".into(), Tag::String(id)));
        Ok(Tag::Compound(vec![
            ("entity_data".into(), Tag::Compound(entity)),
            ("ticks_in_hive".into(), Tag::Int(buf.read_varint()?)),
            ("min_ticks_in_hive".into(), Tag::Int(buf.read_varint()?)),
        ]))
    })?))
}

pub(super) fn compound(value: &Tag) -> Result<&[(String, Tag)], CodecError> {
    if let Tag::Compound(fields) = value {
        Ok(fields)
    } else {
        Err(invalid())
    }
}
pub(super) fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}
pub(super) fn required<'a>(fields: &'a [(String, Tag)], name: &str) -> Result<&'a Tag, CodecError> {
    field(fields, name).ok_or_else(invalid)
}
pub(super) fn string(value: &Tag) -> Result<&str, CodecError> {
    if let Tag::String(value) = value {
        Ok(value)
    } else {
        Err(invalid())
    }
}
pub(super) fn integer(value: &Tag) -> Result<i32, CodecError> {
    match value {
        Tag::Byte(value) => Ok(i32::from(*value)),
        Tag::Short(value) => Ok(i32::from(*value)),
        Tag::Int(value) => Ok(*value),
        _ => Err(invalid()),
    }
}
pub(super) fn list(value: &Tag) -> Result<&[Tag], CodecError> {
    if let Tag::List(list) = value {
        Ok(&list.elements)
    } else {
        Err(invalid())
    }
}
pub(super) fn make_list(mut elements: Vec<Tag>) -> Tag {
    let mut element_type = elements.first().map(Tag::type_id).unwrap_or(tag_type::END);
    if elements.iter().any(|value| value.type_id() != element_type) {
        element_type = tag_type::COMPOUND;
    }
    // ListTag.write in 26.1.2 uses a singleton empty-key compound for
    // heterogeneous entries, escaping compounds which already have that shape.
    if element_type == tag_type::COMPOUND {
        for value in &mut elements {
            let needs_wrapper = match value {
                Tag::Compound(fields) => fields.len() == 1 && fields[0].0.is_empty(),
                _ => true,
            };
            if needs_wrapper {
                let original = std::mem::replace(value, Tag::Compound(Vec::new()));
                *value = Tag::Compound(vec![(String::new(), original)]);
            }
        }
    }
    Tag::List(ListTag {
        element_type,
        elements,
    })
}
pub(super) fn registry_id(registry: &str, name: &str) -> Result<i32, CodecError> {
    let name = Identifier::parse(name).map_err(|_| invalid())?;
    let id = match registry {
        "data_component_type" => mc_data::data_component_types_26_1_2::id(&name),
        "item" => items().id_of(&name),
        "entity_type" => mc_data::entity_types::entity_type_contract_26_1_2_by_name(name.as_str())
            .map(|entry| entry.protocol_id),
        "mob_effect" => mc_data::mob_effects_26_1_2::MobEffect::from_name(name.as_str())
            .map(|effect| effect as u32),
        _ => mc_data::required_registry_entry_id(registry, &name),
    };
    id.map(|id| id as i32).ok_or_else(invalid)
}
pub(super) fn registry_name(registry: &str, id: i32) -> Result<String, CodecError> {
    let id = u32::try_from(id).map_err(|_| invalid())?;
    let name = match registry {
        "data_component_type" => mc_data::data_component_types_26_1_2::name(id).map(str::to_owned),
        "item" => items().name_of(id).map(|name| name.as_str().to_owned()),
        "entity_type" => mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(id)
            .map(|entry| entry.name.to_owned()),
        "mob_effect" => mc_data::mob_effects_26_1_2::name_for_id(id).map(str::to_owned),
        _ => mc_data::required_registry_entry(registry, id).map(|name| name.as_str().to_owned()),
    };
    name.ok_or_else(invalid)
}
fn write_text<B: BufMut>(buf: &mut B, value: &Tag) -> Result<(), CodecError> {
    let value = match value {
        Tag::Compound(fields) if fields.len() == 1 && fields[0].0.is_empty() => &fields[0].1,
        _ => value,
    };
    if let Tag::Compound(fields) = value
        && fields.len() == 1
        && let Some(Tag::String(text)) = field(fields, "text")
    {
        mc_nbt::write_network_value(buf, &Tag::String(text.clone()))?;
        return Ok(());
    }
    if !matches!(value, Tag::String(_) | Tag::Compound(_) | Tag::List(_)) {
        return Err(invalid());
    }
    mc_nbt::write_network_value(buf, value)?;
    Ok(())
}
fn read_text<B: Buf>(buf: &mut B) -> Result<Tag, CodecError> {
    let value = mc_nbt::read_network_value(buf)?;
    if matches!(value, Tag::String(_) | Tag::Compound(_) | Tag::List(_)) {
        Ok(value)
    } else {
        Err(invalid())
    }
}
const COLORS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];
fn color(name: &str) -> Result<i32, CodecError> {
    COLORS
        .iter()
        .position(|candidate| *candidate == name)
        .map(|index| index as i32)
        .ok_or_else(invalid)
}
fn color_name(id: i32) -> Result<&'static str, CodecError> {
    usize::try_from(id)
        .ok()
        .and_then(|id| COLORS.get(id).copied())
        .ok_or_else(invalid)
}

#[cfg(test)]
#[path = "copied_components_tests.rs"]
mod tests;
