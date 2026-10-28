//! Play chunk packets: chunk loading, block deltas, section updates, light updates.

use super::*;

/// `Clientbound Forget Level Chunk` (CB). Unloads one chunk from the
/// client's currently loaded view.
///
/// Per ADR 0002, verified against vanilla 26.1.2: `GameProtocols`
/// registers `CLIENTBOUND_FORGET_LEVEL_CHUNK` at game-CB index 37
/// (including `CLIENTBOUND_BUNDLE`), and
/// `ClientboundForgetLevelChunkPacket.write` calls
/// `FriendlyByteBuf.writeChunkPos`. `writeChunkPos` writes one raw
/// big-endian `i64` from `ChunkPos.pack(x, z)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgetLevelChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl Packet for ForgetLevelChunk {
    const ID: i32 = 0x26;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(pack_chunk_pos(self.chunk_x, self.chunk_z));
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let packed = buf.read_i64()?;
        Ok(Self {
            chunk_x: packed as i32,
            chunk_z: (packed >> 32) as i32,
        })
    }
}

/// Pack vanilla's `ChunkPos.pack(x, z)` representation:
/// `(x & 0xffffffff) | ((z & 0xffffffff) << 32)`.
#[must_use]
pub fn pack_chunk_pos(x: i32, z: i32) -> i64 {
    ((x as i64) & 0xFFFF_FFFF) | (((z as i64) & 0xFFFF_FFFF) << 32)
}

/// A single heightmap entry inside [`LevelChunkWithLight`].
///
/// In 26.1 the heightmaps field on the wire is *no longer* an NBT
/// compound (it was, pre-1.20). It is a `Map<Heightmap$Types, long[]>`
/// encoded via `ByteBufCodecs.map(EnumMap, Heightmap$Types.STREAM_CODEC,
/// LONG_ARRAY)` — count, then `VarInt typeId` + `long[]` per entry.
///
/// `type_id` is the ordinal of the entry in `Heightmap$Types`
/// (verified by `javap -p -c` of the unobfuscated jar, per ADR 0002).
/// Only the three entries with `Usage.CLIENT` are sent:
/// `WORLD_SURFACE = 1`, `MOTION_BLOCKING = 4`,
/// `MOTION_BLOCKING_NO_LEAVES = 5`. The packed `long[]` is the same
/// 9-bits-per-entry, non-crossing-i64 packing the on-disk heightmap
/// uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkHeightmap {
    pub type_id: i32,
    pub data: Vec<i64>,
}

impl ChunkHeightmap {
    pub const WORLD_SURFACE: i32 = 1;
    pub const MOTION_BLOCKING: i32 = 4;
    pub const MOTION_BLOCKING_NO_LEAVES: i32 = 5;
}

/// One block-entity sidecar entry carried inside [`LevelChunkWithLight`].
///
/// Verified via `javap -p -c` of
/// `ClientboundLevelChunkPacketData$BlockEntityInfo.write` (ADR 0002):
/// `u8 packedXZ`, `i16 y`, `VarInt typeId` (id into
/// `minecraft:block_entity_type` registry), then network-NBT
/// (unnamed-root) compound tag.
///
/// `packed_xz` is `(x << 4) | z` with `x`, `z` both 4-bit
/// section-relative; vanilla never emits chunk-relative coords here.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntityInfo {
    pub packed_xz: u8,
    pub y: i16,
    pub type_id: i32,
    pub nbt: Tag,
}

impl BlockEntityInfo {
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_u8(self.packed_xz);
        buf.write_i16(self.y);
        buf.write_varint(self.type_id);
        mc_nbt::write_network(buf, &self.nbt)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            packed_xz: buf.read_u8()?,
            y: buf.read_i16()?,
            type_id: buf.read_varint()?,
            nbt: mc_nbt::read_network(buf)?,
        })
    }
}

/// Sky and block light payload inlined into [`LevelChunkWithLight`].
///
/// Verified via `javap -p -c` of
/// `ClientboundLightUpdatePacketData.write` (ADR 0002): four BitSets
/// (`writeBitSet` = `writeLongArray(bs.toLongArray())`) followed by
/// two `Collection<byte[]>` lists, each entry a 2048-byte light layer
/// (4 bits per block × 16³ blocks = 2048 bytes).
///
/// Bit `i` set in `sky_y_mask` (resp. `block_y_mask`) means section
/// `i` has explicit sky-light (resp. block-light) data in
/// `sky_updates` (resp. `block_updates`). Bit `i` set in
/// `empty_sky_y_mask` (resp. `empty_block_y_mask`) means section `i`
/// has no light data — the client uses its own default. Section
/// indexing extends by one entry on each side of the world Y range,
/// so for a 24-section column there are 26 indexable sections
/// (`-1..=24`); vanilla packs this in a single i64 per bitset.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightData {
    pub sky_y_mask: Vec<i64>,
    pub block_y_mask: Vec<i64>,
    pub empty_sky_y_mask: Vec<i64>,
    pub empty_block_y_mask: Vec<i64>,
    pub sky_updates: Vec<Vec<u8>>,
    pub block_updates: Vec<Vec<u8>>,
}

