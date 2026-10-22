//! Chunk → wire-payload conversion for the `LevelChunkWithLight`
//! packet. Lives in `mc-world` (and not `mc-protocol`) because the
//! conversion needs `Chunk` / `ChunkSection` / `BiomeSection`
//! internals; in exchange, the protocol crate stays unaware of the
//! world model.
//!
//! Two outputs:
//!
//! - [`encode_chunk_data`] — the section-by-section paletted-
//!   container blob that fills `LevelChunkWithLight.data`.
//! - [`client_heightmaps`] — the `(type_id, long[])` pairs that
//!   make up `LevelChunkWithLight.heightmaps`.
//!
//! Wire layout (per
//! `net.minecraft.world.level.chunk.LevelChunkSection.write`,
//! ADR 0002 javap):
//!
//! ```text
//! for each of 24 sections, concatenated:
//!     i16  non_air_block_count    (vanilla's `nonEmptyBlockCount`)
//!     i16  fluid_count            (states with non-empty fluid state)
//!     PalettedContainer<BlockState>:
//!         u8       bits_per_entry
//!         Palette  (see below)
//!         i64[N]   raw packed entries, no length prefix
//!                  (`writeFixedSizeLongArray`)
//!     PalettedContainer<Biome>:    same shape, smaller bpe range
//! ```
//!
//! Vanilla's palette dispatch by `bits_per_entry` (verified via
//! `Strategy.getConfigurationForBitCount`):
//!
//! ```text
//!  bpe   block strategy            biome strategy
//!  0     SingleValuePalette        SingleValuePalette
//!  1..3  (blocks pad to 4)         LinearPalette (1 / 2 / 3 bits)
//!  4     LinearPalette             — (uses direct above 3)
//!  5..8  HashMapPalette            — (uses direct above 3)
//!  ≥ 9   GlobalPalette (direct)    GlobalPalette (direct, ≥ 4 for biomes)
//! ```
//!
//! Wire shapes per palette kind:
//!
//! ```text
//! SingleValuePalette : VarInt single_entry
//! LinearPalette      : VarInt size, VarInt[size] entries
//! HashMapPalette     : VarInt size, VarInt[size] entries
//! GlobalPalette      : (nothing — entries are raw global ids
//!                       in the long[])
//! ```
//!
//! The network storage is vanilla `SimpleBitStorage`: entries do not
//! cross `i64` boundaries, so each word may have unused high bits when
//! `64 % bits_per_entry != 0`.

use std::io::Cursor;

use mc_data::items::ItemRegistry;
use mc_data::{Identifier, Registry};
use mc_nbt::{ListTag, Tag, tag_type};
use thiserror::Error;

use crate::block::BlockRegistry;
use crate::chunk::{
    BIOME_VOLUME, BiomeSection, BlockPos, ChestBlockEntity, Chunk, FurnaceBlockEntity, FurnaceSlot,
    LIGHT_LAYER_BYTES, SECTION_COUNT,
};
use crate::light::{ChunkLight, LightLayer};
use crate::section::SECTION_DIM;
use crate::section::{ChunkSection, PackedBitArray, SECTION_VOLUME};

/// Bits per entry vanilla 26.1.2 expects in `GlobalPalette` (direct)
/// mode for the block-state container. Sized to comfortably hold the
/// 29 873 states the bundled jar enumerates
/// (`ceil(log2(29 873)) = 15`); when the registry grows in a future
/// patch the encoder uses the wider of this constant and the actual
/// ceiling computed from the registry size.
const DIRECT_BITS: u8 = 15;
/// Above this bit-width the wire switches from the LinearPalette /
/// HashMapPalette indirect formats to GlobalPalette (direct). Matches
/// vanilla's threshold (`Strategy.getConfigurationForBitCount`).
const DIRECT_BITS_THRESHOLD: u8 = 9;
const BIOME_DIRECT_BITS_THRESHOLD: u8 = 4;

