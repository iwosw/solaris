use bytes::{Buf, BufMut};

use crate::{
    codec::{ReadMc, WriteMc},
    error::CodecError,
    packets::Packet,
};

/// Local 26.1.2 `GameProtocols` registration index 34 and
/// `ServerboundMoveVehiclePacket.STREAM_CODEC`: Vec3, float, float, boolean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerboundMoveVehicle {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl Packet for ServerboundMoveVehicle {
    const ID: i32 = 0x22;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        buf.write_bool(self.on_ground);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
            on_ground: buf.read_bool()?,
        })
    }
}

/// Local 26.1.2 `GameProtocols` clientbound registration index 57;
/// `ClientboundMoveVehiclePacket.STREAM_CODEC`: Vec3, float, float.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientboundMoveVehicle {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for ClientboundMoveVehicle {
    const ID: i32 = 0x39;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_f64(self.x);
        buf.write_f64(self.y);
        buf.write_f64(self.z);
        buf.write_f32(self.yaw);
        buf.write_f32(self.pitch);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            x: buf.read_f64()?,
            y: buf.read_f64()?,
            z: buf.read_f64()?,
            yaw: buf.read_f32()?,
            pitch: buf.read_f32()?,
        })
    }
}
