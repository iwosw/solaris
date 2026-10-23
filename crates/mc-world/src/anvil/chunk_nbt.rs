//! Anvil chunk schema ↔ [`Chunk`] translation.
//!
//! Decodes the chunk NBT compound vanilla 26.1 writes into a `Chunk`,
//! and emits the supported normalized Anvil shape back. Typed fields are
//! modelled where runtime code consumes them. Root-level fields outside
//! the modelled set are kept in `Chunk::extras` and re-emitted on save
//! when possible. The modelled subset round-trips losslessly:
//!
//! - block states (palette + packed indices)
//! - biomes (palette + packed indices)
//! - heightmaps (long-array view preserved)
//! - opaque block entities (stored in `Chunk::block_entities` and
//!   byte-identical on round-trip); typed container entities are normalized
//!   through runtime models, accepting vanilla byte and existing Solaris int `Slot` tags
//! - scheduled block and fluid ticks
//! - Status string, xPos / zPos
//! - per-section `BlockLight` / `SkyLight` nibble arrays (decoded
//!   into `Chunk::section_lights` and re-emitted when present)
//!
//! Additionally, unmodelled root-level extras such as structures,
//! PostProcessing, InhabitedTime, LastUpdate, and DataVersion are
//! best-effort preserved verbatim.

use std::io::Cursor;

use mc_data::Identifier;
use mc_data::items::ItemRegistry;
use mc_data::loot::chest_26_1_2::ChestLootCatalog;
use mc_nbt::{ListTag, Tag, tag_type};
use thiserror::Error;

use crate::SECTION_DIM;
use crate::anvil::ChunkPayload;
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{
    BIOME_VOLUME, BiomeSection, BlockPos, BrewingStandBlockEntity, ChestBlockEntity, Chunk,
    ChunkGeometry, ChunkPos, FurnaceBlockEntity, FurnaceSlot, Heightmap, HopperBlockEntity,
    LIGHT_LAYER_BYTES, ScheduledBlockTick, ScheduledFluidTick, SectionLight,
};
use crate::section::{ChunkSection, PackedBitArray, SECTION_VOLUME};

const REGION_AXIS_CHUNKS: i32 = 32;
/// Vanilla `DataVersion` stamped on every chunk we write (26.1.2 world
/// version; shared with the entity-region writer).
pub const TARGET_DATA_VERSION: i32 = 4_790;
const VANILLA_METADATA_KEYS: &[&str] = &["DataVersion", "LastUpdate", "InhabitedTime"];

/// Serialise a chunk to a [`ChunkPayload`] ready for
/// [`write_region`](crate::anvil::write_region). Used by the M6.b
/// persistence flush path: encodes the chunk to NBT, serialises with
/// `mc_nbt::write_named` (Anvil chunks are stored as an unnamed root,
/// `name = ""`), and bundles with the chunk's region-local slot
/// coordinates plus the supplied epoch-seconds timestamp.
pub fn chunk_to_payload(
    chunk: &Chunk,
    registry: &BlockRegistry,
    timestamp: u32,
) -> Result<ChunkPayload, ChunkNbtError> {
    chunk_to_payload_with_items(chunk, registry, None, timestamp)
}

pub fn chunk_to_payload_with_items(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    timestamp: u32,
) -> Result<ChunkPayload, ChunkNbtError> {
    chunk_to_payload_with_items_at_tick(chunk, registry, items, timestamp, 0)
}

pub fn chunk_to_payload_with_items_at_tick(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    timestamp: u32,
    current_tick: u64,
) -> Result<ChunkPayload, ChunkNbtError> {
    chunk_to_payload_with_items_at_tick_for_position(
        chunk,
        chunk.pos,
        registry,
        items,
        timestamp,
        current_tick,
    )
}

/// Serialize a chunk for one externally owned cache/region key.
///
/// Persistence callers must supply the expected absolute position separately so
/// a corrupted resident snapshot cannot select a different region slot through
/// its internal `Chunk::pos`.
pub fn chunk_to_payload_with_items_at_tick_for_position(
    chunk: &Chunk,
    expected_pos: ChunkPos,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    timestamp: u32,
    current_tick: u64,
) -> Result<ChunkPayload, ChunkNbtError> {
    ensure_chunk_position(expected_pos, chunk.pos)?;
    let nbt = chunk_to_nbt_with_items_at_tick(chunk, registry, items, current_tick)?;
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    mc_nbt::write_named(&mut buf, "", &nbt)?;
    let local_x = expected_pos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
    let local_z = expected_pos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;
    Ok(ChunkPayload {
        local_x,
        local_z,
        timestamp,
        uncompressed_nbt: buf,
    })
}

#[derive(Debug, Error)]
pub enum ChunkNbtError {
    #[error("field {field} has integral value {value} outside the supported Short range")]
    NumericOutOfRange { field: &'static str, value: i64 },
    #[error("NBT error: {0}")]
    Nbt(#[from] mc_nbt::NbtError),
    #[error("expected root Compound, got tag {0:#x}")]
    NotCompound(u8),
    #[error("missing required field {0}")]
    MissingField(&'static str),
    #[error("field {field} has wrong type: expected {expected}, got tag {got:#x}")]
    WrongType {
        field: &'static str,
        expected: &'static str,
        got: u8,
    },
    #[error("invalid identifier {0:?}")]
    InvalidIdentifier(String),
    #[error("block state {name}[{props:?}] not in registry")]
    UnknownBlockState {
        name: String,
        props: Vec<(String, String)>,
    },
    #[error("block state id {0} not in registry")]
    UnknownBlockStateId(u32),
    #[error("item {0} not in registry")]
    UnknownItem(String),
    #[error("invalid storage block entity: {0}")]
    InvalidStorageBlockEntity(String),
    #[error("invalid enchantment component {0:?}")]
    InvalidEnchantment(String),
    #[error("section Y={0} is outside the authoritative chunk geometry")]
    SectionOutOfRange(i32),
    #[error("section Y={0} is duplicated or leaves a gap in the chunk geometry")]
    InvalidSectionShape(i32),
    #[error(
        "yPos={y_pos} does not match the authoritative chunk geometry ({section_count} sections)"
    )]
    InvalidChunkGeometry { y_pos: i32, section_count: usize },
    #[error("section Y={0} does not fit the vanilla byte field")]
    SectionYOutOfByteRange(i32),
    #[error("packed bit-array length mismatch: expected {expected} words, got {got}")]
    PackedWordMismatch { expected: usize, got: usize },
    #[error("{field} has wrong byte length: expected {expected}, got {got}")]
    LightLengthMismatch {
        field: &'static str,
        expected: usize,
        got: usize,
    },
    #[error("scheduled tick delay {0} does not fit in Anvil int")]
    TickDelayOutOfRange(i128),
    #[error(
        "chunk position mismatch: expected ({expected_x},{expected_z}), decoded/held ({actual_x},{actual_z})"
    )]
    PositionMismatch {
        expected_x: i32,
        expected_z: i32,
        actual_x: i32,
        actual_z: i32,
    },
    #[error("trailing bytes after complete chunk NBT root: {trailing}")]
    TrailingNbtBytes { trailing: usize },
    #[error("{0}")]
    InvalidBlockEntity(String),
}

// ---------------------------------------------------------------------
// Decode
// ---------------------------------------------------------------------

pub fn chunk_from_nbt(
    nbt: &Tag,
    registry: &BlockRegistry,
    geometry: ChunkGeometry,
) -> Result<Chunk, ChunkNbtError> {
    chunk_from_nbt_with_items(nbt, registry, None, &ChestLootCatalog::new(), geometry)
}

