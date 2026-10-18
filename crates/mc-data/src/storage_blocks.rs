//! Exact native crafter, shelf and shulker NBT; slots retain item components.
use crate::item_nbt::{field, stack_from_fields, stack_to_fields};
use crate::{ItemStack, items::ItemRegistry};
use mc_nbt::{ListTag, Tag, tag_type};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CrafterSettings {
    pub disabled_slots: u16,
    pub crafting_ticks_remaining: i32,
    pub triggered: bool,
}

fn one<'a>(fields: &'a [(String, Tag)], name: &str) -> Result<Option<&'a Tag>, String> {
    let mut found = fields.iter().filter(|(key, _)| key == name);
    let value = found.next().map(|(_, value)| value);
    if found.next().is_some() {
        return Err(format!("{name}: duplicate field"));
    }
    Ok(value)
}
fn integer(fields: &[(String, Tag)], name: &str, maximum: i32) -> Result<i32, String> {
    match one(fields, name)? {
        None => Ok(0),
        Some(Tag::Int(value)) if (0..=maximum).contains(value) => Ok(*value),
        _ => Err(format!("{name}: expected int in 0..={maximum}")),
    }
}
fn boolean(fields: &[(String, Tag)], name: &str) -> Result<bool, String> {
    match one(fields, name)? {
        None | Some(Tag::Byte(0)) => Ok(false),
        Some(Tag::Byte(1)) => Ok(true),
        _ => Err(format!("{name}: expected boolean byte")),
    }
}
pub fn decode_slots(
    fields: &[(String, Tag)],
    items: &ItemRegistry,
    size: usize,
) -> Result<Vec<ItemStack>, String> {
    let mut slots = vec![ItemStack::EMPTY; size];
    let Some(tag) = one(fields, "Items")? else {
        return Ok(slots);
    };
    let Tag::List(list) = tag else {
        return Err("Items: expected compound list".into());
    };
    if list.element_type != tag_type::COMPOUND && !list.elements.is_empty() {
        return Err("Items: expected compound list".into());
    }
    let mut seen = 0u32;
    for entry in &list.elements {
        let Tag::Compound(entry) = entry else {
            return Err("Items[]: expected compound".into());
        };
        let Some(Tag::Byte(index)) = one(entry, "Slot")? else {
            return Err("Items[].Slot: expected byte".into());
        };
        let index = usize::try_from(*index)
            .map_err(|_| format!("Items[].Slot: expected 0..{}", size - 1))?;
        if index >= size {
            return Err(format!("Items[].Slot: expected 0..{}", size - 1));
        }
        if seen & (1 << index) != 0 {
            return Err(format!("Items[].Slot: duplicate slot {index}"));
        }
        seen |= 1 << index;
        slots[index] = stack_from_fields(items, entry)
            .map_err(|e| format!("Items[{index}]: {e}"))?
            .ok_or_else(|| format!("Items[{index}]: missing item id"))?;
    }
    Ok(slots)
}
pub fn decode_crafter(
    fields: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<(CrafterSettings, Vec<ItemStack>), String> {
    let slots = decode_slots(fields, items, 9)?;
    let mut disabled_slots = 0u16;
    if let Some(tag) = one(fields, "disabled_slots")? {
        let Tag::IntArray(indices) = tag else {
            return Err("disabled_slots: expected int array".into());
        };
        for index in indices {
            if !(0..9).contains(index) {
                return Err("disabled_slots: expected slot in 0..=8".into());
            }
            let bit = 1 << *index;
            if disabled_slots & bit != 0 {
                return Err(format!("disabled_slots: duplicate slot {index}"));
            }
            if !slots[*index as usize].is_empty() {
                return Err(format!("disabled_slots: occupied slot {index}"));
            }
            disabled_slots |= bit;
        }
    }
    Ok((
        CrafterSettings {
            disabled_slots,
            crafting_ticks_remaining: integer(fields, "crafting_ticks_remaining", 6)?,
            triggered: integer(fields, "triggered", 1)? == 1,
        },
        slots,
    ))
}
pub fn decode_shelf(
    fields: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<(bool, Vec<ItemStack>), String> {
    Ok((
        boolean(fields, "align_items_to_bottom")?,
        decode_slots(fields, items, 3)?,
    ))
}
pub fn encode_items(fields: &mut Vec<(String, Tag)>, slots: &[ItemStack], items: &ItemRegistry) {
    fields.retain(|(key, _)| key != "Items");
    fields.push((
        "Items".into(),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: slots
                .iter()
                .enumerate()
                .filter(|(_, stack)| !stack.is_empty())
                .map(|(slot, stack)| {
                    let mut entry = stack_to_fields(items, stack).expect("registered storage item");
                    entry.push(("Slot".into(), Tag::Byte(slot as i8)));
                    Tag::Compound(entry)
                })
                .collect(),
        }),
    ));
}
pub fn encode_crafter_fields(fields: &mut Vec<(String, Tag)>, settings: CrafterSettings) {
    fields.retain(|(key, _)| {
        !matches!(
            key.as_str(),
            "disabled_slots" | "crafting_ticks_remaining" | "triggered"
        )
    });
    fields.extend([
        (
            "disabled_slots".into(),
            Tag::IntArray(
                (0..9)
                    .filter(|i| settings.disabled_slots & (1 << i) != 0)
                    .collect(),
            ),
        ),
        (
            "crafting_ticks_remaining".into(),
            Tag::Int(settings.crafting_ticks_remaining),
        ),
        ("triggered".into(), Tag::Int(i32::from(settings.triggered))),
    ]);
}
/// Structural validation runs before opaque records become live custody.
pub fn validate(fields: &[(String, Tag)], items: &ItemRegistry) -> Result<(), String> {
    match field(fields, "id") {
        Some(Tag::String(id)) if id == "minecraft:crafter" => {
            decode_crafter(fields, items).map(|_| ())
        }
        Some(Tag::String(id)) if id == "minecraft:shelf" => decode_shelf(fields, items).map(|_| ()),
        Some(Tag::String(id)) if id == "minecraft:shulker_box" => {
            decode_slots(fields, items, 27).map(|_| ())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "storage_blocks_tests.rs"]
mod tests;
