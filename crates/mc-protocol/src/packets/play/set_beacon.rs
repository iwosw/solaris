use bytes::{Buf, BufMut};

use crate::{
    codec::{ReadMc, WriteMc},
    error::CodecError,
    packets::Packet,
};

/// `ServerboundSetBeacon` (play, serverbound).
///
/// Local 26.1.2 evidence (`javap GameProtocols.lambda$static$1` on the
/// bundled `versions/26.1.2/server-26.1.2.jar`): `SERVERBOUND_SET_BEACON`
/// is the 53rd `addPacket` call in the serverbound game template, so the
/// wire id is `0x34`. `javap ServerboundSetBeaconPacket` shows a record of
/// two `Optional<Holder<MobEffect>>` fields encoded by
/// `StreamCodec.composite(ByteBufCodecs.optional(MobEffect.STREAM_CODEC),
/// ...)`; the inner holder codec is `holderRegistry` (VarInt registry
/// id), and `ByteBufCodecs$23` (the `optional` wrapper) writes one boolean
/// then the value when present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundSetBeacon {
    /// Zero-based `mob_effect` registry id, or `None` for "no effect".
    pub primary: Option<u32>,
    /// Zero-based `mob_effect` registry id, or `None` for "no effect".
    pub secondary: Option<u32>,
}

impl Packet for ServerboundSetBeacon {
    const ID: i32 = 0x35;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_effect(buf, self.primary)?;
        write_effect(buf, self.secondary)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            primary: read_effect(buf)?,
            secondary: read_effect(buf)?,
        })
    }
}

fn write_effect<B: BufMut>(buf: &mut B, effect: Option<u32>) -> Result<(), CodecError> {
    match effect {
        None => {
            buf.write_bool(false);
            Ok(())
        }
        Some(id) => {
            if mc_data::mob_effects_26_1_2::name_for_id(id).is_none() {
                return Err(CodecError::NotSupported("unknown mob-effect registry id"));
            }
            buf.write_bool(true);
            buf.write_varint(id as i32);
            Ok(())
        }
    }
}

fn read_effect<B: Buf>(buf: &mut B) -> Result<Option<u32>, CodecError> {
    if !buf.read_bool()? {
        return Ok(None);
    }
    let raw = buf.read_varint()?;
    let id = u32::try_from(raw)
        .map_err(|_| CodecError::NotSupported("negative mob-effect registry id"))?;
    if mc_data::mob_effects_26_1_2::name_for_id(id).is_none() {
        return Err(CodecError::NotSupported("unknown mob-effect registry id"));
    }
    Ok(Some(id))
}

#[cfg(test)]
#[path = "set_beacon_tests.rs"]
mod tests;