pub fn chunk_from_nbt_with_items(
    nbt: &Tag,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    chest_loot: &ChestLootCatalog,
    geometry: ChunkGeometry,
) -> Result<Chunk, ChunkNbtError> {
    chunk_from_nbt_with_items_impl(nbt, registry, items, chest_loot, None, geometry, 0)
}

/// Decode positioned Anvil NBT with signed scheduled delays based on its world clock.
pub fn chunk_from_nbt_with_items_at_position_at_tick(
    nbt: &Tag,
    expected_pos: ChunkPos,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    chest_loot: &ChestLootCatalog,
    geometry: ChunkGeometry,
    current_tick: u64,
) -> Result<Chunk, ChunkNbtError> {
    chunk_from_nbt_with_items_impl(
        nbt,
        registry,
        items,
        chest_loot,
        Some(expected_pos),
        geometry,
        current_tick,
    )
}

/// Parse one exact positioned Anvil NBT payload using the persisted world clock.
pub fn chunk_from_payload_with_items_at_position_at_tick(
    bytes: &[u8],
    expected_pos: ChunkPos,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    chest_loot: &ChestLootCatalog,
    geometry: ChunkGeometry,
    current_tick: u64,
) -> Result<Chunk, ChunkNbtError> {
    let mut cursor = Cursor::new(bytes);
    let (_, root) = mc_nbt::read_named(&mut cursor)?;
    let consumed = usize::try_from(cursor.position())
        .expect("cursor position over an in-memory slice fits usize");
    if consumed != bytes.len() {
        return Err(ChunkNbtError::TrailingNbtBytes {
            trailing: bytes.len() - consumed,
        });
    }
    chunk_from_nbt_with_items_at_position_at_tick(
        &root,
        expected_pos,
        registry,
        items,
        chest_loot,
        geometry,
        current_tick,
    )
}

fn chunk_from_nbt_with_items_impl(
    nbt: &Tag,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    chest_loot: &ChestLootCatalog,
    expected_pos: Option<ChunkPos>,
    geometry: ChunkGeometry,
    current_tick: u64,
) -> Result<Chunk, ChunkNbtError> {
    let root = expect_compound(nbt, "root")?;

    let x = get_int(root, "xPos")?;
    let z = get_int(root, "zPos")?;
    let actual_pos = ChunkPos { x, z };
    if let Some(expected_pos) = expected_pos {
        ensure_chunk_position(expected_pos, actual_pos)?;
    }
    let status = get_string(root, "Status")?.to_string();

    let air = registry
        .block(&id("minecraft:air"))
        .map(|b| b.default)
        .unwrap_or(BlockStateId(0));
    let default_biome = id("minecraft:plains");

    let y_pos =
        get_optional_section_y(root, "yPos")?.unwrap_or(geometry.min_y() / SECTION_DIM as i32);
    // Vanilla's SerializableChunkData treats absent/sparse sections as empty.
    let sections = get_optional_list(root, "sections")?;
    let section_count = geometry.section_count();
    if y_pos != geometry.min_y() / SECTION_DIM as i32 {
        return Err(ChunkNbtError::InvalidChunkGeometry {
            y_pos,
            section_count,
        });
    }

    let mut chunk = Chunk::empty_with_geometry(ChunkPos { x, z }, air, default_biome, geometry);
    chunk.status = status;

    let mut seen_sections = vec![false; section_count];
    let mut seen_boundaries = [false; 2];
    for s in sections.into_iter().flat_map(|list| &list.elements) {
        let cmp = expect_compound(s, "sections[]")?;
        let y = get_section_y(cmp, "Y")?;
        let Some(idx) = y
            .checked_sub(y_pos)
            .and_then(|idx| usize::try_from(idx).ok())
            .filter(|idx| *idx < section_count)
        else {
            let boundary = if y == y_pos - 1 {
                0
            } else if y == y_pos + section_count as i32 {
                1
            } else {
                return Err(ChunkNbtError::SectionOutOfRange(y));
            };
            if cmp
                .iter()
                .any(|(name, _)| name == "block_states" || name == "biomes")
            {
                return Err(ChunkNbtError::SectionOutOfRange(y));
            }
            if seen_boundaries[boundary] {
                return Err(ChunkNbtError::InvalidSectionShape(y));
            }
            seen_boundaries[boundary] = true;
            chunk.boundary_section_lights[boundary] = decode_section_light(cmp)?;
            continue;
        };
        if seen_sections[idx] {
            return Err(ChunkNbtError::InvalidSectionShape(y));
        }
        seen_sections[idx] = true;
        if let Some(bs) = get_optional_compound(cmp, "block_states")? {
            chunk.sections[idx] = decode_block_section(bs, registry, air)?;
        }
        if let Some(bi) = get_optional_compound(cmp, "biomes")? {
            chunk.biomes[idx] = decode_biome_section(bi)?;
        }
        chunk.section_lights[idx] = decode_section_light(cmp)?;
    }

    if let Some(hms) = get_optional_compound(root, "Heightmaps")? {
        for (name, tag) in hms {
            if let Tag::LongArray(longs) = tag {
                chunk
                    .heightmaps
                    .insert(name.clone(), Heightmap::from_long_array(longs));
            } else {
                return Err(ChunkNbtError::WrongType {
                    field: "Heightmaps.*",
                    expected: "LongArray",
                    got: tag.type_id(),
                });
            }
        }
    }
    if let Some(heightmap) = chunk
        .heightmaps
        .get("MOTION_BLOCKING")
        .or_else(|| chunk.heightmaps.get("WORLD_SURFACE"))
    {
        chunk.highest_opaque = heightmap.clone();
    }

    if let Some(be_list) = get_optional_list(root, "block_entities")? {
        for be in &be_list.elements {
            let cmp = expect_compound(be, "block_entities[]")?;
            let bx = get_int(cmp, "x")?;
            let by = get_int(cmp, "y")?;
            let bz = get_int(cmp, "z")?;
            let block_entity_id = get_string(cmp, "id").ok();
            if block_entity_id.is_some_and(|id| id == "minecraft:conduit") {
                mc_data::conduit_nbt::target(cmp).map_err(ChunkNbtError::InvalidBlockEntity)?;
            }
            mc_data::trial_nbt::validate(cmp, items).map_err(|error| {
                ChunkNbtError::InvalidBlockEntity(format!(
                    "block entity at ({bx},{by},{bz}): {error}"
                ))
            })?;
            if block_entity_id.is_some_and(|id| is_furnace_block_entity_id(id))
                && let Some(items) = items
            {
                chunk.furnaces.insert(
                    BlockPos {
                        x: bx,
                        y: by,
                        z: bz,
                    },
                    decode_furnace(cmp, items)?,
                );
                continue;
            }
            if block_entity_id.is_some_and(|id| is_chest_storage_block_entity_id(id))
                && let Some(items) = items
            {
                let pos = BlockPos {
                    x: bx,
                    y: by,
                    z: bz,
                };
                let mut chest = decode_chest(cmp, items)?;
                if chunk
                    .get_block(bx.rem_euclid(16) as u8, by, bz.rem_euclid(16) as u8)
                    .is_some_and(|id| super::copper_chest_nbt::is_copper(registry, id))
                {
                    super::copper_chest_nbt::materialize(&mut chest, items, chest_loot)?;
                    let mut bytes = Vec::new();
                    mc_nbt::write_network(&mut bytes, be)?;
                    chunk.block_entities.insert(pos, bytes);
                }
                if chest.loot_table.is_some() && !chunk.block_entities.contains_key(&pos) {
                    let mut bytes = Vec::new();
                    mc_nbt::write_network(&mut bytes, be)?;
                    chunk.block_entities.insert(pos, bytes);
                }
                chunk.chests.insert(pos, chest);
                continue;
            }
            if block_entity_id.is_some_and(|id| is_hopper_block_entity_id(id))
                && let Some(items) = items
            {
                chunk.hoppers.insert(
                    BlockPos {
                        x: bx,
                        y: by,
                        z: bz,
                    },
                    decode_hopper(cmp, items)?,
                );
                continue;
            }
            if block_entity_id.is_some_and(|id| id == "minecraft:brewing_stand")
                && let Some(items) = items
            {
                chunk.brewing_stands.insert(
                    BlockPos {
                        x: bx,
                        y: by,
                        z: bz,
                    },
                    decode_brewing_stand(cmp, items)?,
                );
                continue;
            }
            if let Some(items) = items {
                mc_data::storage_blocks::validate(cmp, items)
                    .map_err(ChunkNbtError::InvalidStorageBlockEntity)?;
            }

            // Round-trip the entry verbatim by re-encoding it through
            // the network NBT path ([type][payload], no name). Read
            // back via read_network when needed.
            let mut buf = Vec::with_capacity(64);
            mc_nbt::write_network(&mut buf, be)?;
            chunk.block_entities.insert(
                BlockPos {
                    x: bx,
                    y: by,
                    z: bz,
                },
                buf,
            );
        }
    }

    if let Some(block_ticks) = get_optional_list(root, "block_ticks")? {
        chunk.load_scheduled_block_ticks(decode_scheduled_block_ticks(block_ticks, current_tick)?);
    }
    if let Some(fluid_ticks) = get_optional_list(root, "fluid_ticks")? {
        chunk.load_scheduled_fluid_ticks(decode_scheduled_fluid_ticks(fluid_ticks, current_tick)?);
    }

    // M5.c.2: capture every other root-level field verbatim so a
    // load → save round-trip preserves the unmodelled subset
    // (PostProcessing, structures,
    // InhabitedTime, LastUpdate, DataVersion, ...). Order is the
    // original compound's insertion order — vanilla doesn't require
    // any particular ordering, but stable ordering keeps byte-diff
    // workflows clean.
    for (key, value) in root {
        if !MODELLED_ROOT_KEYS.contains(&key.as_str()) {
            chunk.extras.push((key.clone(), value.clone()));
        }
    }

    Ok(chunk)
}