/// Heightmap type ids — the ordinals of `Heightmap$Types` in the
/// vanilla source, verified via `javap -p -c` (ADR 0002). Only the
/// three entries with `Usage.CLIENT` are sent on the wire.
pub mod heightmap_type {
    pub const WORLD_SURFACE: i32 = 1;
    pub const MOTION_BLOCKING: i32 = 4;
    pub const MOTION_BLOCKING_NO_LEAVES: i32 = 5;
}

/// Things that can go wrong producing a `LevelChunkWithLight` body.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WireError {
    /// A biome identifier in the chunk's biome palette is not in the
    /// supplied registry. Indicates a registry / world mismatch (e.g.
    /// the world was generated against a newer datapack); we surface
    /// it instead of silently substituting.
    #[error("biome {0} not present in the supplied registry")]
    UnknownBiome(Identifier),
}

/// One heightmap entry ready for `LevelChunkWithLight.heightmaps`.
/// Mirrors the structure of `mc_protocol::packets::play::ChunkHeightmap`
/// without depending on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightmapEntry {
    pub type_id: i32,
    pub data: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntityWireRecord {
    pub pos: BlockPos,
    pub type_name: Identifier,
    pub nbt: Tag,
}

pub fn client_block_entities(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: &ItemRegistry,
) -> Vec<BlockEntityWireRecord> {
    let mut records = Vec::new();
    let mut opaque_entries: Vec<_> = chunk
        .block_entities
        .iter()
        .filter(|(pos, _)| !chunk.furnaces.contains_key(pos) && !chunk.chests.contains_key(pos))
        .collect();
    opaque_entries.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (pos, bytes) in opaque_entries {
        let mut cur = Cursor::new(bytes.as_slice());
        let Ok(tag) = mc_nbt::read_network(&mut cur) else {
            continue;
        };
        let Some(type_name) = block_entity_type_name(&tag) else {
            continue;
        };
        records.push(BlockEntityWireRecord {
            pos: *pos,
            type_name,
            nbt: strip_persistent_block_entity_fields(tag),
        });
    }

    let mut furnaces: Vec<_> = chunk.furnaces.iter().collect();
    furnaces.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (pos, furnace) in furnaces {
        let type_name = furnace_block_entity_type_for_block(chunk, registry, *pos);
        records.push(BlockEntityWireRecord {
            pos: *pos,
            type_name,
            nbt: furnace_update_tag(furnace, items),
        });
    }

    let mut chests: Vec<_> = chunk.chests.iter().collect();
    chests.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (pos, chest) in chests {
        let type_name = chest_block_entity_type_for_block(chunk, registry, *pos);
        records.push(BlockEntityWireRecord {
            pos: *pos,
            type_name,
            nbt: chest_update_tag(chest, items),
        });
    }

    // Vanilla renders beds exclusively through a block entity, but the bed
    // block entity itself carries no payload (`BedBlockEntity` saves only the
    // id/position metadata): the colour lives in the block id (`white_bed` …
    // `black_bed`), so there is nothing to derive from the biome and no white
    // default to fall back to — every bed half found in the sections gets an
    // empty update-tag record. Halves that already carry a stored block
    // entity (e.g. an Anvil import) are skipped, and furnace/chest records
    // above keep their positions, so no position is emitted twice.

    let mut beds: Vec<_> = bed_block_positions(chunk, registry)
        .into_iter()
        .filter(|pos| !records.iter().any(|record| record.pos == *pos))
        .collect();
    beds.sort_by_key(|pos| (pos.x, pos.y, pos.z));
    for pos in beds {
        records.push(BlockEntityWireRecord {
            pos,
            type_name: Identifier::parse("minecraft:bed").expect("static bed identifier"),
            nbt: Tag::Compound(Vec::new()),
        });
    }

    records
}

