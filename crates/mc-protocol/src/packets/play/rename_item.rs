use bytes::{Buf, BufMut};

use crate::{
    codec::{ReadMc, WriteMc},
    error::CodecError,
    packets::Packet,
};

/// Local 26.1.2 `GameProtocols` serverbound registration index 48;
/// `ServerboundRenameItemPacket` reads and writes one UTF string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundRenameItem {
    pub name: String,
}

impl Packet for ServerboundRenameItem {
    const ID: i32 = 0x31;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_string(&self.name, 32767)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            name: buf.read_string(32767)?,
        })
    }
}

#[cfg(test)]
#[path = "rename_item_tests.rs"]
mod tests;
