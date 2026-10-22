//! 26.1.2 title packets (`ClientboundSetTitleTextPacket`,
//! `ClientboundSetSubtitleTextPacket`, `ClientboundSetActionBarTextPacket`,
//! `ClientboundSetTitlesAnimationPacket`, `ClientboundClearTitlesPacket`):
//! trusted network-NBT component, three big-endian ints, or a reset-times bool.
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSetTitleText {
    pub content: Tag,
}
impl Packet for ClientboundSetTitleText {
    // GameProtocols.CLIENTBOUND_TEMPLATE: bundle=0, ADD_ENTITY=1, ..., SET_TIME
    // at 114 (wire id 0x71, existing anchor), SET_TITLE_TEXT at 115.
    const ID: i32 = 0x72;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        mc_nbt::write_network_value(buf, &self.content)?;
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            content: mc_nbt::read_network_value(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSetSubtitleText {
    pub content: Tag,
}
impl Packet for ClientboundSetSubtitleText {
    // GameProtocols.CLIENTBOUND_TEMPLATE: SET_SUBTITLE_TEXT at 113, between
    // SET_SIMULATION_DISTANCE (0x6F) and SET_TIME (0x71, existing anchor).
    const ID: i32 = 0x70;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        mc_nbt::write_network_value(buf, &self.content)?;
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            content: mc_nbt::read_network_value(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSetActionBarText {
    pub content: Tag,
}
impl Packet for ClientboundSetActionBarText {
    // GameProtocols.CLIENTBOUND_TEMPLATE: SET_ACTION_BAR_TEXT at 88, between
    // SERVER_DATA and the SET_BORDER_* run (wire id 0x57).
    const ID: i32 = 0x57;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        mc_nbt::write_network_value(buf, &self.content)?;
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            content: mc_nbt::read_network_value(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundSetTitlesAnimation {
    pub fade_in: i32,
    pub stay: i32,
    pub fade_out: i32,
}
impl Packet for ClientboundSetTitlesAnimation {
    // GameProtocols.CLIENTBOUND_TEMPLATE: SET_TITLES_ANIMATION at 116, right
    // after SET_TITLE_TEXT (0x72).
    const ID: i32 = 0x73;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i32(self.fade_in);
        buf.write_i32(self.stay);
        buf.write_i32(self.fade_out);
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            fade_in: buf.read_i32()?,
            stay: buf.read_i32()?,
            fade_out: buf.read_i32()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundClearTitles {
    pub reset_times: bool,
}
impl Packet for ClientboundClearTitles {
    // GameProtocols.CLIENTBOUND_TEMPLATE: CLEAR_TITLES at 13, right after
    // CHUNK_BATCH_START and before COMMANDS (wire id 0x0E).
    const ID: i32 = 0x0E;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_bool(self.reset_times);
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            reset_times: buf.read_bool()?,
        })
    }
}

#[cfg(test)]
#[path = "titles_tests.rs"]
mod tests;