/// Explicit offline repair primitive for a parsed chunk root.
///
/// This function is never called by runtime loading. Tooling may use it to
/// canonicalize missing, wrongly typed, duplicated, or mismatched `xPos`/`zPos`
/// fields to the region slot selected by the operator.
pub fn repair_chunk_nbt_position(
    nbt: &mut Tag,
    expected_pos: ChunkPos,
) -> Result<bool, ChunkNbtError> {
    let Tag::Compound(root) = nbt else {
        return Err(ChunkNbtError::NotCompound(nbt.type_id()));
    };
    let repaired_x = repair_position_field(root, "xPos", expected_pos.x);
    let repaired_z = repair_position_field(root, "zPos", expected_pos.z);
    Ok(repaired_x || repaired_z)
}

fn repair_position_field(root: &mut Vec<(String, Tag)>, field: &'static str, value: i32) -> bool {
    let mut changed = false;
    let mut first = None;
    let mut index = 0_usize;
    while index < root.len() {
        if root[index].0 != field {
            index += 1;
            continue;
        }
        if first.is_some() {
            root.remove(index);
            changed = true;
            continue;
        }
        first = Some(index);
        if !matches!(root[index].1, Tag::Int(current) if current == value) {
            root[index].1 = Tag::Int(value);
            changed = true;
        }
        index += 1;
    }
    if first.is_none() {
        root.push((field.to_string(), Tag::Int(value)));
        changed = true;
    }
    changed
}

fn ensure_chunk_position(expected: ChunkPos, actual: ChunkPos) -> Result<(), ChunkNbtError> {
    if expected != actual {
        return Err(ChunkNbtError::PositionMismatch {
            expected_x: expected.x,
            expected_z: expected.z,
            actual_x: actual.x,
            actual_z: actual.z,
        });
    }
    Ok(())
}

/// Root-level NBT keys `chunk_from_nbt` decodes into typed fields.
/// Anything outside this set ends up on `Chunk.extras` so the
/// codec round-trips byte-stably on the unmodelled subset.
const MODELLED_ROOT_KEYS: &[&str] = &[
    "xPos",
    "zPos",
    "yPos",
    "Status",
    "sections",
    "Heightmaps",
    "block_entities",
    "block_ticks",
    "fluid_ticks",
];

struct DecodedScheduledTick {
    pos: BlockPos,
    id: Identifier,
    trigger_tick: u64,
    priority: i32,
    sequence: u64,
}

fn decode_scheduled_tick_entry(
    tag: &Tag,
    list_name: &str,
    current_tick: u64,
    sequence: u64,
) -> Result<DecodedScheduledTick, ChunkNbtError> {
    let tick = expect_compound(tag, list_name)?;
    let name = get_string(tick, "i")?;
    let id = Identifier::parse(name.clone())
        .map_err(|_| ChunkNbtError::InvalidIdentifier(name.clone()))?;
    let delay = get_int(tick, "t")?;
    Ok(DecodedScheduledTick {
        pos: BlockPos {
            x: get_int(tick, "x")?,
            y: get_int(tick, "y")?,
            z: get_int(tick, "z")?,
        },
        id,
        // Vanilla may persist overdue work as a negative relative delay.
        trigger_tick: current_tick.saturating_add_signed(i64::from(delay)),
        priority: get_int(tick, "p")?,
        sequence,
    })
}

fn decode_scheduled_block_ticks(
    list: &ListTag,
    current_tick: u64,
) -> Result<Vec<ScheduledBlockTick>, ChunkNbtError> {
    list.elements
        .iter()
        .enumerate()
        .map(|(sequence, tag)| {
            let tick =
                decode_scheduled_tick_entry(tag, "block_ticks[]", current_tick, sequence as u64)?;
            Ok(ScheduledBlockTick::from_storage(
                tick.pos,
                tick.id,
                tick.trigger_tick,
                tick.priority,
                tick.sequence,
            ))
        })
        .collect()
}

fn decode_scheduled_fluid_ticks(
    list: &ListTag,
    current_tick: u64,
) -> Result<Vec<ScheduledFluidTick>, ChunkNbtError> {
    list.elements
        .iter()
        .enumerate()
        .map(|(sequence, tag)| {
            let tick =
                decode_scheduled_tick_entry(tag, "fluid_ticks[]", current_tick, sequence as u64)?;
            Ok(ScheduledFluidTick::from_storage(
                tick.pos,
                tick.id,
                tick.trigger_tick,
                tick.priority,
                tick.sequence,
            ))
        })
        .collect()
}

