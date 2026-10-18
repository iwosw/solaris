//! 26.1.2 ServerboundContainerSlotStateChangedPacket (javap -p -c).
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundContainerSlotStateChanged {
    pub slot_id: i32,
    pub container_id: i32,
    /// True enables the slot; false disables it (ServerGamePacketListenerImpl).
    pub new_state: bool,
}
impl Packet for ServerboundContainerSlotStateChanged {
    // reports/packets.json: minecraft:container_slot_state_changed = 20.
    const ID: i32 = 0x14;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.slot_id);
        buf.write_varint(self.container_id);
        buf.write_bool(self.new_state);
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            slot_id: buf.read_varint()?,
            container_id: buf.read_varint()?,
            new_state: buf.read_bool()?,
        })
    }
}
#[cfg(test)]
#[path = "crafter_tests.rs"]
mod tests;
