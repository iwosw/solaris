//! ClientboundMapItemDataPacket / MapPatch / MapDecoration javap, 26.1.2.
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct MapDecoration {
    pub kind: i32,
    pub x: i8,
    pub z: i8,
    pub rotation: u8,
    pub name: Option<Tag>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundMapData {
    pub id: i32,
    pub scale: u8,
    pub locked: bool,
    pub decorations: Option<Vec<MapDecoration>>,
    /// x, z, width, height; zero width encodes an absent patch.
    pub rectangle: [u8; 4],
    pub colors: Vec<u8>,
}
impl ClientboundMapData {
    fn validate(&self) -> Result<(), CodecError> {
        let [x, z, w, h] = self.rectangle;
        if self.id < 0 || self.scale > 4 || usize::from(w) * usize::from(h) != self.colors.len()
            || (w != 0 && (h == 0 || u16::from(x) + u16::from(w) > 128 || u16::from(z) + u16::from(h) > 128))
            || self.decorations.as_ref().is_some_and(|d| d.len() > 1024 || d.iter().any(|d| !(0..35).contains(&d.kind))) {
            return Err(CodecError::NotSupported("invalid map data"));
        }
        Ok(())
    }
}
impl Packet for ClientboundMapData {
    // reports/packets.json play/clientbound minecraft:map_item_data.
    const ID: i32 = 51;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.validate()?;
        buf.write_varint(self.id);
        buf.write_u8(self.scale);
        buf.write_bool(self.locked);
        buf.write_bool(self.decorations.is_some());
        if let Some(decorations) = &self.decorations {
            buf.write_varint(decorations.len() as i32);
            for d in decorations {
                buf.write_varint(d.kind);
                buf.write_u8(d.x as u8);
                buf.write_u8(d.z as u8);
                buf.write_u8(d.rotation & 15);
                buf.write_bool(d.name.is_some());
                if let Some(name) = &d.name { mc_nbt::write_network_value(buf, name)?; }
            }
        }
        let [x, z, w, h] = self.rectangle;
        buf.write_u8(w);
        if w != 0 {
            buf.write_u8(h); buf.write_u8(x); buf.write_u8(z);
            buf.write_varint(self.colors.len() as i32); buf.put_slice(&self.colors);
        }
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let id = buf.read_varint()?;
        let scale = buf.read_u8()?;
        let locked = buf.read_bool()?;
        let decorations = if buf.read_bool()? {
            let count = buf.read_varint()?;
            if !(0..=1024).contains(&count) { return Err(CodecError::NotSupported("map decoration count")); }
            let mut values = Vec::with_capacity(count as usize);
            for _ in 0..count {
                values.push(MapDecoration { kind: buf.read_varint()?, x: buf.read_u8()? as i8,
                    z: buf.read_u8()? as i8, rotation: buf.read_u8()? & 15,
                    name: if buf.read_bool()? { Some(mc_nbt::read_network_value(buf)?) } else { None } });
            }
            Some(values)
        } else { None };
        let w = buf.read_u8()?;
        let mut rectangle = [0; 4];
        let mut colors = Vec::new();
        if w != 0 {
            let h = buf.read_u8()?; let x = buf.read_u8()?; let z = buf.read_u8()?;
            rectangle = [x, z, w, h];
            let count = buf.read_varint()?;
            if count != i32::from(w) * i32::from(h) || count > 16384 || buf.remaining() < count as usize {
                return Err(CodecError::NotSupported("map patch length"));
            }
            colors.resize(count as usize, 0);
            buf.copy_to_slice(&mut colors);
        }
        let packet = Self { id, scale, locked, decorations, rectangle, colors };
        packet.validate()?;
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn map_patch_roundtrip_and_bounds() {
        let packet = ClientboundMapData { id: 7, scale: 2, locked: true,
            decorations: Some(vec![MapDecoration { kind: 26, x: -12, z: 13, rotation: 8, name: None }]),
            rectangle: [126, 127, 2, 1], colors: vec![5, 6] };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(ClientboundMapData::decode(&mut buf.as_slice()).unwrap(), packet);
        let mut invalid = packet;
        invalid.rectangle[0] = 127;
        assert!(invalid.encode(&mut Vec::new()).is_err());
    }
}
