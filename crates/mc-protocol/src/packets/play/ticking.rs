//! 26.1.2 ticking packets (`ClientboundTickingStatePacket`,
//! `ClientboundTickingStepPacket`): a float tick rate plus frozen flag, and a
//! VarInt frozen-tick budget.
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundTickingState {
    pub tick_rate: f32,
    pub frozen: bool,
}
impl Packet for ClientboundTickingState {
    // GameProtocols.CLIENTBOUND_TEMPLATE: TICKING_STATE at 128 (wire id 0x7F),
    // right after TEST_INSTANCE_BLOCK_STATUS and before TICKING_STEP.
    const ID: i32 = 0x82;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f32(self.tick_rate);
        buf.write_bool(self.frozen);
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            tick_rate: buf.read_f32()?,
            frozen: buf.read_bool()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundTickingStep {
    pub tick_steps: i32,
}
impl Packet for ClientboundTickingStep {
    // GameProtocols.CLIENTBOUND_TEMPLATE: TICKING_STEP at 129, right after
    // TICKING_STATE (0x7F) and before the common TRANSFER packet.
    const ID: i32 = 0x83;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.tick_steps);
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            tick_steps: buf.read_varint()?,
        })
    }
}

#[cfg(test)]
#[path = "ticking_tests.rs"]
mod tests;