/// Build the CLIENT-usage heightmaps from a chunk in the order
/// vanilla emits them (`WORLD_SURFACE`, `MOTION_BLOCKING`,
/// `MOTION_BLOCKING_NO_LEAVES`). Entries are skipped silently when
/// the chunk has no heightmap of that name — vanilla rejects unknown
/// keys but happily accepts a subset.
#[must_use]
pub fn client_heightmaps(chunk: &Chunk) -> Vec<HeightmapEntry> {
    const ORDER: [(&str, i32); 3] = [
        ("WORLD_SURFACE", heightmap_type::WORLD_SURFACE),
        ("MOTION_BLOCKING", heightmap_type::MOTION_BLOCKING),
        (
            "MOTION_BLOCKING_NO_LEAVES",
            heightmap_type::MOTION_BLOCKING_NO_LEAVES,
        ),
    ];
    ORDER
        .iter()
        .filter_map(|(name, type_id)| {
            chunk.heightmaps.get(*name).map(|h| HeightmapEntry {
                type_id: *type_id,
                data: h.to_long_array(),
            })
        })
        .collect()
}

fn block_entity_type_name(tag: &Tag) -> Option<Identifier> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    fields.iter().find_map(|(key, value)| {
        if key == "id"
            && let Tag::String(id) = value
        {
            Identifier::parse(id.clone()).ok()
        } else {
            None
        }
    })
}

fn strip_persistent_block_entity_fields(tag: Tag) -> Tag {
    let Tag::Compound(fields) = tag else {
        return tag;
    };
    Tag::Compound(
        fields
            .into_iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "id" | "x"
                        | "y"
                        | "z"
                        | "CookingTimes"
                        | "CookingTotalTimes"
                        | "solaris_cooking_remaining"
                        | "solaris_cooking_total"
                )
            })
            .collect(),
    )
}

fn furnace_block_entity_type_for_block(
    chunk: &Chunk,
    registry: &BlockRegistry,
    pos: BlockPos,
) -> Identifier {
    let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
    let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
    use mc_data::block_entity_kinds::BlockEntityKind;
    let kind = chunk
        .get_block(local_x, pos.y, local_z)
        .and_then(|state_id| registry.by_id(state_id))
        .and_then(|state| BlockEntityKind::for_block(state.block.id.path()))
        .filter(|kind| {
            matches!(
                kind,
                BlockEntityKind::Furnace | BlockEntityKind::Smoker | BlockEntityKind::BlastFurnace
            )
        })
        .unwrap_or(BlockEntityKind::Furnace);
    Identifier::parse(kind.vanilla_id()).unwrap()
}

fn chest_block_entity_type_for_block(
    chunk: &Chunk,
    registry: &BlockRegistry,
    pos: BlockPos,
) -> Identifier {
    let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
    let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
    use mc_data::block_entity_kinds::BlockEntityKind;
    let kind = chunk
        .get_block(local_x, pos.y, local_z)
        .and_then(|state_id| registry.by_id(state_id))
        .and_then(|state| BlockEntityKind::for_block(state.block.id.path()))
        .filter(|kind| {
            matches!(
                kind,
                BlockEntityKind::Chest | BlockEntityKind::TrappedChest | BlockEntityKind::Barrel
            )
        })
        .unwrap_or(BlockEntityKind::Chest);
    Identifier::parse(kind.vanilla_id()).unwrap()
}

fn furnace_update_tag(furnace: &FurnaceBlockEntity, items: &ItemRegistry) -> Tag {
    Tag::Compound(vec![
        ("Items".into(), item_list_tag(&furnace.slots, items)),
        (
            "lit_time_remaining".into(),
            Tag::Short(furnace.burn_remaining),
        ),
        ("lit_total_time".into(), Tag::Short(furnace.burn_total)),
        (
            "cooking_time_spent".into(),
            Tag::Short(furnace.cook_progress),
        ),
        ("cooking_total_time".into(), Tag::Short(furnace.cook_total)),
    ])
}