impl LightData {
    /// Length of one light-data layer in bytes (`16³ / 2`, since vanilla
    /// packs 4 bits per block).
    pub const LIGHT_LAYER_BYTES: usize = 2048;

    /// "No lighting data" payload: all masks empty, both update lists
    /// empty. Legal on the wire; the client uses its own defaults.
    /// Six VarInt(0) bytes total.
    pub fn empty() -> Self {
        Self::default()
    }

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_long_array(buf, &self.sky_y_mask)?;
        write_long_array(buf, &self.block_y_mask)?;
        write_long_array(buf, &self.empty_sky_y_mask)?;
        write_long_array(buf, &self.empty_block_y_mask)?;
        Self::encode_light_list(buf, &self.sky_updates)?;
        Self::encode_light_list(buf, &self.block_updates)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let sky_y_mask = read_long_array(buf)?;
        let block_y_mask = read_long_array(buf)?;
        let empty_sky_y_mask = read_long_array(buf)?;
        let empty_block_y_mask = read_long_array(buf)?;
        let sky_updates = Self::decode_light_list(buf)?;
        let block_updates = Self::decode_light_list(buf)?;
        Ok(Self {
            sky_y_mask,
            block_y_mask,
            empty_sky_y_mask,
            empty_block_y_mask,
            sky_updates,
            block_updates,
        })
    }

    fn encode_light_list<B: BufMut>(buf: &mut B, layers: &[Vec<u8>]) -> Result<(), CodecError> {
        if layers.len() > 1024 {
            return Err(CodecError::StringTooLong {
                len: layers.len(),
                max: 1024,
            });
        }
        let len = i32::try_from(layers.len()).map_err(|_| CodecError::StringTooLong {
            len: layers.len(),
            max: 1024,
        })?;
        buf.write_varint(len);
        for layer in layers {
            if layer.len() > Self::LIGHT_LAYER_BYTES {
                return Err(CodecError::StringTooLong {
                    len: layer.len(),
                    max: Self::LIGHT_LAYER_BYTES,
                });
            }
            buf.write_byte_array(layer)?;
        }
        Ok(())
    }

    fn decode_light_list<B: Buf>(buf: &mut B) -> Result<Vec<Vec<u8>>, CodecError> {
        // Each layer is at most LIGHT_LAYER_BYTES; cap collection length
        // at one entry per Y section in a 1024-block-tall column (huge
        // overshoot for vanilla's 26).
        read_bounded_vec(buf, 1024, 1, |buf| {
            buf.read_byte_array(Self::LIGHT_LAYER_BYTES)
        })
    }
}

/// `Level Chunk With Light` (CB). The packet that finally puts blocks
/// in front of the client. Wraps the bodies of vanilla's
/// `ClientboundLevelChunkPacketData` and `ClientboundLightUpdatePacketData`
/// behind the chunk coordinates.
///
/// `data` is the paletted-container chunk body: section-by-section
/// (`i16 block_count`, paletted block-state container, paletted biome
/// container) concatenated. M3.a defines the outer packet only —
/// constructing `data` from a `mc_world::Chunk` is M3.b's job, so the
/// field is exposed as raw bytes here and a `Vec<u8>` round-trips
/// through `encode` / `decode` unchanged.
///
/// The leading `i32 chunk_x` / `i32 chunk_z` are raw `int`s on the
/// wire (not VarInts), per
/// `ClientboundLevelChunkWithLightPacket.STREAM_CODEC` in the
/// unobfuscated jar.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelChunkWithLight {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub heightmaps: Vec<ChunkHeightmap>,
    pub data: Vec<u8>,
    pub block_entities: Vec<BlockEntityInfo>,
    pub light: LightData,
}

impl Packet for LevelChunkWithLight {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_LEVEL_CHUNK_WITH_LIGHT at game-CB index 45 = 0x2D.
    const ID: i32 = 0x2E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        if self.heightmaps.len() > MAX_CHUNK_HEIGHTMAPS {
            return Err(CodecError::StringTooLong {
                len: self.heightmaps.len(),
                max: MAX_CHUNK_HEIGHTMAPS,
            });
        }
        if self.block_entities.len() > MAX_CHUNK_BLOCK_ENTITIES {
            return Err(CodecError::StringTooLong {
                len: self.block_entities.len(),
                max: MAX_CHUNK_BLOCK_ENTITIES,
            });
        }
        buf.write_i32(self.chunk_x);
        buf.write_i32(self.chunk_z);

