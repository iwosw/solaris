use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;
use crate::packets::Packet;
use bytes::{Buf, BufMut};

/// Pinned 26.1.2 ClientboundPlayerCombatKillPacket: VarInt player id,
/// followed by a trusted network-NBT component (local javap STREAM_CODEC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundPlayerCombatKill {
    pub player_id: i32,
    pub message_nbt: Vec<u8>,
}

impl Packet for ClientboundPlayerCombatKill {
    // 26.1.2 reports/packets.json: minecraft:player_combat_kill = 0x44
    // (0x45 is player_info_remove; bundle_delimiter is 0x00).
    const ID: i32 = 0x45;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.player_id);
        buf.put_slice(&self.message_nbt);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let player_id = buf.read_varint()?;
        let message_nbt = buf.copy_to_bytes(buf.remaining()).to_vec();
        Ok(Self {
            player_id,
            message_nbt,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn combat_kill_retains_translated_component() {
        let mut message_nbt = Vec::new();
        mc_nbt::write_network(
            &mut message_nbt,
            &mc_nbt::Tag::Compound(vec![(
                "translate".into(),
                mc_nbt::Tag::String("death.attack.genericKill".into()),
            )]),
        )
        .unwrap();
        let packet = ClientboundPlayerCombatKill {
            player_id: 300,
            message_nbt,
        };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(ClientboundPlayerCombatKill::ID, 0x45);
        assert_eq!(&bytes[..2], &[0xac, 0x02]);
        assert_eq!(
            ClientboundPlayerCombatKill::decode(&mut bytes.as_slice()).unwrap(),
            packet
        );
    }
}