pub(crate) fn decode_block_section(
    nbt: &[(String, Tag)],
    registry: &BlockRegistry,
    air: BlockStateId,
) -> Result<ChunkSection, ChunkNbtError> {
    let palette_list = get_list(nbt, "palette")?;
    let palette: Vec<BlockStateId> = palette_list
        .elements
        .iter()
        .map(|p| {
            let entry = expect_compound(p, "palette[]")?;
            let name_str: String = get_string(entry, "Name")?.clone();
            let name = Identifier::parse(name_str.clone())
                .map_err(|_| ChunkNbtError::InvalidIdentifier(name_str.clone()))?;
            let mut props = if let Some(props_cmp) = get_optional_compound(entry, "Properties")? {
                props_cmp
                    .iter()
                    .map(|(k, v)| match v {
                        Tag::String(s) => Ok((k.clone(), s.clone())),
                        other => Err(ChunkNbtError::WrongType {
                            field: "Properties.*",
                            expected: "String",
                            got: other.type_id(),
                        }),
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            // Vanilla NbtUtils.readBlockState begins with defaultBlockState and
            // applies supplied properties. Older palettes can omit properties
            // introduced later (for example skulls' powered=false).
            if let Some(default) = registry
                .block(&name)
                .and_then(|block| registry.by_id(block.default))
            {
                for (key, value) in &default.properties {
                    if !props.iter().any(|(supplied, _)| supplied == key) {
                        props.push((key.clone(), value.clone()));
                    }
                }
            }
            registry
                .by_name_and_props(&name, &props)
                .ok_or(ChunkNbtError::UnknownBlockState {
                    name: name_str,
                    props,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    match get_optional_long_array(nbt, "data")? {
        None => {
            // Single-state section.
            Ok(ChunkSection::filled(palette[0], air))
        }
        Some(longs) => {
            let bits = block_states_bits_per_entry(palette.len());
            let words: Vec<u64> = longs.iter().map(|&l| l as u64).collect();
            let epw = 64 / bits as usize;
            let expected_words = SECTION_VOLUME.div_ceil(epw);
            if words.len() != expected_words {
                return Err(ChunkNbtError::PackedWordMismatch {
                    expected: expected_words,
                    got: words.len(),
                });
            }
            let indices = PackedBitArray::from_words(bits, SECTION_VOLUME, words);
            Ok(ChunkSection::from_indirect(palette, indices, air))
        }
    }
}

fn decode_biome_section(nbt: &[(String, Tag)]) -> Result<BiomeSection, ChunkNbtError> {
    let palette_list = get_list(nbt, "palette")?;
    let palette: Vec<Identifier> = palette_list
        .elements
        .iter()
        .map(|p| match p {
            Tag::String(s) => Identifier::parse(s.clone())
                .map_err(|_| ChunkNbtError::InvalidIdentifier(s.clone())),
            other => Err(ChunkNbtError::WrongType {
                field: "biomes.palette[]",
                expected: "String",
                got: other.type_id(),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?;

    match get_optional_long_array(nbt, "data")? {
        None => Ok(BiomeSection::filled(
            palette
                .into_iter()
                .next()
                .expect("vanilla biome palette always non-empty"),
        )),
        Some(longs) => {
            let bits = biome_bits_per_entry(palette.len());
            let words: Vec<u64> = longs.iter().map(|&l| l as u64).collect();
            let epw = 64 / bits as usize;
            let expected_words = BIOME_VOLUME.div_ceil(epw);
            if words.len() != expected_words {
                return Err(ChunkNbtError::PackedWordMismatch {
                    expected: expected_words,
                    got: words.len(),
                });
            }
            let indices = PackedBitArray::from_words(bits, BIOME_VOLUME, words);
            Ok(BiomeSection::from_indirect(palette, indices))
        }
    }
}

fn decode_section_light(cmp: &[(String, Tag)]) -> Result<SectionLight, ChunkNbtError> {
    Ok(SectionLight {
        block: decode_light_layer(cmp, "BlockLight")?,
        sky: decode_light_layer(cmp, "SkyLight")?,
    })
}

fn decode_light_layer(
    cmp: &[(String, Tag)],
    field: &'static str,
) -> Result<Option<crate::chunk::LightSection>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == field).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::ByteArray(bytes) => {
            if bytes.len() != LIGHT_LAYER_BYTES {
                return Err(ChunkNbtError::LightLengthMismatch {
                    field,
                    expected: LIGHT_LAYER_BYTES,
                    got: bytes.len(),
                });
            }
            Ok(Some(crate::chunk::LightSection::from_bytes(
                std::array::from_fn(|index| bytes[index] as u8),
            )))
        }
        other => Err(ChunkNbtError::WrongType {
            field,
            expected: "ByteArray",
            got: other.type_id(),
        }),
    }
}

/// Per Anvil: `max(4, ceil(log2(palette.len())))`.
fn block_states_bits_per_entry(palette_len: usize) -> u8 {
    if palette_len <= 1 {
        return 4;
    }
    let raw = (palette_len - 1).ilog2() as u8 + 1;
    raw.max(4)
}

/// Biome bits per entry: `max(1, ceil(log2(palette.len())))`, no
/// minimum-4 quirk (biomes only have 64 cells per section, the
/// minimum-4 padding would waste 75% of every entry).
fn biome_bits_per_entry(palette_len: usize) -> u8 {
    if palette_len <= 1 {
        return 1;
    }
    (palette_len - 1).ilog2() as u8 + 1
}

// ---------------------------------------------------------------------
// Encode
// ---------------------------------------------------------------------

pub fn chunk_to_nbt(chunk: &Chunk, registry: &BlockRegistry) -> Result<Tag, ChunkNbtError> {
    chunk_to_nbt_with_items(chunk, registry, None)
}

pub fn chunk_to_nbt_with_items(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
) -> Result<Tag, ChunkNbtError> {
    chunk_to_nbt_with_items_at_tick(chunk, registry, items, 0)
}

pub fn chunk_to_nbt_with_items_at_tick(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    current_tick: u64,
) -> Result<Tag, ChunkNbtError> {
    chunk_to_nbt_parts(chunk, registry, items, current_tick, true)
}

/// Native side state omits procedural blocks and Anvil-relative ticks; the native
/// codec owns lossless absolute tick deadlines and sequence identities.
pub(crate) fn chunk_metadata_to_nbt_with_items_at_tick(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    current_tick: u64,
) -> Result<Tag, ChunkNbtError> {
    chunk_to_nbt_parts(chunk, registry, items, current_tick, false)
}

fn chunk_to_nbt_parts(
    chunk: &Chunk,
    registry: &BlockRegistry,
    items: Option<&ItemRegistry>,
    current_tick: u64,
    include_blocks: bool,
) -> Result<Tag, ChunkNbtError> {
    let extra = |key: &str| {
        chunk
            .extras
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    };
    let data_version = match extra("DataVersion") {
        Some(Tag::Int(value)) => *value,
        _ => TARGET_DATA_VERSION,
    };
    let last_update = if current_tick == 0 {
        match extra("LastUpdate") {
            Some(Tag::Long(value)) => *value,
            _ => 0,
        }
    } else {
        i64::try_from(current_tick).unwrap_or(i64::MAX)
    };
    let inhabited_time = match extra("InhabitedTime") {
        Some(Tag::Long(value)) => *value,
        _ => 0,
    };

    let mut root: Vec<(String, Tag)> = Vec::with_capacity(11);
    root.push(("DataVersion".into(), Tag::Int(data_version)));
    root.push(("xPos".into(), Tag::Int(chunk.pos.x)));
    root.push(("zPos".into(), Tag::Int(chunk.pos.z)));
    let min_section_y = chunk.geometry().min_y() / SECTION_DIM as i32;
    root.push(("yPos".into(), Tag::Int(min_section_y)));
    root.push(("LastUpdate".into(), Tag::Long(last_update)));
    root.push(("InhabitedTime".into(), Tag::Long(inhabited_time)));
    root.push(("Status".into(), Tag::String(chunk.status.clone())));

    // sections
    let mut sections = Vec::with_capacity(chunk.sections.len());
    for (i, sec) in chunk.sections.iter().enumerate() {
        let section_y = min_section_y + i as i32;
        let y = i8::try_from(section_y)
            .map_err(|_| ChunkNbtError::SectionYOutOfByteRange(section_y))?;
        let mut s_cmp = vec![("Y".into(), Tag::Byte(y))];
        if include_blocks {
            s_cmp.push(("block_states".into(), encode_block_section(sec, registry)?));
        }
        s_cmp.push(("biomes".into(), encode_biome_section(&chunk.biomes[i])));
        if let Some(tag) = encode_light_layer(&chunk.section_lights[i].block) {
            s_cmp.push(("BlockLight".into(), tag));
        }
        if let Some(tag) = encode_light_layer(&chunk.section_lights[i].sky) {
            s_cmp.push(("SkyLight".into(), tag));
        }
        sections.push(Tag::Compound(s_cmp));
    }
    for (boundary, light) in chunk.boundary_section_lights.iter().enumerate() {
        if light.block.is_none() && light.sky.is_none() {
            continue;
        }
        let section_y = if boundary == 0 {
            min_section_y - 1
        } else {
            min_section_y + chunk.sections.len() as i32
        };
        let y = i8::try_from(section_y)
            .map_err(|_| ChunkNbtError::SectionYOutOfByteRange(section_y))?;
        let mut fields = vec![("Y".into(), Tag::Byte(y))];
        if let Some(tag) = encode_light_layer(&light.block) {
            fields.push(("BlockLight".into(), tag));
        }
        if let Some(tag) = encode_light_layer(&light.sky) {
            fields.push(("SkyLight".into(), tag));
        }
        sections.push(Tag::Compound(fields));
    }
    root.push((
        "sections".into(),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: sections,
        }),
    ));

    // Heightmaps
    let mut hms: Vec<(String, Tag)> = chunk
        .heightmaps
        .iter()
        .map(|(name, h)| (name.clone(), Tag::LongArray(h.to_long_array())))
        .collect();
    // Stable order for byte-similar emission.
    hms.sort_by(|a, b| a.0.cmp(&b.0));
    root.push(("Heightmaps".into(), Tag::Compound(hms)));

    // block_entities — read back the opaque network NBT into Tags.
    let mut be_list: Vec<Tag> = Vec::with_capacity(chunk.block_entities.len());
    let mut be_entries: Vec<(&BlockPos, &Vec<u8>)> = chunk
        .block_entities
        .iter()
        .filter(|(pos, _)| {
            !chunk.furnaces.contains_key(pos)
                && !chunk.chests.contains_key(pos)
                && !chunk.hoppers.contains_key(pos)
                && !chunk.brewing_stands.contains_key(pos)
        })
        .collect();
    be_entries.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (_, bytes) in be_entries {
        let mut cur = Cursor::new(&bytes[..]);
        let tag = mc_nbt::read_network(&mut cur)?;
        be_list.push(tag);
    }
    if let Some(items) = items {
        let mut furnaces: Vec<(&BlockPos, &FurnaceBlockEntity)> = chunk.furnaces.iter().collect();
        furnaces.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
        for (pos, furnace) in furnaces {
            be_list.push(encode_furnace(
                pos,
                furnace,
                items,
                furnace_block_entity_id_for_block(chunk, registry, *pos),
            )?);
        }
        let mut chests: Vec<(&BlockPos, &ChestBlockEntity)> = chunk.chests.iter().collect();
        chests.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
        for (pos, chest) in chests {
            let mut encoded = encode_chest(
                pos,
                chest,
                items,
                chest_storage_block_entity_id_for_block(chunk, registry, *pos),
            )?;
            if let Some(native) = chunk.block_entities.get(pos) {
                super::copper_chest_nbt::merge_metadata(&mut encoded, native)?;
            }
            be_list.push(encoded);
        }
        let mut hoppers: Vec<(&BlockPos, &HopperBlockEntity)> = chunk.hoppers.iter().collect();
        hoppers.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
        for (pos, hopper) in hoppers {
            be_list.push(encode_hopper(pos, hopper, items)?);
        }
        let mut stands: Vec<_> = chunk.brewing_stands.iter().collect();
        stands.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
        for (pos, stand) in stands {
            be_list.push(encode_brewing_stand(pos, stand, items)?);
        }
    }
    root.push((
        "block_entities".into(),
        Tag::List(ListTag {
            element_type: if be_list.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements: be_list,
        }),
    ));

    if include_blocks {
        root.push((
            "block_ticks".into(),
            encode_scheduled_block_ticks(&chunk.scheduled_block_ticks, current_tick)?,
        ));
        root.push((
            "fluid_ticks".into(),
            encode_scheduled_fluid_ticks(&chunk.scheduled_fluid_ticks, current_tick)?,
        ));
    }

    // M5.c.2: re-emit every root-level field decode kept in
    // `extras` (PostProcessing,
    // structures, InhabitedTime, LastUpdate, DataVersion, etc.).
    // Order is the decode-time insertion order so the round-trip
    // stays stable.
    for (key, value) in &chunk.extras {
        if !MODELLED_ROOT_KEYS.contains(&key.as_str())
            && !VANILLA_METADATA_KEYS.contains(&key.as_str())
        {
            root.push((key.clone(), value.clone()));
        }
    }

    Ok(Tag::Compound(root))
}

fn encode_light_layer(layer: &Option<crate::chunk::LightSection>) -> Option<Tag> {
    layer
        .as_ref()
        .map(|layer| Tag::ByteArray(layer.bytes().map(|byte| byte as i8).collect()))
}

fn anvil_tick_delay(deadline: u64, current_tick: u64) -> Result<i32, ChunkNbtError> {
    // Wider arithmetic preserves the sign across the entire native u64 clock range.
    let delay = i128::from(deadline) - i128::from(current_tick);
    i32::try_from(delay).map_err(|_| ChunkNbtError::TickDelayOutOfRange(delay))
}
fn encode_scheduled_block_ticks(
    ticks: &[ScheduledBlockTick],
    current_tick: u64,
) -> Result<Tag, ChunkNbtError> {
    let mut elements = Vec::with_capacity(ticks.len());
    for tick in ticks {
        let delay = anvil_tick_delay(tick.trigger_tick, current_tick)?;
        elements.push(Tag::Compound(vec![
            ("i".into(), Tag::String(tick.block.as_str().to_string())),
            ("x".into(), Tag::Int(tick.pos.x)),
            ("y".into(), Tag::Int(tick.pos.y)),
            ("z".into(), Tag::Int(tick.pos.z)),
            ("t".into(), Tag::Int(delay)),
            ("p".into(), Tag::Int(tick.priority)),
        ]));
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
}

fn encode_scheduled_fluid_ticks(
    ticks: &[ScheduledFluidTick],
    current_tick: u64,
) -> Result<Tag, ChunkNbtError> {
    let mut elements = Vec::with_capacity(ticks.len());
    for tick in ticks {
        let delay = anvil_tick_delay(tick.trigger_tick, current_tick)?;
        elements.push(Tag::Compound(vec![
            ("i".into(), Tag::String(tick.fluid.as_str().to_string())),
            ("x".into(), Tag::Int(tick.pos.x)),
            ("y".into(), Tag::Int(tick.pos.y)),
            ("z".into(), Tag::Int(tick.pos.z)),
            ("t".into(), Tag::Int(delay)),
            ("p".into(), Tag::Int(tick.priority)),
        ]));
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
}

pub(crate) fn encode_block_section(
    section: &ChunkSection,
    registry: &BlockRegistry,
) -> Result<Tag, ChunkNbtError> {
    let mut out: Vec<(String, Tag)> = Vec::with_capacity(2);
    match section.palette() {
        None => {
            // Single mode: palette has exactly one entry, no data.
            let id = section.get(0, 0, 0);
            let entry = block_state_to_palette_entry(id, registry)?;
            out.push((
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![entry],
                }),
            ));
        }
        Some(palette) => {
            let pal_tags = palette
                .iter()
                .map(|&id| block_state_to_palette_entry(id, registry))
                .collect::<Result<Vec<_>, _>>()?;
            out.push((
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: pal_tags,
                }),
            ));
            let indices = section.indices().expect("indirect mode has indices");
            let longs = indices
                .words_at_bits(block_states_bits_per_entry(palette.len()))
                .map(|word| word as i64)
                .collect();
            out.push(("data".into(), Tag::LongArray(longs)));
        }
    }
    Ok(Tag::Compound(out))
}

pub(crate) fn block_state_to_palette_entry(
    id: BlockStateId,
    registry: &BlockRegistry,
) -> Result<Tag, ChunkNbtError> {
    let state = registry
        .by_id(id)
        .ok_or(ChunkNbtError::UnknownBlockStateId(id.0))?;
    let mut entry: Vec<(String, Tag)> = Vec::with_capacity(2);
    entry.push((
        "Name".into(),
        Tag::String(state.block.id.as_str().to_string()),
    ));
    if !state.properties.is_empty() {
        let props: Vec<(String, Tag)> = state
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), Tag::String(v.clone())))
            .collect();
        entry.push(("Properties".into(), Tag::Compound(props)));
    }
    Ok(Tag::Compound(entry))
}

fn encode_biome_section(section: &BiomeSection) -> Tag {
    let mut out: Vec<(String, Tag)> = Vec::with_capacity(2);
    match section {
        BiomeSection::Single(id) => {
            out.push((
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::STRING,
                    elements: vec![Tag::String(id.as_str().to_string())],
                }),
            ));
        }
        BiomeSection::Indirect { palette, indices } => {
            let pal_tags: Vec<Tag> = palette
                .iter()
                .map(|id| Tag::String(id.as_str().to_string()))
                .collect();
            out.push((
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::STRING,
                    elements: pal_tags,
                }),
            ));
            let bits = biome_bits_per_entry(palette.len());
            let longs: Vec<i64> = if indices.bits_per_entry() == bits {
                indices.words().iter().map(|&w| w as i64).collect()
            } else {
                let mut packed = PackedBitArray::zeroed(bits, BIOME_VOLUME);
                for idx in 0..BIOME_VOLUME {
                    packed.set(idx, indices.get(idx));
                }
                packed.words().iter().map(|&w| w as i64).collect()
            };
            out.push(("data".into(), Tag::LongArray(longs)));
        }
    }
    Tag::Compound(out)
}

