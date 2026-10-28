//! Command admission for the native component values supported by item NBT.
//! Typed values still go through stack_from_fields/ExtendedItemComponents;
//! this rejects malformed opaque values before inventory/wire publication.
use super::item_nbt::field;
use crate::data_component_types_26_1_2 as ids;
use crate::{Identifier, items::ItemRegistry};
use mc_nbt::Tag;

pub(crate) fn validate(
    items: &ItemRegistry,
    components: &[(String, Tag)],
    depth: usize,
) -> Result<(), String> {
    if depth > 8 {
        return Err("nested item component depth".into());
    }
    for (index, (name, value)) in components.iter().enumerate() {
        let canonical = name.strip_prefix('!').unwrap_or(name);
        if components[..index]
            .iter()
            .any(|(other, _)| other.strip_prefix('!').unwrap_or(other) == canonical)
        {
            return Err(format!("duplicate item component {canonical}"));
        }
        let id = ids::ITEM_COMPONENT_CODECS
            .iter()
            .find_map(|(name, id)| (*name == canonical).then_some(*id))
            .ok_or_else(|| format!("unsupported item component {canonical}"))?;
        let result = if name.starts_with('!') {
            if matches!(value, Tag::Compound(fields) if fields.is_empty()) {
                Ok(())
            } else {
                Err("removed component must be an empty compound")
            }
        } else {
            value_shape(items, id, value, depth)
        };
        result.map_err(|reason| format!("invalid item component {canonical}: {reason}"))?;
    }
    Ok(())
}
fn integer(value: &Tag) -> Result<i32, &'static str> {
    match value {
        Tag::Byte(value) => Ok(i32::from(*value)),
        Tag::Short(value) => Ok(i32::from(*value)),
        Tag::Int(value) => Ok(*value),
        _ => Err("expected integer"),
    }
}
fn string(value: &Tag) -> Result<&str, &'static str> {
    match value {
        Tag::String(value) => Ok(value),
        _ => Err("expected string"),
    }
}
fn compound(value: &Tag) -> Result<&[(String, Tag)], &'static str> {
    match value {
        Tag::Compound(fields) => Ok(fields),
        _ => Err("expected compound"),
    }
}
fn list(value: &Tag, max: usize) -> Result<&[Tag], &'static str> {
    match value {
        Tag::List(list) if list.elements.len() <= max => Ok(&list.elements),
        Tag::List(_) => Err("too many entries"),
        _ => Err("expected list"),
    }
}
fn required<'a>(fields: &'a [(String, Tag)], name: &str) -> Result<&'a Tag, &'static str> {
    field(fields, name).ok_or("missing field")
}
fn identifier(value: &Tag) -> Result<Identifier, &'static str> {
    Identifier::parse(string(value)?).map_err(|_| "invalid identifier")
}
fn registry(registry: &str, value: &Tag) -> Result<(), &'static str> {
    registry_name(registry, string(value)?)
}
fn registry_name(registry: &str, name: &str) -> Result<(), &'static str> {
    let id = Identifier::parse(name).map_err(|_| "invalid identifier")?;
    let known = match registry {
        "data_component_type" => ids::id(&id).is_some(),
        "entity_type" => {
            crate::entity_types::entity_type_contract_26_1_2_by_name(id.as_str()).is_some()
        }
        "mob_effect" => crate::mob_effects_26_1_2::MobEffect::from_name(id.as_str()).is_some(),
        _ => crate::required_registry_entry_id(registry, &id).is_some(),
    };
    if known {
        Ok(())
    } else {
        Err("unknown registry entry")
    }
}
fn text(value: &Tag) -> Result<(), &'static str> {
    match value {
        Tag::String(_) => Ok(()),
        Tag::List(_) => {
            for entry in list(value, 256)? {
                text(entry)?;
            }
            Ok(())
        }
        Tag::Compound(fields) if fields.len() == 1 && fields[0].0.is_empty() => text(&fields[0].1),
        Tag::Compound(fields) => {
            let content = ["text", "translate", "selector", "keybind", "score", "nbt"]
                .iter()
                .find_map(|name| field(fields, name))
                .ok_or("missing text component content")?;
            if !matches!(content, Tag::String(_) | Tag::Compound(_)) {
                return Err("invalid text component content");
            }
            for name in [
                "bold",
                "italic",
                "underlined",
                "strikethrough",
                "obfuscated",
            ] {
                if let Some(value) = field(fields, name)
                    && !matches!(integer(value)?, 0 | 1)
                {
                    return Err("invalid text style boolean");
                }
            }
            if let Some(extra) = field(fields, "extra") {
                for value in list(extra, 256)? {
                    text(value)?;
                }
            }
            Ok(())
        }
        _ => Err("expected text component"),
    }
}
fn nested(
    items: &ItemRegistry,
    fields: &[(String, Tag)],
    depth: usize,
) -> Result<(), &'static str> {
    if depth >= 8 {
        return Err("nested item component depth");
    }
    if let Some(components) = field(fields, "components") {
        validate(items, compound(components)?, depth + 1)
            .map_err(|_| "invalid nested component")?;
    }
    let stack = super::item_nbt::stack_from_fields(items, fields)
        .map_err(|_| "invalid nested item")?
        .ok_or("missing nested item")?;
    if stack.item_id == 0 {
        return Err("empty nested item");
    }
    if !(1..=99).contains(&stack.count) {
        return Err("invalid nested item count");
    }
    Ok(())
}