fn chest_update_tag(chest: &ChestBlockEntity, items: &ItemRegistry) -> Tag {
    Tag::Compound(vec![("Items".into(), item_list_tag(&chest.slots, items))])
}

/// Absolute positions of every bed half in the chunk, section by section.
///
/// Sections whose palette holds no bed state are skipped without touching
/// their packed storage, so chunks without beds pay one small palette scan.
fn bed_block_positions(chunk: &Chunk, registry: &BlockRegistry) -> Vec<BlockPos> {
    let is_bed_state = |state: crate::block::BlockStateId| {
        registry
            .by_id(state)
            .is_some_and(|state| state.block.id.path().ends_with("_bed"))
    };
    let min_y = chunk.geometry().min_y();
    let mut beds = Vec::new();
    for (section_index, section) in chunk.sections.iter().enumerate() {
        let contains_bed = match section.palette() {
            Some(palette) => palette.iter().copied().any(is_bed_state),
            None => is_bed_state(section.get(0, 0, 0)),
        };
        if !contains_bed {
            continue;
        }
        let base_y = min_y + (section_index as i32) * SECTION_DIM as i32;
        for sy in 0..SECTION_DIM as u8 {
            for sz in 0..SECTION_DIM as u8 {
                for sx in 0..SECTION_DIM as u8 {
                    if is_bed_state(section.get(sx, sy, sz)) {
                        beds.push(BlockPos {
                            x: chunk.pos.x * SECTION_DIM as i32 + i32::from(sx),
                            y: base_y + i32::from(sy),
                            z: chunk.pos.z * SECTION_DIM as i32 + i32::from(sz),
                        });
                    }
                }
            }
        }
    }
    beds
}

fn item_list_tag(slots: &[FurnaceSlot], items: &ItemRegistry) -> Tag {
    let item_tags = slots
        .iter()
        .enumerate()
        .filter_map(|(slot, stack)| {
            if stack.is_empty() {
                return None;
            }
            let name = items.name_of(stack.item_id)?;
            Some(Tag::Compound(vec![
                ("Slot".into(), Tag::Int(slot as i32)),
                ("id".into(), Tag::String(name.as_str().to_string())),
                ("count".into(), Tag::Int(stack.count)),
            ]))
        })
        .collect::<Vec<_>>();
    Tag::List(ListTag {
        element_type: if item_tags.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements: item_tags,
    })
}

/// Encode all sections of `chunk` into the paletted-container blob
/// that fills `LevelChunkWithLight.data`.
///
/// `biomes` is the `worldgen/biome` registry; the chunk's biome
/// palette stores identifiers and the wire format needs numeric
/// registry indices, so the registry has to be supplied.
pub fn encode_chunk_data(
    chunk: &Chunk,
    biomes: &Registry,
    blocks: &BlockRegistry,
) -> Result<Vec<u8>, WireError> {
    encode_chunk_data_with_block_projection(chunk, biomes, blocks, &|state| state)
}

/// Encode chunk data after projecting server-owned block states into the state
/// ids understood by one client.
pub fn encode_chunk_data_with_block_projection(
    chunk: &Chunk,
    biomes: &Registry,
    blocks: &BlockRegistry,
    project: &dyn Fn(crate::block::BlockStateId) -> crate::block::BlockStateId,
) -> Result<Vec<u8>, WireError> {
    encode_chunk_data_inner(chunk, biomes, blocks, project, &[])
}

/// Encode chunk data with some sections replaced on the wire (anti-xray hides
/// ores in copies; the authoritative chunk is untouched). `overrides[i]`, when
/// present, stands in for section `i`; an empty or shorter slice leaves the
/// remaining sections as stored. A replacement must keep the section's
/// non-air and fluid counts (it only swaps solid blocks for solid blocks).
pub fn encode_chunk_data_with_sections(
    chunk: &Chunk,
    biomes: &Registry,
    blocks: &BlockRegistry,
    overrides: &[Option<ChunkSection>],
) -> Result<Vec<u8>, WireError> {
    encode_chunk_data_inner(chunk, biomes, blocks, &|state| state, overrides)
}