fn decode_brewing_stand(
    cmp: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<BrewingStandBlockEntity, ChunkNbtError> {
    let mut stand = BrewingStandBlockEntity {
        brew_time: get_optional_short(cmp, "BrewTime")?
            .unwrap_or(0)
            .clamp(0, 400),
        fuel: cmp
            .iter()
            .find(|(key, _)| key == "Fuel")
            .map(|(_, tag)| match tag {
                Tag::Byte(value) => Ok(i16::from(*value).clamp(0, 20)),
                other => Err(ChunkNbtError::WrongType {
                    field: "Fuel",
                    expected: "Byte",
                    got: other.type_id(),
                }),
            })
            .transpose()?
            .unwrap_or(0),
        ..Default::default()
    };
    if let Some(list) = get_optional_list(cmp, "Items")? {
        for tag in &list.elements {
            let item = expect_compound(tag, "Items[]")?;
            let slot = get_container_slot(item)?;
            if (0..=4).contains(&slot) {
                stand.slots[slot as usize] = decode_container_stack(item, items)?;
            }
        }
    }
    if stand.brew_time > 0 && !stand.slots[3].is_empty() {
        stand.ingredient = Some(stand.slots[3].item_id);
    }
    Ok(stand)
}

fn encode_brewing_stand(
    pos: &BlockPos,
    stand: &BrewingStandBlockEntity,
    items: &ItemRegistry,
) -> Result<Tag, ChunkNbtError> {
    let mut item_tags = Vec::new();
    for (slot, stack) in stand.slots.iter().enumerate() {
        if !stack.is_empty() {
            item_tags.push(encode_container_stack(slot, stack, items)?);
        }
    }
    Ok(Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:brewing_stand".into())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        ("BrewTime".into(), Tag::Short(stand.brew_time)),
        ("Fuel".into(), Tag::Byte(stand.fuel as i8)),
        (
            "Items".into(),
            Tag::List(ListTag {
                element_type: if item_tags.is_empty() {
                    tag_type::END
                } else {
                    tag_type::COMPOUND
                },
                elements: item_tags,
            }),
        ),
    ]))
}

