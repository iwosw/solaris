//! 26.1.2 ClientboundBossEventPacket javap: UUID, operation enum VarInt,
//! trusted network-NBT component, progress float, color/overlay enums, flags byte.
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub enum BossOperation {
    Add {
        name: Tag,
        progress: f32,
        color: i32,
        overlay: i32,
        flags: u8,
    },
    Remove,
    Progress(f32),
    Name(Tag),
    Style {
        color: i32,
        overlay: i32,
    },
    Properties(u8),
}
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundBossEvent {
    pub uuid: Uuid,
    pub operation: BossOperation,
}
impl Packet for ClientboundBossEvent {
    // GameProtocols.CLIENTBOUND_TEMPLATE: bundle=0, ADD_ENTITY=1,
    // ANIMATE=2, AWARD_STATS=3, block ack/destruction/entity/event/update=4..8.
    const ID: i32 = 0x09;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_uuid(self.uuid);
        match &self.operation {
            BossOperation::Add {
                name,
                progress,
                color,
                overlay,
                flags,
            } => {
                buf.write_varint(0);
                mc_nbt::write_network_value(buf, name)?;
                buf.write_f32(*progress);
                buf.write_varint(*color);
                buf.write_varint(*overlay);
                buf.write_u8(*flags);
            }
            BossOperation::Remove => buf.write_varint(1),
            BossOperation::Progress(value) => {
                buf.write_varint(2);
                buf.write_f32(*value);
            }
            BossOperation::Name(value) => {
                buf.write_varint(3);
                mc_nbt::write_network_value(buf, value)?;
            }
            BossOperation::Style { color, overlay } => {
                buf.write_varint(4);
                buf.write_varint(*color);
                buf.write_varint(*overlay);
            }
            BossOperation::Properties(flags) => {
                buf.write_varint(5);
                buf.write_u8(*flags);
            }
        }
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let uuid = buf.read_uuid()?;
        let operation = match buf.read_varint()? {
            0 => BossOperation::Add {
                name: mc_nbt::read_network_value(buf)?,
                progress: buf.read_f32()?,
                color: buf.read_varint()?,
                overlay: buf.read_varint()?,
                flags: buf.read_u8()?,
            },
            1 => BossOperation::Remove,
            2 => BossOperation::Progress(buf.read_f32()?),
            3 => BossOperation::Name(mc_nbt::read_network_value(buf)?),
            4 => BossOperation::Style {
                color: buf.read_varint()?,
                overlay: buf.read_varint()?,
            },
            5 => BossOperation::Properties(buf.read_u8()?),
            _ => return Err(CodecError::NotSupported("invalid boss operation")),
        };
        Ok(Self { uuid, operation })
    }
}
#[cfg(test)]
#[path = "boss_event_tests.rs"]
mod tests;