fn encode_chunk_data_inner(
    chunk: &Chunk,
    biomes: &Registry,
    blocks: &BlockRegistry,
    project: &dyn Fn(crate::block::BlockStateId) -> crate::block::BlockStateId,
    overrides: &[Option<ChunkSection>],
) -> Result<Vec<u8>, WireError> {
    debug_assert_eq!(chunk.sections.len(), chunk.geometry().section_count());
    debug_assert_eq!(chunk.biomes.len(), chunk.geometry().section_count());
    let mut buf = Vec::with_capacity(chunk.geometry().section_count() * 16);
    for (index, (stored, bsec)) in chunk.sections.iter().zip(chunk.biomes.iter()).enumerate() {
        let sec = overrides
            .get(index)
            .and_then(Option::as_ref)
            .unwrap_or(stored);
        // Both section counters are big-endian on the wire. The client uses
        // fluid_count as a fast gate before scanning entity/fluid overlap.
        buf.extend_from_slice(&(sec.non_air_count() as i16).to_be_bytes());
        buf.extend_from_slice(&(section_fluid_count(sec, blocks) as i16).to_be_bytes());
        encode_block_palette(&mut buf, sec, project);
        encode_biome_palette(&mut buf, bsec, biomes)?;
    }
    Ok(buf)
}

fn section_fluid_count(section: &ChunkSection, blocks: &BlockRegistry) -> u16 {
    let Some(palette) = section.palette() else {
        return if blocks
            .by_id(section.get(0, 0, 0))
            .is_some_and(crate::block::BlockState::has_fluid)
        {
            SECTION_VOLUME as u16
        } else {
            0
        };
    };
    let indices = section
        .indices()
        .expect("indirect section carries packed palette indices");
    let has_fluid = |state| {
        blocks
            .by_id(state)
            .is_some_and(crate::block::BlockState::has_fluid)
    };
    if !palette.iter().copied().any(has_fluid) {
        return 0;
    }
    let fluid_palette = palette.iter().copied().map(has_fluid).collect::<Vec<_>>();
    (0..indices.len())
        .filter(|&index| fluid_palette[indices.get(index) as usize])
        .count() as u16
}

fn encode_block_palette(
    buf: &mut Vec<u8>,
    section: &ChunkSection,
    project: &dyn Fn(crate::block::BlockStateId) -> crate::block::BlockStateId,
) {
    match (section.palette(), section.indices()) {
        (None, _) => {
            // Section is in Single mode — vanilla's
            // SingleValuePalette: bpe=0, VarInt single state id, no
            // backing storage.
            //
            // Single sections, including admitted one-entry palettes,
            // expose their uniform state through `get(0, 0, 0)`.
            buf.push(0);
            write_varint(
                buf,
                i32::try_from(project(section.get(0, 0, 0)).0).expect("state id < i32::MAX"),
            );
            // No bit-storage longs.
        }
        (Some(palette), Some(indices)) => {
            if indices.bits_per_entry() >= DIRECT_BITS_THRESHOLD {
                encode_block_direct(buf, palette, indices, project);
            } else {
                let bits = indices.bits_per_entry().max(4);
                buf.push(bits);
                write_varint(
                    buf,
                    i32::try_from(palette.len()).expect("palette len < i32::MAX"),
                );
                for state in palette {
                    write_varint(
                        buf,
                        i32::try_from(project(*state).0).expect("state id < i32::MAX"),
                    );
                }
                for word in indices.words_at_bits(bits) {
                    buf.extend_from_slice(&(word as i64).to_be_bytes());
                }
            }
        }
        (Some(_), None) => unreachable!("indirect sections always have indices"),
    }
}