fn decode_furnace(
    cmp: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<FurnaceBlockEntity, ChunkNbtError> {
    let mut furnace = FurnaceBlockEntity {
        burn_remaining: get_optional_furnace_timer(cmp, "lit_time_remaining")?.unwrap_or(0),
        burn_total: get_optional_furnace_timer(cmp, "lit_total_time")?.unwrap_or(1600),
        cook_progress: get_optional_furnace_timer(cmp, "cooking_time_spent")?.unwrap_or(0),
        cook_total: get_optional_furnace_timer(cmp, "cooking_total_time")?.unwrap_or(200),
        ..FurnaceBlockEntity::default()
    };

    if let Some(list) = get_optional_list(cmp, "Items")? {
        for tag in &list.elements {
            let item = expect_compound(tag, "Items[]")?;
            let slot = get_container_slot(item)?;
            if !(0..=2).contains(&slot) {
                continue;
            }
            furnace.slots[slot as usize] = decode_container_stack(item, items)?;
        }
    }
    if let Some(recipes_used) = get_optional_compound(cmp, "RecipesUsed")? {
        for (recipe_id, count) in recipes_used {
            let parsed = Identifier::parse(recipe_id.clone())
                .map_err(|_| ChunkNbtError::InvalidIdentifier(recipe_id.clone()))?;
            let Tag::Int(count) = count else {
                return Err(ChunkNbtError::WrongType {
                    field: "RecipesUsed.*",
                    expected: "Int",
                    got: count.type_id(),
                });
            };
            if *count > 0 {
                furnace
                    .recipes_used
                    .insert(parsed.as_str().to_string(), *count);
            }
        }
    }
    Ok(furnace)
}

fn decode_chest(
    cmp: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<ChestBlockEntity, ChunkNbtError> {
    let mut chest = ChestBlockEntity {
        loot_table: get_optional_string(cmp, "LootTable")?
            .map(|name| {
                Identifier::parse(name)
                    .map_err(|_| ChunkNbtError::InvalidIdentifier(name.to_owned()))
            })
            .transpose()?,
        ..Default::default()
    };
    if let Some((_, seed)) = cmp.iter().find(|(name, _)| name == "LootTableSeed") {
        let Tag::Long(seed) = seed else {
            return Err(ChunkNbtError::WrongType {
                field: "LootTableSeed",
                expected: "Long",
                got: seed.type_id(),
            });
        };
        chest.loot_seed = *seed;
    }
    if chest.loot_table.is_some() {
        return Ok(chest);
    }
    if let Some(list) = get_optional_list(cmp, "Items")? {
        for tag in &list.elements {
            let item = expect_compound(tag, "Items[]")?;
            let slot = get_container_slot(item)?;
            if !(0..=26).contains(&slot) {
                continue;
            }
            chest.slots[slot as usize] = decode_container_stack(item, items)?;
        }
    }
    Ok(chest)
}

fn decode_hopper(
    cmp: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<HopperBlockEntity, ChunkNbtError> {
    let mut hopper = HopperBlockEntity {
        transfer_cooldown: get_optional_int(cmp, "TransferCooldown")?.unwrap_or(-1),
        ..Default::default()
    };
    if let Some(list) = get_optional_list(cmp, "Items")? {
        for tag in &list.elements {
            let item = expect_compound(tag, "Items[]")?;
            let slot = get_container_slot(item)?;
            if !(0..=4).contains(&slot) {
                continue;
            }
            hopper.slots[slot as usize] = decode_container_stack(item, items)?;
        }
    }
    Ok(hopper)
}

fn encode_furnace(
    pos: &BlockPos,
    furnace: &FurnaceBlockEntity,
    items: &ItemRegistry,
    block_entity_id: &str,
) -> Result<Tag, ChunkNbtError> {
    let mut compound = vec![
        ("id".into(), Tag::String(block_entity_id.to_string())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ];

    let mut item_tags = Vec::new();
    for (slot, stack) in furnace.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        item_tags.push(encode_container_stack(slot, stack, items)?);
    }
    compound.push((
        "Items".into(),
        Tag::List(ListTag {
            element_type: if item_tags.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements: item_tags,
        }),
    ));
    compound.push((
        "lit_time_remaining".into(),
        Tag::Short(furnace.burn_remaining),
    ));
    compound.push(("lit_total_time".into(), Tag::Short(furnace.burn_total)));
    compound.push((
        "cooking_time_spent".into(),
        Tag::Short(furnace.cook_progress),
    ));
    compound.push(("cooking_total_time".into(), Tag::Short(furnace.cook_total)));
    compound.push((
        "RecipesUsed".into(),
        Tag::Compound(
            furnace
                .recipes_used
                .iter()
                .filter(|(_, count)| **count > 0)
                .map(|(recipe_id, count)| (recipe_id.clone(), Tag::Int(*count)))
                .collect(),
        ),
    ));
    Ok(Tag::Compound(compound))
}

fn is_furnace_block_entity_id(id: &str) -> bool {
    matches!(
        id,
        "minecraft:furnace" | "minecraft:smoker" | "minecraft:blast_furnace"
    )
}

fn furnace_block_entity_id_for_block(
    chunk: &Chunk,
    registry: &BlockRegistry,
    pos: BlockPos,
) -> &'static str {
    let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
    let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
    use mc_data::block_entity_kinds::BlockEntityKind;
    chunk
        .get_block(local_x, pos.y, local_z)
        .and_then(|state_id| registry.by_id(state_id))
        .and_then(|state| BlockEntityKind::for_block(state.block.id.path()))
        .filter(|kind| {
            matches!(
                kind,
                BlockEntityKind::Furnace | BlockEntityKind::Smoker | BlockEntityKind::BlastFurnace
            )
        })
        .unwrap_or(BlockEntityKind::Furnace)
        .vanilla_id()
}

fn encode_chest(
    pos: &BlockPos,
    chest: &ChestBlockEntity,
    items: &ItemRegistry,
    block_entity_id: &str,
) -> Result<Tag, ChunkNbtError> {
    let mut compound = vec![
        ("id".into(), Tag::String(block_entity_id.to_string())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ];
    if let Some(table) = &chest.loot_table {
        compound.push(("LootTable".into(), Tag::String(table.to_string())));
        if chest.loot_seed != 0 {
            compound.push(("LootTableSeed".into(), Tag::Long(chest.loot_seed)));
        }
        return Ok(Tag::Compound(compound));
    }

    let mut item_tags = Vec::new();
    for (slot, stack) in chest.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        item_tags.push(encode_container_stack(slot, stack, items)?);
    }
    compound.push((
        "Items".into(),
        Tag::List(ListTag {
            element_type: if item_tags.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements: item_tags,
        }),
    ));
    Ok(Tag::Compound(compound))
}

fn is_chest_storage_block_entity_id(id: &str) -> bool {
    matches!(
        id,
        "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:barrel"
    )
}

fn encode_hopper(
    pos: &BlockPos,
    hopper: &HopperBlockEntity,
    items: &ItemRegistry,
) -> Result<Tag, ChunkNbtError> {
    let mut compound = vec![
        ("id".into(), Tag::String("minecraft:hopper".to_string())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        (
            "TransferCooldown".into(),
            Tag::Int(hopper.transfer_cooldown),
        ),
    ];

    let mut item_tags = Vec::new();
    for (slot, stack) in hopper.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        item_tags.push(encode_container_stack(slot, stack, items)?);
    }
    compound.push((
        "Items".into(),
        Tag::List(ListTag {
            element_type: if item_tags.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements: item_tags,
        }),
    ));
    Ok(Tag::Compound(compound))
}

fn is_hopper_block_entity_id(id: &str) -> bool {
    id == "minecraft:hopper"
}

fn decode_container_stack(
    item: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<FurnaceSlot, ChunkNbtError> {
    // Canonical item codec; mapped onto the chunk's slot shape.
    let stack = mc_data::item_nbt::stack_from_fields(items, item)
        .map_err(|err| match err {
            mc_data::item_nbt::ItemNbtError::InvalidId(id) => ChunkNbtError::InvalidIdentifier(id),
            mc_data::item_nbt::ItemNbtError::UnknownItem(id) => ChunkNbtError::UnknownItem(id),
            mc_data::item_nbt::ItemNbtError::InvalidEnchantment(id) => {
                ChunkNbtError::InvalidEnchantment(id)
            }
            mc_data::item_nbt::ItemNbtError::InvalidComponent(reason) => {
                ChunkNbtError::InvalidIdentifier(reason)
            }
        })?
        .ok_or_else(|| ChunkNbtError::InvalidIdentifier("container stack without id".to_owned()))?;
    Ok(FurnaceSlot {
        count: stack.count,
        item_id: stack.item_id,
        damage: stack.damage,
        enchantments: stack.enchantments,
        custom_name: stack.custom_name.map(|name| name.into_string()),
        item_model: stack.item_model,
        stew_effects: stack.stew_effects,
        components: stack.components,
    })
}

fn encode_container_stack(
    slot: usize,
    stack: &FurnaceSlot,
    items: &ItemRegistry,
) -> Result<Tag, ChunkNbtError> {
    // Canonical item codec via the shared item shape.
    let shared = mc_data::ItemStack {
        count: stack.count,
        item_id: stack.item_id,
        damage: stack.damage,
        enchantments: stack.enchantments.clone(),
        custom_name: stack
            .custom_name
            .as_deref()
            .map(str::to_owned)
            .map(String::into_boxed_str),
        item_model: stack.item_model.clone(),
        stew_effects: stack.stew_effects.clone(),
        components: stack.components.clone(),
    };
    let mut fields =
        mc_data::item_nbt::stack_to_fields(items, &shared).map_err(|err| match err {
            mc_data::item_nbt::ItemNbtError::InvalidId(id) => ChunkNbtError::InvalidIdentifier(id),
            mc_data::item_nbt::ItemNbtError::UnknownItem(id) => ChunkNbtError::UnknownItem(id),
            mc_data::item_nbt::ItemNbtError::InvalidEnchantment(id) => {
                ChunkNbtError::InvalidEnchantment(id)
            }
            mc_data::item_nbt::ItemNbtError::InvalidComponent(reason) => {
                ChunkNbtError::InvalidIdentifier(reason)
            }
        })?;
    fields.insert(0, ("Slot".into(), Tag::Int(slot as i32)));
    Ok(Tag::Compound(fields))
}

fn chest_storage_block_entity_id_for_block(
    chunk: &Chunk,
    registry: &BlockRegistry,
    pos: BlockPos,
) -> &'static str {
    let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
    let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
    use mc_data::block_entity_kinds::BlockEntityKind;
    chunk
        .get_block(local_x, pos.y, local_z)
        .and_then(|state_id| registry.by_id(state_id))
        .and_then(|state| BlockEntityKind::for_block(state.block.id.path()))
        .filter(|kind| {
            matches!(
                kind,
                BlockEntityKind::Chest | BlockEntityKind::TrappedChest | BlockEntityKind::Barrel
            )
        })
        .unwrap_or(BlockEntityKind::Chest)
        .vanilla_id()
}

// ---------------------------------------------------------------------
// Small NBT helpers
// ---------------------------------------------------------------------

fn id(s: &str) -> Identifier {
    Identifier::parse(s).expect("static identifier")
}

fn expect_compound<'a>(tag: &'a Tag, name: &str) -> Result<&'a [(String, Tag)], ChunkNbtError> {
    match tag {
        Tag::Compound(entries) => Ok(entries),
        other => Err(if name == "root" {
            ChunkNbtError::NotCompound(other.type_id())
        } else {
            ChunkNbtError::WrongType {
                field: leak_field_name(name),
                expected: "Compound",
                got: other.type_id(),
            }
        }),
    }
}

