//! Configuration push verified against local 26.1.2 packets.json and javap
//! ClientboundResourcePackPushPacket: UUID, UTF8 URL, UTF8 hash (40), bool,
//! optional context-free component (network NBT).
use super::Packet;
use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;
use bytes::{Buf, BufMut};

#[derive(Debug, Clone, PartialEq)]
pub struct ResourcePackPush {
    pub id: uuid::Uuid,
    pub url: String,
    pub sha1: String,
    pub required: bool,
    pub prompt: Option<mc_nbt::Tag>,
}

impl Packet for ResourcePackPush {
    const ID: i32 = 9;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_uuid(self.id);
        buf.write_string(&self.url, 32767)?;
        buf.write_string(&self.sha1, 40)?;
        buf.write_bool(self.required);
        buf.write_bool(self.prompt.is_some());
        if let Some(prompt) = &self.prompt {
            mc_nbt::write_network(buf, prompt)?;
        }
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            id: buf.read_uuid()?,
            url: buf.read_string(32767)?,
            sha1: buf.read_string(40)?,
            required: buf.read_bool()?,
            prompt: if buf.read_bool()? {
                Some(mc_nbt::read_network(buf)?)
            } else {
                None
            },
        })
    }
}