/// Emit a section's block container in vanilla's `GlobalPalette`
/// (direct) shape: a single `bits_per_entry` byte, *no* palette
/// section, then a packed long array containing every cell's raw
/// global state-id at [`DIRECT_BITS`] per entry. Triggered when our
/// internal palette has grown past the indirect threshold
/// (~256 entries → ≥ 9 bits per palette index).
fn encode_block_direct(
    buf: &mut Vec<u8>,
    palette: &[crate::block::BlockStateId],
    indices: &PackedBitArray,
    project: &dyn Fn(crate::block::BlockStateId) -> crate::block::BlockStateId,
) {
    let bits = DIRECT_BITS.max(indices.bits_per_entry());
    let direct = pack_fixed_longs(bits, SECTION_VOLUME, |cell| {
        let p = indices.get(cell) as usize;
        project(palette[p]).0
    });
    buf.push(bits);
    // GlobalPalette: no palette VarInts.
    for word in direct {
        buf.extend_from_slice(&(word as i64).to_be_bytes());
    }
}

fn encode_biome_palette(
    buf: &mut Vec<u8>,
    section: &BiomeSection,
    registry: &Registry,
) -> Result<(), WireError> {
    match section {
        BiomeSection::Single(biome) => {
            let id = registry_index_of(registry, biome)?;
            buf.push(0);
            write_varint(buf, id);
        }
        BiomeSection::Indirect { palette, indices } => {
            debug_assert_eq!(indices.len(), BIOME_VOLUME);
            if indices.bits_per_entry() >= BIOME_DIRECT_BITS_THRESHOLD {
                let direct_bits = bits_for_distinct_values(registry.entries.len());
                let palette_ids = palette
                    .iter()
                    .map(|biome| registry_index_of(registry, biome).map(|id| id as u32))
                    .collect::<Result<Vec<_>, _>>()?;
                buf.push(direct_bits);
                for word in pack_fixed_longs(direct_bits, indices.len(), |idx| {
                    let p = indices.get(idx) as usize;
                    palette_ids[p]
                }) {
                    buf.extend_from_slice(&(word as i64).to_be_bytes());
                }
            } else {
                buf.push(indices.bits_per_entry());
                write_varint(
                    buf,
                    i32::try_from(palette.len()).expect("palette len < i32::MAX"),
                );
                for biome in palette {
                    write_varint(buf, registry_index_of(registry, biome)?);
                }
                for word in pack_fixed_longs(indices.bits_per_entry(), indices.len(), |idx| {
                    indices.get(idx)
                }) {
                    buf.extend_from_slice(&(word as i64).to_be_bytes());
                }
            }
        }
    }
    Ok(())
}

fn pack_fixed_longs<F>(bits_per_entry: u8, len: usize, mut value_at: F) -> Vec<u64>
where
    F: FnMut(usize) -> u32,
{
    let bits = bits_per_entry as usize;
    debug_assert!((1..=32).contains(&bits_per_entry));
    let entries_per_word = (64 / bits).max(1);
    let mask = (1u64 << bits) - 1;
    let mut words = vec![0u64; len.div_ceil(entries_per_word)];
    for idx in 0..len {
        let value = value_at(idx) as u64 & mask;
        let word_index = idx / entries_per_word;
        let bit_offset = (idx % entries_per_word) * bits;
        words[word_index] |= value << bit_offset;
    }
    words
}

fn bits_for_distinct_values(len: usize) -> u8 {
    if len <= 1 {
        return 1;
    }
    (len - 1).ilog2() as u8 + 1
}

fn registry_index_of(registry: &Registry, biome: &Identifier) -> Result<i32, WireError> {
    registry
        .entries
        .iter()
        .position(|e| e == biome)
        .map(|p| p as i32)
        .ok_or_else(|| WireError::UnknownBiome(biome.clone()))
}

// ---------------------------------------------------------------------
// Light wire encoding (M4.d)
// ---------------------------------------------------------------------