        let hm_count =
            i32::try_from(self.heightmaps.len()).map_err(|_| CodecError::StringTooLong {
                len: self.heightmaps.len(),
                max: MAX_CHUNK_HEIGHTMAPS,
            })?;
        buf.write_varint(hm_count);
        for hm in &self.heightmaps {
            buf.write_varint(hm.type_id);
            write_long_array(buf, &hm.data)?;
        }

        if self.data.len() > MAX_CHUNK_DATA_LEN {
            return Err(CodecError::StringTooLong {
                len: self.data.len(),
                max: MAX_CHUNK_DATA_LEN,
            });
        }
        buf.write_byte_array(&self.data)?;

        let be_count =
            i32::try_from(self.block_entities.len()).map_err(|_| CodecError::StringTooLong {
                len: self.block_entities.len(),
                max: MAX_CHUNK_BLOCK_ENTITIES,
            })?;
        buf.write_varint(be_count);
        for be in &self.block_entities {
            be.encode(buf)?;
        }

        self.light.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let chunk_x = buf.read_i32()?;
        let chunk_z = buf.read_i32()?;

        // Six entries in Heightmap$Types; cap at 16 to leave room. One
        // heightmap contains at least a type-id and long-array count VarInt.
        let heightmaps = read_bounded_vec(buf, MAX_CHUNK_HEIGHTMAPS, 2, |buf| {
            Ok(ChunkHeightmap {
                type_id: buf.read_varint()?,
                data: read_long_array(buf)?,
            })
        })?;

        let data = buf.read_byte_array(MAX_CHUNK_DATA_LEN)?;

        // One block entity per block in a 16³ section × 24 sections =
        // 98 304; cap one order of magnitude higher than that. The minimum
        // wire body is packed-xz + y + type-id + one NBT tag byte.
        let block_entities = read_bounded_vec(
            buf,
            MAX_CHUNK_BLOCK_ENTITIES,
            MIN_BLOCK_ENTITY_BYTES,
            BlockEntityInfo::decode,
        )?;

        let light = LightData::decode(buf)?;

        Ok(Self {
            chunk_x,
            chunk_z,
            heightmaps,
            data,
            block_entities,
            light,
        })
    }
}

/// `ClientboundLevelEventPacket` (CB). Vanilla uses this compact event
/// channel for client-rendered world effects such as block destroy particles
/// and their matching sound.
///
/// Verified against the local 26.1.2 source: raw `i32` event id, packed
/// `BlockPos`, raw `i32` event data, then one global-event boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelEvent {
    pub event_id: i32,
    pub position: i64,
    pub data: i32,
    pub global: bool,
}

impl Packet for LevelEvent {
    // CLIENTBOUND_LEVEL_EVENT at game-CB index 46 = wire id 0x2E.
    const ID: i32 = 0x2F;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i32(self.event_id);
        buf.write_i64(self.position);
        buf.write_i32(self.data);
        buf.write_bool(self.global);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            event_id: buf.read_i32()?,
            position: buf.read_i64()?,
            data: buf.read_i32()?,
            global: buf.read_bool()?,
        })
    }
}

/// `Set Center Chunk` (CB). Tells the client which chunk is the center
/// of its view-distance window — the chunk it is "looking from". Every
/// `LevelChunkWithLight` packet that follows is rendered relative to
/// this anchor; if the center is wrong the client will silently
/// discard chunks that fall outside its (anchored, but still
/// vd-bounded) loaded ring.
///
/// Wire layout per `javap -p -c` of
/// `ClientboundSetChunkCacheCenterPacket.write` (ADR 0002): two
/// VarInts, `x` then `z`, in chunk coordinates (= `floor(world / 16)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetCenterChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl Packet for SetCenterChunk {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_SET_CHUNK_CACHE_CENTER at game-CB index 94 = 0x5E.
    const ID: i32 = 0x60;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.chunk_x);
        buf.write_varint(self.chunk_z);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            chunk_x: buf.read_varint()?,
            chunk_z: buf.read_varint()?,
        })
    }
}

/// Clientbound effective chunk-cache radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetChunkCacheRadius {
    pub radius: i32,
}

impl Packet for SetChunkCacheRadius {
    // Verified via `javap` of vanilla 26.1.2: game-CB index 95, one VarInt.
    const ID: i32 = 0x61;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.radius);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            radius: buf.read_varint()?,
        })
    }
}

// -----------------------------------------------------------------------
// Serverbound

