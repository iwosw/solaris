//! UseRemainder is one template; BundleContents is a list of templates.
//! Their persistent codecs also accept the ItemStackTemplate item-name shorthand.
use super::super::{item_stack_template, write_count};
use super::{compound, integer, invalid, items, list, make_list};
use crate::codec::read_bounded_vec;
use crate::error::CodecError;
use bytes::{Buf, BufMut};
use mc_data::{Identifier, ItemStack};
use mc_nbt::Tag;

// Server admission bound, not a limit of vanilla's unbounded list stream codec.
const MAX_BUNDLE_ENTRIES: usize = 64;

pub(super) fn encode<B: BufMut>(buf: &mut B, value: &Tag, depth: usize) -> Result<(), CodecError> {
    if depth >= super::MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    let stack = if let Tag::String(name) = value {
        let item = Identifier::parse(name).map_err(|_| invalid())?;
        ItemStack::new(items().id_of(&item).ok_or_else(invalid)?, 1)
    } else {
        let fields = compound(value)?;
        if let Some(count) = super::field(fields, "count")
            && !(1..=99).contains(&integer(count)?)
        {
            return Err(invalid());
        }
        mc_data::item_nbt::stack_from_fields(items(), fields)
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?
    };
    item_stack_template::encode(buf, &stack, depth + 1)
}

pub(super) fn decode<B: Buf>(buf: &mut B, depth: usize) -> Result<Tag, CodecError> {
    if depth >= super::MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    let stack = item_stack_template::decode(buf, depth + 1)?;
    Ok(Tag::Compound(
        mc_data::item_nbt::stack_to_fields(items(), &stack).map_err(|_| invalid())?,
    ))
}

pub(super) fn encode_list<B: BufMut>(
    buf: &mut B,
    value: &Tag,
    depth: usize,
) -> Result<(), CodecError> {
    if depth >= super::MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    let entries = list(value)?;
    if entries.len() > MAX_BUNDLE_ENTRIES {
        return Err(CodecError::StringTooLong {
            len: entries.len(),
            max: MAX_BUNDLE_ENTRIES,
        });
    }
    write_count(buf, entries.len())?;
    for entry in entries {
        let entry = match entry {
            Tag::Compound(fields) if fields.len() == 1 && fields[0].0.is_empty() => &fields[0].1,
            _ => entry,
        };
        encode(buf, entry, depth)?;
    }
    Ok(())
}

pub(super) fn decode_list<B: Buf>(buf: &mut B, depth: usize) -> Result<Tag, CodecError> {
    if depth >= super::MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    Ok(make_list(read_bounded_vec(
        buf,
        MAX_BUNDLE_ENTRIES,
        4,
        |buf| decode(buf, depth),
    )?))
}
