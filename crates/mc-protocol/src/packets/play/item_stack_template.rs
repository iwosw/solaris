//! ItemStackTemplate.STREAM_CODEC in vanilla 26.1.2: item holder, count, patch.
//! Item.STREAM_CODEC uses the raw item registry id (ByteBufCodecs$29), not id + 1.
use super::{ItemStackWireCodec, copied_components::MAX_ITEM_COMPONENT_DEPTH};
use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;
use bytes::{Buf, BufMut};
use mc_data::ItemStack;

fn validate(item_id: u32, count: i32) -> Result<(), CodecError> {
    if item_id == 0 || count <= 0 {
        return Err(CodecError::NotSupported(
            "item stack template must be non-empty",
        ));
    }
    if count > 99 {
        return Err(CodecError::NotSupported(
            "invalid item stack template count",
        ));
    }
    Ok(())
}

pub(super) fn encode<B: BufMut>(
    buf: &mut B,
    stack: &ItemStack,
    depth: usize,
) -> Result<(), CodecError> {
    if depth > MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    validate(stack.item_id, stack.count)?;
    buf.write_varint(stack.item_id as i32);
    buf.write_varint(stack.count);
    stack.encode_components_at_depth(buf, depth)
}

pub(super) fn decode<B: Buf>(buf: &mut B, depth: usize) -> Result<ItemStack, CodecError> {
    if depth > MAX_ITEM_COMPONENT_DEPTH {
        return Err(CodecError::NotSupported("nested item component depth"));
    }
    let item_id = u32::try_from(buf.read_varint()?)
        .map_err(|_| CodecError::NotSupported("negative item stack template item id"))?;
    if super::copied_components::items().name_of(item_id).is_none() {
        return Err(CodecError::NotSupported(
            "unknown item stack template item id",
        ));
    }
    let count = buf.read_varint()?;
    validate(item_id, count)?;
    ItemStack::decode_components_at_depth(buf, item_id, count, depth)
}