/// `Clientbound Block Update` (CB). Single-block delta the server
/// sends in response to any state change at a `BlockPos`. Per
/// ADR 0002, verified against `javap -p` of the unobfuscated jar:
/// `ClientboundBlockUpdatePacket(BlockPos pos, BlockState state)`,
/// the `BlockState` wire form being its global state-id VarInt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockUpdate {
    /// Block position packed into vanilla's standard `BlockPos`
    /// `i64`. Use [`pack_block_pos`] / [`unpack_block_pos`].
    pub position: i64,
    /// Global block-state id — the same id space `blocks.json` /
    /// `mc_world::BlockStateId` use.
    pub state_id: i32,
}

impl Packet for BlockUpdate {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_BLOCK_UPDATE at game-CB index 8 = wire id 0x08.
    const ID: i32 = 0x08;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.position);
        buf.write_varint(self.state_id);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            position: buf.read_i64()?,
            state_id: buf.read_varint()?,
        })
    }
}

/// One entry inside [`SectionBlocksUpdate`]. `relative_pos` is the
/// section-local 12-bit coordinate value produced by
/// [`pack_section_relative_pos`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionBlockChange {
    pub relative_pos: u16,
    pub state_id: i32,
}

/// `Clientbound Section Blocks Update` (CB). Multi-block delta for one
/// chunk section. Per ADR 0002, verified against `javap -p`:
/// `ClientboundSectionBlocksUpdatePacket(SectionPos sectionPos,
/// short[] positions, BlockState[] states)`. Vanilla's stream codec writes
/// the raw `SectionPos.asLong`, then a VarInt count, then each entry as a
/// VarLong `(state_id << 12) | (section_relative_pos & 0xFFF)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionBlocksUpdate {
    /// Raw vanilla `SectionPos.asLong`. Use [`pack_section_pos`].
    pub section_pos: i64,
    pub changes: Vec<SectionBlockChange>,
}

impl Packet for SectionBlocksUpdate {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_SECTION_BLOCKS_UPDATE at game-CB index 84 = wire id 0x54.
    const ID: i32 = 0x56;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_i64(self.section_pos);
        if self.changes.len() > MAX_SECTION_BLOCK_UPDATE_ENTRIES {
            return Err(CodecError::StringTooLong {
                len: self.changes.len(),
                max: MAX_SECTION_BLOCK_UPDATE_ENTRIES,
            });
        }
        let count = i32::try_from(self.changes.len()).map_err(|_| CodecError::StringTooLong {
            len: self.changes.len(),
            max: i32::MAX as usize,
        })?;
        buf.write_varint(count);
        for change in &self.changes {
            buf.write_varlong(pack_section_block_update_value(
                change.state_id,
                change.relative_pos,
            ));
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let section_pos = buf.read_i64()?;
        let changes = read_bounded_vec(buf, MAX_SECTION_BLOCK_UPDATE_ENTRIES, 1, |buf| {
            let value = buf.read_varlong()?;
            Ok(SectionBlockChange {
                relative_pos: (value & 0xFFF) as u16,
                state_id: (value >> 12) as i32,
            })
        })?;
        Ok(Self {
            section_pos,
            changes,
        })
    }
}

/// `Clientbound Block Changed Ack` (CB). One-VarInt packet that
/// echoes the `sequence` from a `ServerboundPlayerAction` or
/// `ServerboundUseItemOn`; without it the vanilla client rolls
/// back its predicted edit. Per ADR 0002, verified against
/// `javap -p`: `ClientboundBlockChangedAckPacket(int sequence)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockChangedAck {
    pub sequence: i32,
}

impl Packet for BlockChangedAck {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_BLOCK_CHANGED_ACK at game-CB index 4 = wire id 0x04.
    const ID: i32 = 0x04;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.sequence);
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            sequence: buf.read_varint()?,
        })
    }
}

/// `Clientbound Light Update` (CB). Per-chunk light delta — sent
/// when block changes alter sky / block light without re-streaming
/// the full chunk. Per ADR 0002, verified against `javap -p`:
/// `ClientboundLightUpdatePacket(int x, int z,
/// ClientboundLightUpdatePacketData lightData)`. The wire body for
/// the chunk coordinates is a pair of VarInts (matching vanilla's
/// `STREAM_CODEC`), not raw `i32`s — distinct from
/// `LevelChunkWithLight` which writes the coordinates as `i32`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightUpdate {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub light: LightData,
}

impl Packet for LightUpdate {
    // Verified via `javap` of vanilla 26.1.2's GameProtocols:
    // CLIENTBOUND_LIGHT_UPDATE at game-CB index 48 = wire id 0x30.
    const ID: i32 = 0x31;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.chunk_x);
        buf.write_varint(self.chunk_z);
        self.light.encode(buf)?;
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            chunk_x: buf.read_varint()?,
            chunk_z: buf.read_varint()?,
            light: LightData::decode(buf)?,
        })
    }
}

// ---------------------------------------------------------------------
// M5.a — serverbound interaction packets
