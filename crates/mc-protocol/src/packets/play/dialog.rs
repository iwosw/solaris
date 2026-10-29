//! Server-driven dialogs (26.1.2). Layouts from the local decompiled client
//! `net.minecraft.network.protocol.common.{ClientboundShowDialogPacket,
//! ClientboundClearDialogPacket,ServerboundCustomClickActionPacket}`; IDs from
//! `data/vanilla/reports/packets.json` (play: clear_dialog 139, show_dialog 140,
//! serverbound custom_click_action 68).
//!
//! `show_dialog` is `ByteBufCodecs.holder(Registries.DIALOG, fromCodecWithRegistriesTrusted(..))`:
//! VarInt `0` for a direct (inline) dialog followed by its anonymous network NBT
//! (the `Dialog.DIRECT_CODEC` result); `id + 1` selects a registry entry, which
//! Solaris never sends and rejects on decode.
use bytes::{Buf, BufMut};
use mc_nbt::Tag;

use crate::codec::{Identifier, ReadMc, WriteMc};
use crate::error::CodecError;
use crate::packets::Packet;

/// Vanilla `ServerboundCustomClickActionPacket.UNTRUSTED_TAG_CODEC` byte ceiling
/// (`ByteBufCodecs.lengthPrefixed(65536)`).
const MAX_CLICK_PAYLOAD_BYTES: usize = 65536;
/// `new NbtAccounter(32768L, 16)`: heap quota and nesting depth of the payload.
const MAX_CLICK_PAYLOAD_QUOTA: usize = 32768;
const MAX_CLICK_PAYLOAD_DEPTH: usize = 16;

/// Inline dialog; `dialog` is the `Dialog.DIRECT_CODEC` NBT compound
/// (`{type:"minecraft:multi_action", title:…, …}`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundShowDialog {
    pub dialog: Tag,
}

impl Packet for ClientboundShowDialog {
    const ID: i32 = 0x8F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        let mut body = Vec::new();
        mc_nbt::write_network_value(&mut body, &self.dialog)?;
        buf.write_varint(0);
        buf.put_slice(&body);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        match buf.read_varint()? {
            0 => Ok(Self {
                dialog: mc_nbt::read_network_value(buf)?,
            }),
            _ => Err(CodecError::NotSupported(
                "registry dialog reference (Solaris sends inline dialogs only)",
            )),
        }
    }
}

/// Closes the dialog screen the client is showing; no fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundClearDialog;

impl Packet for ClientboundClearDialog {
    const ID: i32 = 0x8E;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

/// Result of a `custom` dialog action: the action id plus (when the action
/// carries any) an NBT payload. Dynamic `dynamic/custom` actions deliver the
/// `additions` compound extended with one entry per dialog input.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerboundCustomClickAction {
    pub id: Identifier,
    pub payload: Option<Tag>,
}

impl Packet for ServerboundCustomClickAction {
    const ID: i32 = 0x44;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        let mut body = Vec::new();
        match &self.payload {
            // FriendlyByteBuf.writeNbt(null) writes TAG_End.
            None => body.push(0),
            Some(tag) => mc_nbt::write_network_value(&mut body, tag)?,
        }
        if body.len() > MAX_CLICK_PAYLOAD_BYTES {
            return Err(CodecError::ArrayTooLong {
                len: body.len(),
                max: MAX_CLICK_PAYLOAD_BYTES,
            });
        }
        buf.write_identifier(&self.id)?;
        buf.write_varint(body.len() as i32);
        buf.put_slice(&body);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let id = buf.read_identifier()?;
        let len = buf.read_varint()?;
        let len = usize::try_from(len).map_err(|_| CodecError::NegativeLength(len))?;
        if len > MAX_CLICK_PAYLOAD_BYTES {
            return Err(CodecError::ArrayTooLong {
                len,
                max: MAX_CLICK_PAYLOAD_BYTES,
            });
        }
        if buf.remaining() < len {
            return Err(CodecError::Underflow {
                needed: len - buf.remaining(),
                available: buf.remaining(),
            });
        }
        let mut slice = buf.copy_to_bytes(len);
        let payload = if slice.first() == Some(&0) {
            None
        } else {
            if len > MAX_CLICK_PAYLOAD_QUOTA {
                return Err(CodecError::NotSupported(
                    "custom click payload exceeds the 32768-byte NBT quota",
                ));
            }
            let tag = mc_nbt::read_network_value(&mut slice)?;
            if nbt_depth(&tag) > MAX_CLICK_PAYLOAD_DEPTH {
                return Err(CodecError::NotSupported(
                    "custom click payload exceeds NBT depth 16",
                ));
            }
            Some(tag)
        };
        Ok(Self { id, payload })
    }
}

/// Nesting depth in `NbtAccounter.pushDepth` terms: scalars cost nothing, each
/// compound/list level costs one.
fn nbt_depth(tag: &Tag) -> usize {
    match tag {
        Tag::Compound(entries) => 1 + entries.iter().map(|(_, t)| nbt_depth(t)).max().unwrap_or(0),
        Tag::List(list) => 1 + list.elements.iter().map(nbt_depth).max().unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod tests;