fn template(items: &ItemRegistry, value: &Tag, depth: usize) -> Result<(), &'static str> {
    if depth >= 8 {
        return Err("nested item component depth");
    }
    if matches!(value, Tag::String(_)) {
        let id = items
            .id_of(&identifier(value)?)
            .ok_or("unknown nested item")?;
        if id == 0 {
            return Err("empty nested item");
        }
        Ok(())
    } else {
        nested(items, compound(value)?, depth)
    }
}
fn profile(value: &Tag) -> Result<(), &'static str> {
    if let Tag::String(name) = value {
        return if name.chars().count() <= 16 {
            Ok(())
        } else {
            Err("profile name too long")
        };
    }
    let fields = compound(value)?;
    if let Some(name) = field(fields, "name")
        && string(name)?.chars().count() > 16
    {
        return Err("profile name too long");
    }
    if let Some(id) = field(fields, "id") {
        match id {
            Tag::IntArray(parts) if parts.len() == 4 => {}
            Tag::String(id) if uuid::Uuid::parse_str(id).is_ok() => {}
            _ => return Err("invalid profile UUID"),
        }
    }
    let mut count = 0;
    if let Some(properties) = field(fields, "properties") {
        match properties {
            Tag::List(_) => {
                for property in list(properties, 16)? {
                    let property = compound(property)?;
                    property_value(string(required(property, "name")?)?, property)?;
                    count += 1;
                }
            }
            Tag::Compound(groups) => {
                for (name, entries) in groups {
                    for property in list(entries, 16)? {
                        property_value(name, compound(property)?)?;
                        count += 1;
                    }
                }
            }
            _ => return Err("invalid profile properties"),
        }
    }
    if count > 16 {
        return Err("too many profile properties");
    }
    for name in ["texture", "cape", "elytra"] {
        if let Some(value) = field(fields, name) {
            identifier(value)?;
        }
    }
    if let Some(model) = field(fields, "model")
        && !matches!(string(model)?, "slim" | "wide")
    {
        return Err("invalid profile model");
    }
    Ok(())
}
fn property_value(name: &str, fields: &[(String, Tag)]) -> Result<(), &'static str> {
    if name.chars().count() > 64 || string(required(fields, "value")?)?.chars().count() > 32767 {
        return Err("profile property too long");
    }
    if let Some(signature) = field(fields, "signature")
        && string(signature)?.chars().count() > 1024
    {
        return Err("profile signature too long");
    }
    Ok(())
}
fn value_shape(
    items: &ItemRegistry,
    id: i32,
    value: &Tag,
    depth: usize,
) -> Result<(), &'static str> {
    match id {
        ids::CUSTOM_DATA => {
            compound(value)?;
        }
        ids::MAX_DAMAGE => crate::item_stack::validate_max_damage(
            u32::try_from(integer(value)?).map_err(|_| "invalid max_damage")?,
        )?,
        ids::DAMAGE | ids::REPAIR_COST => {
            if integer(value)? < 0 {
                return Err("negative integer");
            }
        }
        ids::OMINOUS_BOTTLE_AMPLIFIER => {
            if !(0..=4).contains(&integer(value)?) {
                return Err("invalid ominous amplifier");
            }
        }
        ids::CUSTOM_NAME | ids::ITEM_NAME => text(value)?,
        ids::ITEM_MODEL | ids::NOTE_BLOCK_SOUND => {
            identifier(value)?;
        }
        ids::LORE => {
            for line in list(value, 256)? {
                text(line)?;
            }
        }
        ids::RARITY => {
            if !matches!(string(value)?, "common" | "uncommon" | "rare" | "epic") {
                return Err("invalid rarity");
            }
        }
        ids::ENCHANTMENTS | ids::STORED_ENCHANTMENTS => {
            for (name, level) in compound(value)? {
                registry_name("enchantment", name)?;
                if !(1..=255).contains(&integer(level)?) {
                    return Err("invalid enchantment level");
                }
            }
        }
        ids::TOOLTIP_DISPLAY => {
            let fields = compound(value)?;
            if let Some(value) = field(fields, "hide_tooltip")
                && !matches!(integer(value)?, 0 | 1)
            {
                return Err("invalid tooltip boolean");
            }
            if let Some(hidden) = field(fields, "hidden_components") {
                for value in list(hidden, ids::COUNT)? {
                    registry("data_component_type", value)?;
                }
            }
        }
        ids::ENCHANTMENT_GLINT_OVERRIDE => {
            if !matches!(integer(value)?, 0 | 1) {
                return Err("invalid glint boolean");
            }
        }
        ids::USE_REMAINDER => template(items, value, depth)?,
        ids::DEATH_PROTECTION => crate::item_stack::validate_death_protection(value)?,
        ids::DYED_COLOR => {
            crate::item_stack::decode_dyed_color(value)?;
        }
        ids::BUNDLE_CONTENTS => {
            for value in list(value, i32::MAX as usize)? {
                template(items, value, depth)?;
            }
        }
        ids::CHARGED_PROJECTILES => {
            for value in list(value, 3)? {
                nested(items, compound(value)?, depth)?;
            }
        }
        ids::POTION_CONTENTS => {
            let fields = compound(value)?;
            if let Some(potion) = field(fields, "potion")
                && crate::stations::potion_id(&identifier(potion)?).is_none()
            {
                return Err("unknown potion");
            }
        }
        ids::SUSPICIOUS_STEW_EFFECTS => {
            for effect in list(value, 256)? {
                let fields = compound(effect)?;
                registry("mob_effect", required(fields, "id")?)?;
                if let Some(duration) = field(fields, "duration") {
                    integer(duration)?;
                }
            }
        }
        ids::TRIM | ids::FIREWORKS => {
            compound(value)?;
        }
        ids::PROFILE => profile(value)?,
        ids::BANNER_PATTERNS => {
            list(value, 20)?;
        }
        ids::POT_DECORATIONS => {
            for decoration in list(value, 4)? {
                if items.id_of(&identifier(decoration)?).is_none() {
                    return Err("unknown decoration item");
                }
            }
        }
        ids::CONTAINER => {
            let mut seen = [false; 256];
            for entry in list(value, 256)? {
                let fields = compound(entry)?;
                let slot = usize::try_from(integer(required(fields, "slot")?)?)
                    .map_err(|_| "invalid container slot")?;
                if slot >= 256 || seen[slot] {
                    return Err("invalid or duplicate container slot");
                }
                seen[slot] = true;
                nested(items, compound(required(fields, "item")?)?, depth)?;
            }
        }
        ids::BLOCK_STATE => {
            for (_, property) in compound(value)? {
                if string(property)?.chars().count() > 32767 {
                    return Err("block state value too long");
                }
            }
        }
        ids::BEES => {
            for occupant in list(value, 1024)? {
                let fields = compound(occupant)?;
                integer(required(fields, "ticks_in_hive")?)?;
                integer(required(fields, "min_ticks_in_hive")?)?;
                let data = compound(required(fields, "entity_data")?)?;
                registry("entity_type", required(data, "id")?)?;
            }
        }
        ids::LOCK | ids::CONTAINER_LOOT => {
            compound(value)?;
        }
        _ => return Err("unsupported component"),
    }
    Ok(())
}
