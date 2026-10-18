//! ItemContainerContents: list(256) of optional ItemStackTemplate.
//! A present template is boolean, item id, count, then DataComponentPatch;
//! unlike an ordinary slot, its count does not precede the item id.
use super::super::item_stack_template;
use super::{compound, integer, invalid, items, list, make_list, required};
use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;
use bytes::{Buf, BufMut};
use mc_nbt::Tag;

pub(super) fn encode<B: BufMut>(buf: &mut B, value: &Tag, depth: usize) -> Result<(), CodecError> {
    if depth >= super::MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    let entries = list(value)?;
    if entries.len() > 256 {
        return Err(invalid());
    }
    let mut slots: [Option<&Tag>; 256] = [None; 256];
    let mut length = 0;
    for entry in entries {
        let fields = compound(entry)?;
        let slot = usize::try_from(integer(required(fields, "slot")?)?).map_err(|_| invalid())?;
        if slot >= slots.len() {
            return Err(invalid());
        }
        slots[slot] = Some(required(fields, "item")?);
        length = length.max(slot + 1);
    }
    super::super::write_count(buf, length)?;
    for item in &slots[..length] {
        let Some(item) = item else {
            buf.write_bool(false);
            continue;
        };
        let fields = compound(item)?;
        let stack = mc_data::item_nbt::stack_from_fields(items(), fields)
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;
        if !(1..=99).contains(&stack.count) {
            return Err(invalid());
        }
        buf.write_bool(true);
        item_stack_template::encode(buf, &stack, depth + 1)?;
    }
    Ok(())
}

pub(super) fn decode<B: Buf>(buf: &mut B, depth: usize) -> Result<Tag, CodecError> {
    if depth >= super::MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    let length = crate::codec::read_bounded_count(buf, 256, 1)?;
    let mut entries = Vec::new();
    for slot in 0..length {
        if !buf.read_bool()? {
            continue;
        }
        let stack = item_stack_template::decode(buf, depth + 1)?;
        let fields = mc_data::item_nbt::stack_to_fields(items(), &stack).map_err(|_| invalid())?;
        entries.push(Tag::Compound(vec![
            ("slot".into(), Tag::Int(slot as i32)),
            ("item".into(), Tag::Compound(fields)),
        ]));
    }
    Ok(make_list(entries))
}