/// Wire-ready light payload for one chunk. Mirrors the shape of
/// `mc_protocol::packets::play::LightData` so the edge can lift fields straight
/// through without translation. Held here (and not in
/// `mc-protocol`) so the conversion from `ChunkLight` doesn't drag
/// world-model types into the protocol crate, matching the same
/// posture `encode_chunk_data` / `client_heightmaps` already establish.
///
/// Y-slot indexing contains one slab below the world, all in-world
/// sections, and one slab above the world. Slot `i` corresponds to
/// in-world chunk section index `i - 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightWire {
    pub sky_y_mask: Vec<i64>,
    pub block_y_mask: Vec<i64>,
    pub empty_sky_y_mask: Vec<i64>,
    pub empty_block_y_mask: Vec<i64>,
    pub sky_updates: Vec<Vec<u8>>,
    pub block_updates: Vec<Vec<u8>>,
}

/// Total number of wire light slots (one below + 24 in-world + one
/// above).
pub const WIRE_LIGHT_SECTIONS: usize = SECTION_COUNT + 2;

/// Pack a per-chunk computed [`ChunkLight`] into the wire payload
/// `LevelChunkWithLight` expects. Always emits every light slot: each
/// section is either listed in the `*_y_mask` with its 2048-byte
/// nibble layer, or in the `empty_*_y_mask`. The below-world slab is
/// emitted empty for both channels; the above-world slab is emitted
/// as `sky=15` (open sky) and `block=0` (empty for block channel).
#[must_use]
pub fn encode_chunk_light(light: &ChunkLight) -> LightWire {
    let section_count = light.section_count();
    let wire_light_sections = section_count + 2;
    assert!(wire_light_sections <= u64::BITS as usize);
    let mut sky_mask: u64 = 0;
    let mut block_mask: u64 = 0;
    let mut empty_sky_mask: u64 = 0;
    let mut empty_block_mask: u64 = 0;
    let mut sky_updates: Vec<Vec<u8>> = Vec::new();
    let mut block_updates: Vec<Vec<u8>> = Vec::new();

    // Slot 0 (Y=-5, below world): empty for both channels.
    empty_sky_mask |= 1 << 0;
    empty_block_mask |= 1 << 0;

    // In-world sections occupy slots 1..=section_count.
    for section_idx in 0..section_count {
        let slot = section_idx + 1;
        let sky_layer = pack_section_layer(&light.sky, section_idx);
        let block_layer = pack_section_layer(&light.block, section_idx);
        if let Some(sky_layer) = sky_layer {
            sky_mask |= 1 << slot;
            sky_updates.push(sky_layer);
        } else {
            empty_sky_mask |= 1 << slot;
        }
        if let Some(block_layer) = block_layer {
            block_mask |= 1 << slot;
            block_updates.push(block_layer);
        } else {
            empty_block_mask |= 1 << slot;
        }
    }

    // The slot above the world is sky=15 (open sky), block empty.
    let top_slot = section_count + 1;
    sky_mask |= 1 << top_slot;
    sky_updates.push(vec![0xFF; LIGHT_LAYER_BYTES]);
    empty_block_mask |= 1 << top_slot;

    LightWire {
        sky_y_mask: vec![sky_mask as i64],
        block_y_mask: vec![block_mask as i64],
        empty_sky_y_mask: vec![empty_sky_mask as i64],
        empty_block_y_mask: vec![empty_block_mask as i64],
        sky_updates,
        block_updates,
    }
}

/// Expand nonzero light at the wire boundary. Known zero sections use the
/// empty mask rather than a redundant array.
fn pack_section_layer(channel: &LightLayer, section_idx: usize) -> Option<Vec<u8>> {
    let layer = channel.section(section_idx)?;
    (!layer.is_zero()).then(|| layer.to_vec())
}

fn write_varint(buf: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        if v & !0x7F == 0 {
            buf.push(v as u8);
            return;
        }
        buf.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

// -------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------

#[cfg(test)]
#[path = "wire_palette_tests.rs"]
mod palette_tests;

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