fn get_field<'a>(cmp: &'a [(String, Tag)], name: &'static str) -> Result<&'a Tag, ChunkNbtError> {
    cmp.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
        .ok_or(ChunkNbtError::MissingField(name))
}

/// Vanilla inventories use a byte slot; Solaris chunk encoding uses an int.
/// Accept both locally to inventory slots, not general NBT integers.
fn get_container_slot(cmp: &[(String, Tag)]) -> Result<i32, ChunkNbtError> {
    match get_field(cmp, "Slot")? {
        Tag::Byte(value) => Ok(i32::from(*value)),
        Tag::Int(value) => Ok(*value),
        other => Err(ChunkNbtError::WrongType {
            field: "Slot",
            expected: "Byte or Int",
            got: other.type_id(),
        }),
    }
}

fn get_int(cmp: &[(String, Tag)], name: &'static str) -> Result<i32, ChunkNbtError> {
    match get_field(cmp, name)? {
        Tag::Int(v) => Ok(*v),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Int",
            got: other.type_id(),
        }),
    }
}

fn get_optional_int(
    cmp: &[(String, Tag)],
    name: &'static str,
) -> Result<Option<i32>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::Int(v) => Ok(Some(*v)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Int",
            got: other.type_id(),
        }),
    }
}

// DFU leaves missing yPos absent, rewrites an existing value to byte, and mixes
// byte/int section Y. The current writer emits int yPos and byte section Y.
fn get_section_y(cmp: &[(String, Tag)], name: &'static str) -> Result<i32, ChunkNbtError> {
    get_optional_section_y(cmp, name)?.ok_or(ChunkNbtError::MissingField(name))
}

