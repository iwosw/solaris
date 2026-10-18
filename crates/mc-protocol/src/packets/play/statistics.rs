//! Vanilla 26.1.2 statistics snapshot codec.
//! Provenance: local `javap -c -p` against
//! `/tmp/vanilla262/META-INF/versions/26.1.2/server-26.1.2.jar`:
//! GameProtocols registers CLIENTBOUND_AWARD_STATS at index 3;
//! ClientboundAwardStatsPacket maps Stat.STREAM_CODEC to VAR_INT counts.
//! Stat dispatches the STAT_TYPE registry id to StatType's value registry codec;
//! ByteBufCodecs.map prefixes the entries with a VarInt count.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundAwardStats {
    /// `(stat type registry id, value registry id, count)`.
    pub stats: Vec<(i32, i32, i32)>,
}

impl Packet for ClientboundAwardStats {
    const ID: i32 = 0x03;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_count(buf, self.stats.len())?;
        for &(stat_type, stat, value) in &self.stats {
            buf.write_varint(stat_type);
            buf.write_varint(stat);
            buf.write_varint(value);
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let stats = read_bounded_vec(buf, i32::MAX as usize, 3, |buf| {
            Ok((buf.read_varint()?, buf.read_varint()?, buf.read_varint()?))
        })?;
        Ok(Self { stats })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_id_and_layout_match_local_javap() {
        // Stats.<clinit>: mined=0, custom=8, play_time=1, jump=23.
        // Stat/StatType registry codecs and ByteBufCodecs$25 establish
        // VarInt entry count then type id, value id, count for each entry.
        assert_eq!(ClientboundAwardStats::ID, 0x03);
        let packet = ClientboundAwardStats {
            stats: vec![(8, 1, 300), (0, 128, 2), (8, 23, i32::MAX)],
        };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(
            bytes,
            [
                3, 8, 1, 0xac, 2, 0, 0x80, 1, 2, 8, 23, 0xff, 0xff, 0xff, 0xff, 7
            ]
        );
        let mut cursor = bytes.as_slice();
        assert_eq!(ClientboundAwardStats::decode(&mut cursor).unwrap(), packet);
        assert!(cursor.is_empty());
    }

    #[test]
    fn empty_statistics_snapshot_still_has_count() {
        let mut bytes = Vec::new();
        ClientboundAwardStats { stats: Vec::new() }
            .encode(&mut bytes)
            .unwrap();
        assert_eq!(bytes, [0]);
    }

    #[test]
    fn request_stats_action_layout_matches_local_javap() {
        // ServerboundClientCommandPacket.readEnum/writeEnum and Action.<clinit>
        // establish REQUEST_STATS ordinal 1; GameProtocols SB index is 12.
        assert_eq!(ServerboundClientCommand::ID, 0x0c);
        let packet = ServerboundClientCommand {
            action: ClientCommandAction::RequestStats,
        };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(bytes, [1]);
        assert_eq!(
            ServerboundClientCommand::decode(&mut bytes.as_slice()).unwrap(),
            packet
        );
    }
}
