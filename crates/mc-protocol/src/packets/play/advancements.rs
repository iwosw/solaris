//! Vanilla 26.1.2 advancement codecs, derived from local server javap.
//! DisplayInfo uses ItemStackTemplate (raw item id, count, component patch),
//! not the optional inventory Slot codec. Criteria/rewards are not on the wire.
use super::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvancementFrame {
    Task,
    Challenge,
    Goal,
}
#[derive(Debug, Clone, PartialEq)]
pub struct AdvancementDisplay {
    pub title: Tag,
    pub description: Tag,
    pub icon: mc_data::ItemStack,
    pub frame: AdvancementFrame,
    pub background: Option<Identifier>,
    pub show_toast: bool,
    pub hidden: bool,
    pub x: f32,
    pub y: f32,
}
#[derive(Debug, Clone, PartialEq)]
pub struct AdvancementDefinition {
    pub id: Identifier,
    pub parent: Option<Identifier>,
    pub display: Option<AdvancementDisplay>,
    pub requirements: Vec<Vec<String>>,
    pub sends_telemetry: bool,
}
/// Criterion timestamps are signed Unix epoch milliseconds (nullable Instant).
pub type AdvancementProgress = BTreeMap<String, Option<i64>>;
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundUpdateAdvancements {
    pub reset: bool,
    pub added: Vec<AdvancementDefinition>,
    pub removed: Vec<Identifier>,
    pub progress: BTreeMap<Identifier, AdvancementProgress>,
    pub show_advancements: bool,
}
fn write_id_option<B: BufMut>(buf: &mut B, id: &Option<Identifier>) -> Result<(), CodecError> {
    buf.write_bool(id.is_some());
    if let Some(id) = id {
        buf.write_identifier(id)?;
    }
    Ok(())
}
fn read_id_option<B: Buf>(buf: &mut B) -> Result<Option<Identifier>, CodecError> {
    if buf.read_bool()? {
        Ok(Some(buf.read_identifier()?))
    } else {
        Ok(None)
    }
}
impl Packet for ClientboundUpdateAdvancements {
    // GameProtocols CLIENTBOUND_TEMPLATE ordinal 130, including common/cookie/ping.
    const ID: i32 = 0x82;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_bool(self.reset);
        buf.write_varint(self.added.len() as i32);
        for definition in &self.added {
            buf.write_identifier(&definition.id)?;
            write_id_option(buf, &definition.parent)?;
            buf.write_bool(definition.display.is_some());
            if let Some(display) = &definition.display {
                mc_nbt::write_network_value(buf, &display.title)?;
                mc_nbt::write_network_value(buf, &display.description)?;
                item_stack_template::encode(buf, &display.icon, 0)?;
                buf.write_varint(match display.frame {
                    AdvancementFrame::Task => 0,
                    AdvancementFrame::Challenge => 1,
                    AdvancementFrame::Goal => 2,
                });
                buf.write_i32(
                    i32::from(display.background.is_some())
                        | (i32::from(display.show_toast) << 1)
                        | (i32::from(display.hidden) << 2),
                );
                if let Some(background) = &display.background {
                    buf.write_identifier(background)?;
                }
                buf.write_f32(display.x);
                buf.write_f32(display.y);
            }
            buf.write_varint(definition.requirements.len() as i32);
            for group in &definition.requirements {
                buf.write_varint(group.len() as i32);
                for criterion in group {
                    buf.write_string(criterion, DEFAULT_MAX_STRING_LEN)?;
                }
            }
            buf.write_bool(definition.sends_telemetry);
        }
        buf.write_varint(self.removed.len() as i32);
        for id in &self.removed {
            buf.write_identifier(id)?;
        }
        buf.write_varint(self.progress.len() as i32);
        for (id, criteria) in &self.progress {
            buf.write_identifier(id)?;
            buf.write_varint(criteria.len() as i32);
            for (name, timestamp) in criteria {
                buf.write_string(name, DEFAULT_MAX_STRING_LEN)?;
                buf.write_bool(timestamp.is_some());
                if let Some(timestamp) = timestamp {
                    buf.write_i64(*timestamp);
                }
            }
        }
        buf.write_bool(self.show_advancements);
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let reset = buf.read_bool()?;
        let added = read_bounded_vec(buf, 8192, 5, |buf| {
            let id = buf.read_identifier()?;
            let parent = read_id_option(buf)?;
            let display = if buf.read_bool()? {
                let title = mc_nbt::read_network_value(buf)?;
                let description = mc_nbt::read_network_value(buf)?;
                let icon = item_stack_template::decode(buf, 0)?;
                let frame = match buf.read_varint()? {
                    0 => AdvancementFrame::Task,
                    1 => AdvancementFrame::Challenge,
                    2 => AdvancementFrame::Goal,
                    _ => return Err(CodecError::NotSupported("invalid advancement frame")),
                };
                let flags = buf.read_i32()?;
                let background = if flags & 1 != 0 {
                    Some(buf.read_identifier()?)
                } else {
                    None
                };
                Some(AdvancementDisplay {
                    title,
                    description,
                    icon,
                    frame,
                    background,
                    show_toast: flags & 2 != 0,
                    hidden: flags & 4 != 0,
                    x: buf.read_f32()?,
                    y: buf.read_f32()?,
                })
            } else {
                None
            };
            let requirements = read_bounded_vec(buf, 1024, 1, |buf| {
                read_bounded_vec(buf, 1024, 1, |buf| buf.read_string(DEFAULT_MAX_STRING_LEN))
            })?;
            Ok(AdvancementDefinition {
                id,
                parent,
                display,
                requirements,
                sends_telemetry: buf.read_bool()?,
            })
        })?;
        let removed = read_bounded_vec(buf, 8192, 1, |buf| buf.read_identifier())?;
        let count = read_bounded_count(buf, 8192, 2)?;
        let mut progress = BTreeMap::new();
        for _ in 0..count {
            let id = buf.read_identifier()?;
            let count = read_bounded_count(buf, 1024, 2)?;
            let mut criteria = BTreeMap::new();
            for _ in 0..count {
                let name = buf.read_string(DEFAULT_MAX_STRING_LEN)?;
                let timestamp = if buf.read_bool()? {
                    Some(buf.read_i64()?)
                } else {
                    None
                };
                criteria.insert(name, timestamp);
            }
            progress.insert(id, criteria);
        }
        Ok(Self {
            reset,
            added,
            removed,
            progress,
            show_advancements: buf.read_bool()?,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerboundSeenAdvancements {
    OpenedTab(Identifier),
    ClosedScreen,
}
impl Packet for ServerboundSeenAdvancements {
    // GameProtocols SERVERBOUND_TEMPLATE ordinal 50, including common/cookie/ping.
    const ID: i32 = 0x32;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        match self {
            Self::OpenedTab(id) => {
                buf.write_varint(0);
                buf.write_identifier(id)?;
            }
            Self::ClosedScreen => buf.write_varint(1),
        }
        Ok(())
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        match buf.read_varint()? {
            0 => Ok(Self::OpenedTab(buf.read_identifier()?)),
            1 => Ok(Self::ClosedScreen),
            _ => Err(CodecError::NotSupported("invalid seen advancements action")),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSelectAdvancementsTab {
    pub tab: Option<Identifier>,
}
impl Packet for ClientboundSelectAdvancementsTab {
    // GameProtocols CLIENTBOUND_TEMPLATE ordinal 85, including common/cookie/ping.
    const ID: i32 = 0x55;
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_id_option(buf, &self.tab)
    }
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            tab: read_id_option(buf)?,
        })
    }
}
#[cfg(test)]
#[path = "advancements_tests.rs"]
mod tests;