fn get_optional_section_y(
    cmp: &[(String, Tag)],
    name: &'static str,
) -> Result<Option<i32>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(key, _)| key == name).map(|(_, tag)| tag) else {
        return Ok(None);
    };
    match tag {
        Tag::Byte(value) => Ok(Some(i32::from(*value))),
        Tag::Int(value) => Ok(Some(*value)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Byte or Int",
            got: other.type_id(),
        }),
    }
}

// Furnace timers in imported worlds may use any integral NBT width. Vanilla
// 26.1.2 still saves shorts (AbstractFurnaceBlockEntity.saveAdditional), so keep
// the persisted/internal i16 boundary and reject narrowing overflow.
fn get_optional_furnace_timer(
    cmp: &[(String, Tag)],
    name: &'static str,
) -> Result<Option<i16>, ChunkNbtError> {
    let Some((_, tag)) = cmp.iter().find(|(key, _)| key == name) else {
        return Ok(None);
    };
    let value = match tag {
        Tag::Byte(value) => i64::from(*value),
        Tag::Short(value) => i64::from(*value),
        Tag::Int(value) => i64::from(*value),
        Tag::Long(value) => *value,
        other => {
            return Err(ChunkNbtError::WrongType {
                field: name,
                expected: "Byte, Short, Int or Long",
                got: other.type_id(),
            });
        }
    };
    i16::try_from(value)
        .map(Some)
        .map_err(|_| ChunkNbtError::NumericOutOfRange { field: name, value })
}

fn get_optional_short(
    cmp: &[(String, Tag)],
    name: &'static str,
) -> Result<Option<i16>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::Short(v) => Ok(Some(*v)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Short",
            got: other.type_id(),
        }),
    }
}

fn get_string<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<&'a String, ChunkNbtError> {
    match get_field(cmp, name)? {
        Tag::String(s) => Ok(s),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "String",
            got: other.type_id(),
        }),
    }
}

fn get_list<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<&'a ListTag, ChunkNbtError> {
    match get_field(cmp, name)? {
        Tag::List(l) => Ok(l),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "List",
            got: other.type_id(),
        }),
    }
}

fn get_optional_list<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<Option<&'a ListTag>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::List(l) => Ok(Some(l)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "List",
            got: other.type_id(),
        }),
    }
}

fn get_optional_compound<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<Option<&'a [(String, Tag)]>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::Compound(e) => Ok(Some(e)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "Compound",
            got: other.type_id(),
        }),
    }
}

fn get_optional_string<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<Option<&'a str>, ChunkNbtError> {
    let Some(tag) = cmp
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
    else {
        return Ok(None);
    };
    match tag {
        Tag::String(value) => Ok(Some(value)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "String",
            got: other.type_id(),
        }),
    }
}

fn get_optional_long_array<'a>(
    cmp: &'a [(String, Tag)],
    name: &'static str,
) -> Result<Option<&'a [i64]>, ChunkNbtError> {
    let Some(tag) = cmp.iter().find(|(k, _)| k == name).map(|(_, v)| v) else {
        return Ok(None);
    };
    match tag {
        Tag::LongArray(a) => Ok(Some(a)),
        other => Err(ChunkNbtError::WrongType {
            field: name,
            expected: "LongArray",
            got: other.type_id(),
        }),
    }
}

fn leak_field_name(s: &str) -> &'static str {
    // Used only on error paths.
    Box::leak(s.to_string().into_boxed_str())
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
#[path = "block_palette_tests.rs"]
mod palette_tests;

#[cfg(test)]
#[path = "chunk_nbt_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "stations_tests.rs"]
mod stations_tests;
