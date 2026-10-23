//! Chunk: a 16×height×16 column at a fixed (x, z) coordinate, made of
//! stacked sections plus per-chunk side data (heightmaps, biomes,
//! block entities, generation status).
//!
//! `Chunk::empty` keeps the vanilla Overworld layout (Y=-64..320).
//! `Chunk::empty_with_geometry` supports another section-aligned block
//! storage range. Anvil, wire, and light geometry are migrated separately.
//!
//! For M2.d the chunk is mostly *storage*: a constructor that
//! produces an air-filled column, `get_block` / `set_block` that
//! route into the right section, and types ready for M2.e's Anvil
//! codec to populate (`heightmaps`, `biomes`, `block_entities`,
//! `status`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::block_light::BlockLightTable;
use mc_nbt::Tag;

use crate::block::BlockStateId;
use crate::light::ChunkLight;
use crate::section::{ChunkSection, PackedBitArray, SECTION_DIM};

#[cfg(test)]
mod light_compact_tests;

/// Lowest world Y coordinate (inclusive).
pub const MIN_Y: i32 = -64;
/// One past the highest world Y coordinate (exclusive).
pub const MAX_Y: i32 = 320;
/// Number of stacked sections per chunk.
pub const SECTION_COUNT: usize = 24;
/// Section-y index of the bottom section. `section_y = section_index + MIN_SECTION_Y`.
pub const MIN_SECTION_Y: i32 = MIN_Y / SECTION_DIM as i32;
/// Bits needed to address a Y value in `0..=MAX_Y - MIN_Y` for heightmap
/// packing. `MAX_Y - MIN_Y = 384` so 9 bits suffice (`2^9 = 512`).
pub const HEIGHTMAP_BITS: u8 = 9;
/// One heightmap entry per (x, z) cell in the 16×16 chunk footprint.
pub const HEIGHTMAP_LEN: usize = SECTION_DIM * SECTION_DIM;
const WORLD_JOURNAL_LSN_KEY: &str = "SolarisJournalLsn";
/// One biome cell per 4×4×4 sub-cube; vanilla packs biomes at 1/4 the
/// block resolution.
pub const BIOME_DIM: usize = 4;
pub const BIOME_VOLUME: usize = BIOME_DIM * BIOME_DIM * BIOME_DIM;
/// Bytes per per-section light layer: `16³` cells × 4 bits per cell.
pub const LIGHT_LAYER_BYTES: usize = SECTION_DIM * SECTION_DIM * SECTION_DIM / 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkGeometry {
    min_y: i32,
    max_y: i32,
}

impl ChunkGeometry {
    /// Returns `None` unless the range is section-aligned and fits the
    /// current 9-bit heightmap representation.
    #[must_use]
    pub fn new(min_y: i32, height: i32) -> Option<Self> {
        if min_y.rem_euclid(SECTION_DIM as i32) != 0
            || height <= 0
            || height > (1_i32 << HEIGHTMAP_BITS) - 1
            || height.rem_euclid(SECTION_DIM as i32) != 0
        {
            return None;
        }
        let max_y = min_y.checked_add(height)?;
        Some(Self { min_y, max_y })
    }

    #[must_use]
    pub const fn min_y(self) -> i32 {
        self.min_y
    }

    #[must_use]
    pub const fn max_y(self) -> i32 {
        self.max_y
    }

    #[must_use]
    pub const fn height(self) -> i32 {
        self.max_y - self.min_y
    }

    #[must_use]
    pub const fn section_count(self) -> usize {
        (self.height() / SECTION_DIM as i32) as usize
    }

    pub fn world_y_to_section(self, y: i32) -> Option<(usize, u8)> {
        if !(self.min_y..self.max_y).contains(&y) {
            return None;
        }
        let local = y - self.min_y;
        Some((
            (local / SECTION_DIM as i32) as usize,
            (local % SECTION_DIM as i32) as u8,
        ))
    }
}

pub const OVERWORLD_GEOMETRY: ChunkGeometry = ChunkGeometry {
    min_y: MIN_Y,
    max_y: MAX_Y,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

/// Returns every chunk touched by the inclusive world-space X/Z rectangle.
/// Invalid bounds or rectangles exceeding `max_chunks` fail closed.
#[must_use]
pub fn chunk_rectangle_for_world_bounds(
    min_x: f64,
    max_x: f64,
    min_z: f64,
    max_z: f64,
    max_chunks: usize,
) -> Option<Vec<ChunkPos>> {
    if !min_x.is_finite()
        || !max_x.is_finite()
        || !min_z.is_finite()
        || !max_z.is_finite()
        || min_x > max_x
        || min_z > max_z
        || max_chunks == 0
    {
        return None;
    }
    let min_cx = world_coordinate_to_chunk(min_x)?;
    let max_cx = world_coordinate_to_chunk(max_x)?;
    let min_cz = world_coordinate_to_chunk(min_z)?;
    let max_cz = world_coordinate_to_chunk(max_z)?;
    let width = i64::from(max_cx) - i64::from(min_cx) + 1;
    let depth = i64::from(max_cz) - i64::from(min_cz) + 1;
    let count = usize::try_from(width.checked_mul(depth)?).ok()?;
    if count > max_chunks {
        return None;
    }
    let mut chunks = Vec::with_capacity(count);
    for x in min_cx..=max_cx {
        for z in min_cz..=max_cz {
            chunks.push(ChunkPos { x, z });
        }
    }
    Some(chunks)
}

fn world_coordinate_to_chunk(value: f64) -> Option<i32> {
    let block = value.floor();
    if block < f64::from(i32::MIN) || block > f64::from(i32::MAX) {
        return None;
    }
    Some((block as i32).div_euclid(16))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

const GENERATED_ENTITIES_KEY: &str = "SolarisGeneratedEntities";

#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedEntityMarker {
    pub claim: String,
    pub entity_type: String,
    pub position: [f64; 3],
    /// Authored `PersistenceRequired`; applied through the native mob row.
    pub persistent: bool,
    pub villager_kind: String,
    pub profession: String,
    pub level: u8,
    /// The entity's native `Age`: negative for a baby, `0` for an adult.
    pub age: i32,
    /// The yaw the entity is placed with. Absent reads as `0.0`.
    pub yaw: f32,
    /// The pitch the entity's template authored. Absent reads as `0.0`.
    pub pitch: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledBlockTick {
    pub pos: BlockPos,
    pub block: Identifier,
    pub trigger_tick: u64,
    pub priority: i32,
    sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledFluidTick {
    pub pos: BlockPos,
    pub fluid: Identifier,
    pub trigger_tick: u64,
    pub priority: i32,
    sequence: u64,
}

impl ScheduledFluidTick {
    /// Supported scheduled-fluid identities, without rewriting the source identifier.
    #[must_use]
    pub fn fluid_kind(name: &Identifier) -> Option<mc_data::block_facts::FluidKind> {
        use mc_data::block_facts::FluidKind;
        match name.as_str() {
            "minecraft:water" | "minecraft:flowing_water" => Some(FluidKind::Water),
            "minecraft:lava" | "minecraft:flowing_lava" => Some(FluidKind::Lava),
            _ => None,
        }
    }

    #[must_use]
    pub fn new(pos: BlockPos, fluid: Identifier, trigger_tick: u64, priority: i32) -> Self {
        Self {
            pos,
            fluid,
            trigger_tick,
            priority,
            sequence: 0,
        }
    }

    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn from_storage(
        pos: BlockPos,
        fluid: Identifier,
        trigger_tick: u64,
        priority: i32,
        sequence: u64,
    ) -> Self {
        Self {
            pos,
            fluid,
            trigger_tick,
            priority,
            sequence,
        }
    }

    fn sort_key(&self) -> (u64, i32, u64) {
        (self.trigger_tick, self.priority, self.sequence)
    }

    fn same_request(&self, other: &ScheduledFluidTick) -> bool {
        self.pos == other.pos
            && self.fluid == other.fluid
            && self.trigger_tick == other.trigger_tick
            && self.priority == other.priority
    }
}

impl ScheduledBlockTick {
    #[must_use]
    pub fn new(pos: BlockPos, block: Identifier, trigger_tick: u64, priority: i32) -> Self {
        Self {
            pos,
            block,
            trigger_tick,
            priority,
            sequence: 0,
        }
    }

    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Restore an authoritative scheduler identity without assigning a new sequence.
    pub fn from_storage(
        pos: BlockPos,
        block: Identifier,
        trigger_tick: u64,
        priority: i32,
        sequence: u64,
    ) -> Self {
        Self {
            pos,
            block,
            trigger_tick,
            priority,
            sequence,
        }
    }

    fn sort_key(&self) -> (u64, i32, u64) {
        (self.trigger_tick, self.priority, self.sequence)
    }

    fn same_request(&self, other: &ScheduledBlockTick) -> bool {
        self.pos == other.pos
            && self.block == other.block
            && self.trigger_tick == other.trigger_tick
            && self.priority == other.priority
    }
}

#[derive(Debug, Clone, Eq, Default)]
pub struct FurnaceSlot {
    pub count: i32,
    pub item_id: u32,
    pub damage: Option<i32>,
    pub enchantments: Vec<mc_data::ItemEnchantment>,
    pub custom_name: Option<String>,
    pub item_model: Option<Arc<Identifier>>,
    pub stew_effects: Vec<mc_data::item_stack::StewEffect>,
    pub components: Option<Box<mc_data::item_stack::ExtendedItemComponents>>,
}

impl PartialEq for FurnaceSlot {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count
            && self.item_id == other.item_id
            && self.damage == other.damage
            && self.enchantments == other.enchantments
            && self.custom_name == other.custom_name
            && self.item_model == other.item_model
            && self.stew_effects == other.stew_effects
            && self.extended() == other.extended()
    }
}

impl FurnaceSlot {
    pub const EMPTY: FurnaceSlot = FurnaceSlot {
        count: 0,
        item_id: 0,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
        components: None,
    };

    #[must_use]
    pub fn extended(&self) -> &mc_data::item_stack::ExtendedItemComponents {
        self.components
            .as_deref()
            .unwrap_or(mc_data::item_stack::ExtendedItemComponents::empty())
    }

    pub fn extended_mut(&mut self) -> &mut mc_data::item_stack::ExtendedItemComponents {
        self.components.get_or_insert_with(Default::default)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count <= 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FurnaceBlockEntity {
    pub slots: [FurnaceSlot; 3],
    pub burn_remaining: i16,
    pub burn_total: i16,
    pub cook_progress: i16,
    pub cook_total: i16,
    pub recipes_used: BTreeMap<String, i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewingStandBlockEntity {
    pub slots: [FurnaceSlot; 5],
    pub brew_time: i16,
    pub fuel: i16,
    /// Ingredient identity at the start of the current batch.
    pub ingredient: Option<u32>,
}

impl Default for BrewingStandBlockEntity {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| FurnaceSlot::EMPTY),
            brew_time: 0,
            fuel: 0,
            ingredient: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChestBlockEntity {
    pub slots: [FurnaceSlot; 27],
    /// Unopened vanilla container loot; cleared atomically when its contents are populated.
    pub loot_table: Option<Identifier>,
    pub loot_seed: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HopperBlockEntity {
    pub slots: [FurnaceSlot; 5],
    pub transfer_cooldown: i32,
}

impl Default for ChestBlockEntity {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| FurnaceSlot::EMPTY),
            loot_table: None,
            loot_seed: 0,
        }
    }
}

impl Default for HopperBlockEntity {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| FurnaceSlot::EMPTY),
            transfer_cooldown: -1,
        }
    }
}

impl Default for FurnaceBlockEntity {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| FurnaceSlot::EMPTY),
            burn_remaining: 0,
            burn_total: 1600,
            cook_progress: 0,
            cook_total: 200,
            recipes_used: BTreeMap::new(),
        }
    }
}

/// Biome storage for one section (4³ cells, palette of `Identifier`s).
///
/// Almost every section in a generated world has a single biome, so
/// `Single` is the overwhelmingly common case.
#[derive(Debug, Clone)]
pub enum BiomeSection {
    Single(Identifier),
    Indirect {
        palette: Vec<Identifier>,
        indices: PackedBitArray,
    },
}

impl BiomeSection {
    #[must_use]
    pub fn filled(biome: Identifier) -> Self {
        BiomeSection::Single(biome)
    }

    /// Build directly from a palette + packed indices, as the Anvil
    /// codec gets them from disk.
    #[must_use]
    pub fn from_indirect(palette: Vec<Identifier>, indices: PackedBitArray) -> Self {
        assert_eq!(indices.len(), BIOME_VOLUME);
        BiomeSection::Indirect { palette, indices }
    }

    /// Read a biome cell at the section-local 4×4×4 coordinate.
    #[must_use]
    pub fn get(&self, x: u8, y: u8, z: u8) -> &Identifier {
        debug_assert!((x as usize) < BIOME_DIM);
        debug_assert!((y as usize) < BIOME_DIM);
        debug_assert!((z as usize) < BIOME_DIM);
        match self {
            BiomeSection::Single(id) => id,
            BiomeSection::Indirect { palette, indices } => {
                let idx = (y as usize * BIOME_DIM + z as usize) * BIOME_DIM + x as usize;
                let p = indices.get(idx) as usize;
                &palette[p]
            }
        }
    }

    #[must_use]
    pub fn palette(&self) -> &[Identifier] {
        match self {
            BiomeSection::Single(id) => std::slice::from_ref(id),
            BiomeSection::Indirect { palette, .. } => palette,
        }
    }
}

/// One named heightmap (e.g. `MOTION_BLOCKING`, `WORLD_SURFACE`).
///
/// Stores 256 entries of `HEIGHTMAP_BITS` bits each, indexed by
/// `(z * 16) + x` (vanilla's heightmap indexing — note Z is outer, X
/// inner; this differs from the section's (y, z, x) order and matches
/// what `.mca` files contain).
#[derive(Debug, Clone)]
pub struct Heightmap {
    data: PackedBitArray,
}

impl Heightmap {
    #[must_use]
    pub fn zeroed() -> Self {
        Self {
            data: PackedBitArray::zeroed(HEIGHTMAP_BITS, HEIGHTMAP_LEN),
        }
    }

    /// Build a heightmap directly from the packed `i64[]` Anvil stores
    /// on disk (a single NBT `LongArray` per heightmap kind).
    #[must_use]
    pub fn from_long_array(longs: &[i64]) -> Self {
        let words: Vec<u64> = longs.iter().map(|&l| l as u64).collect();
        Self {
            data: PackedBitArray::from_words(HEIGHTMAP_BITS, HEIGHTMAP_LEN, words),
        }
    }

    /// Raw long-array form for emission back to NBT.
    #[must_use]
    pub fn to_long_array(&self) -> Vec<i64> {
        self.data.words().iter().map(|&w| w as i64).collect()
    }

    #[must_use]
    pub fn get(&self, x: u8, z: u8) -> u32 {
        debug_assert!((x as usize) < SECTION_DIM);
        debug_assert!((z as usize) < SECTION_DIM);
        self.data.get(z as usize * SECTION_DIM + x as usize)
    }

    pub fn set(&mut self, x: u8, z: u8, height: u32) {
        debug_assert!((x as usize) < SECTION_DIM);
        debug_assert!((z as usize) < SECTION_DIM);
        self.data.set(z as usize * SECTION_DIM + x as usize, height);
    }

    /// Raw packed words, for the Anvil codec.
    #[must_use]
    pub fn words(&self) -> &[u64] {
        self.data.words()
    }
}

/// A known section light channel in vanilla low-nibble-first cell order.
/// Uniform packed bytes allocate nothing; mixed layers share immutable storage
/// until a cell is changed. Absence belongs to [`SectionLight`], not this type.
#[derive(Debug, Clone)]
pub struct LightSection {
    storage: LightSectionStorage,
    nonzero_nibbles: u16,
}

#[derive(Debug, Clone)]
enum LightSectionStorage {
    Uniform(u8),
    Shared(Arc<[u8; LIGHT_LAYER_BYTES]>),
}

impl LightSection {
    #[must_use]
    pub fn uniform(packed_byte: u8) -> Self {
        Self {
            storage: LightSectionStorage::Uniform(packed_byte),
            nonzero_nibbles: (u16::from(packed_byte & 0x0F != 0)
                + u16::from(packed_byte >> 4 != 0))
                * LIGHT_LAYER_BYTES as u16,
        }
    }

    #[must_use]
    pub fn from_bytes(bytes: [u8; LIGHT_LAYER_BYTES]) -> Self {
        if bytes.iter().all(|&byte| byte == bytes[0]) {
            return Self::uniform(bytes[0]);
        }
        let nonzero_nibbles = bytes
            .iter()
            .map(|byte| u16::from(byte & 0x0F != 0) + u16::from(byte >> 4 != 0))
            .sum();
        Self {
            storage: LightSectionStorage::Shared(Arc::new(bytes)),
            nonzero_nibbles,
        }
    }

    /// Reuse the nibble count already accumulated by the light packing kernel.
    pub(crate) fn from_bytes_with_nonzero_count(
        bytes: [u8; LIGHT_LAYER_BYTES],
        nonzero_nibbles: u16,
    ) -> Self {
        if nonzero_nibbles == 0 {
            return Self::uniform(0);
        }
        if bytes.iter().all(|&byte| byte == bytes[0]) {
            return Self::uniform(bytes[0]);
        }
        Self {
            storage: LightSectionStorage::Shared(Arc::new(bytes)),
            nonzero_nibbles,
        }
    }

    #[must_use]
    pub fn byte(&self, index: usize) -> u8 {
        assert!(index < LIGHT_LAYER_BYTES);
        match &self.storage {
            LightSectionStorage::Uniform(byte) => *byte,
            LightSectionStorage::Shared(bytes) => bytes[index],
        }
    }

    #[must_use]
    pub fn get(&self, cell: usize) -> u8 {
        (self.byte(cell / 2) >> ((cell & 1) * 4)) & 0x0F
    }

    pub fn set(&mut self, cell: usize, value: u8) {
        assert!(value <= 15);
        let old = self.get(cell);
        if old == value {
            return;
        }
        if let LightSectionStorage::Uniform(byte) = &self.storage {
            self.storage = LightSectionStorage::Shared(Arc::new([*byte; LIGHT_LAYER_BYTES]));
        }
        let LightSectionStorage::Shared(bytes) = &mut self.storage else {
            unreachable!()
        };
        let byte = &mut Arc::make_mut(bytes)[cell / 2];
        let shift = (cell & 1) * 4;
        *byte = (*byte & !(0x0F << shift)) | (value << shift);
        self.nonzero_nibbles = self.nonzero_nibbles - u16::from(old != 0) + u16::from(value != 0);
        if self.is_zero() {
            self.storage = LightSectionStorage::Uniform(0);
        }
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.nonzero_nibbles == 0
    }

    pub fn bytes(&self) -> impl ExactSizeIterator<Item = u8> + '_ {
        (0..LIGHT_LAYER_BYTES).map(|index| self.byte(index))
    }

    /// Expand only at a vanilla serialization boundary.
    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        match &self.storage {
            LightSectionStorage::Uniform(byte) => vec![*byte; LIGHT_LAYER_BYTES],
            LightSectionStorage::Shared(bytes) => bytes.to_vec(),
        }
    }

    /// Heap bytes referenced by this layer, including the Arc counters.
    /// Shared owners must deduplicate [`Self::shared_allocation`] for a physical total.
    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        self.shared_allocation().map_or(0, |(_, bytes)| bytes)
    }

    /// Stable allocation identity and size while any owning layer remains alive.
    #[must_use]
    pub fn shared_allocation(&self) -> Option<(*const u8, usize)> {
        match &self.storage {
            LightSectionStorage::Uniform(_) => None,
            LightSectionStorage::Shared(bytes) => {
                Some((bytes.as_ptr(), LIGHT_LAYER_BYTES + 2 * size_of::<usize>()))
            }
        }
    }
}

impl PartialEq for LightSection {
    fn eq(&self, other: &Self) -> bool {
        if self.nonzero_nibbles != other.nonzero_nibbles {
            return false;
        }
        match (&self.storage, &other.storage) {
            (LightSectionStorage::Uniform(a), LightSectionStorage::Uniform(b)) => a == b,
            (LightSectionStorage::Shared(a), LightSectionStorage::Shared(b)) => {
                Arc::ptr_eq(a, b) || a == b
            }
            _ => self.bytes().eq(other.bytes()),
        }
    }
}

impl Eq for LightSection {}

/// Per-section light channels. `None` is missing/unknown, distinct from a
/// present allocation-free zero layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionLight {
    pub block: Option<LightSection>,
    pub sky: Option<LightSection>,
}

/// A 16×height×16 column.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub pos: ChunkPos,
    geometry: ChunkGeometry,
    pub sections: Vec<ChunkSection>,
    pub biomes: Vec<BiomeSection>,
    /// Named heightmaps as Anvil stores them: `MOTION_BLOCKING`,
    /// `MOTION_BLOCKING_NO_LEAVES`, `OCEAN_FLOOR`, `WORLD_SURFACE`,
    /// plus the worldgen-time `*_WG` variants. We don't enforce the
    /// set here — the Anvil codec stores whatever it reads.
    pub heightmaps: HashMap<String, Heightmap>,
    /// Derived highest sky-blocking cell per column, stored as the
    /// same `top + 1 - MIN_Y` offset as vanilla heightmaps. This is not
    /// serialized as an Anvil heightmap; it is maintained from the
    /// light table for spawn picking and sky-relight shortcuts.
    pub highest_opaque: Heightmap,
    /// Opaque Java-standard block-entity NBT, keyed by absolute world position.
    /// Supported runtime block entities are also represented in the typed maps below.
    pub block_entities: HashMap<BlockPos, Vec<u8>>,
    /// Runtime-typed furnace state keyed by absolute world position.
    pub furnaces: HashMap<BlockPos, FurnaceBlockEntity>,
    /// Runtime-typed single chest state keyed by absolute world position.
    pub chests: HashMap<BlockPos, ChestBlockEntity>,
    /// Runtime-typed hopper state keyed by absolute world position.
    pub hoppers: HashMap<BlockPos, HopperBlockEntity>,
    pub brewing_stands: HashMap<BlockPos, BrewingStandBlockEntity>,
    /// Chunk-owned scheduled block ticks, ordered by trigger tick, priority,
    /// and insertion sequence and persisted under Anvil's `block_ticks`.
    pub scheduled_block_ticks: Vec<ScheduledBlockTick>,
    next_scheduled_block_tick_sequence: u64,
    /// Chunk-owned scheduled fluid ticks. Mirrors block tick ordering but is
    /// kept separate because vanilla persists them under `fluid_ticks`.
    pub scheduled_fluid_ticks: Vec<ScheduledFluidTick>,
    next_scheduled_fluid_tick_sequence: u64,
    /// Vanilla generation status (`"full"`, `"biomes"`, `"structure_starts"`, …).
    pub status: String,
    /// Baked light arrays read from Anvil, one entry per
    /// `sections[i]`. Decoded by `anvil::chunk_nbt` when present;
    /// `Chunk::empty` initialises everything to `None` (sections
    /// have no baked light). The wire encoder may use these as the
    /// source of truth or fall back to recomputed light — see
    /// `mc_world::light`.
    pub section_lights: Vec<SectionLight>,
    /// Vanilla light-only sections immediately below/above the block geometry.
    pub boundary_section_lights: [SectionLight; 2],
    /// Unmodelled root-level NBT fields retained for stable Anvil round-trips,
    /// including structures, `PostProcessing`, `InhabitedTime`, `LastUpdate`,
    /// `DataVersion`, and future fields outside the modelled set.
    /// Order is preserved from the original
    /// compound so a load-then-save produces a stable byte stream
    /// (modulo NBT compound ordering being unspecified — vanilla
    /// reads it back identically either way).
    pub extras: Vec<(String, Tag)>,
    /// Set when chunk state changes. Decoders default this to `false`.
    pub dirty: bool,
}

/// One generated vanilla village, as the generator that places it describes it.
///
/// Plain data: the generator's own answer about a bounded chunk region, with no
/// world state and no identity — a caller that needs an id mints it from the
/// world it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedVillageSite {
    /// The chunk the placement formula started this village in.
    pub start_chunk: (i32, i32),
    /// The structure id that placed this village (`minecraft:village_plains`,
    /// ...): the assembly the gate resolved, so callers can filter by kind.
    pub structure: Identifier,
    /// The region the plan reaches, in world blocks, inclusive.
    pub min: BlockPos,
    pub max: BlockPos,
    /// Every placed piece, in placement order.
    pub pieces: Vec<GeneratedVillagePiece>,
}

/// One piece of a generated village: the template it places at its rotation, or
/// a placed feature, which places no template.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedVillagePiece {
    /// The template the piece places; `None` for a `feature_pool_element`.
    pub template: Option<String>,
    pub position: BlockPos,
    pub rotation: u16,
    /// Inclusive world-space template bounds from the placement solver.
    /// Features have no template geometry to certify.
    pub bounds: Option<(BlockPos, BlockPos)>,
    /// The template-pool owner used in generated entity claims. This is not
    /// necessarily the template ID (aliases/pools may refer another template).
    pub entity_owner: Option<String>,
}

/// A generated non-village structure, using the same start decision as placement.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedStructureSite {
    pub structure: Identifier,
    pub position: BlockPos,
    pub start_chunk: (i32, i32),
}

pub trait ChunkGenerator: Send + Sync {
    /// Build a brand-new `Chunk` for the given position.
    /// Complete light arrays, when supplied, must describe the final blocks.
    /// Omit light layers after changing a prelit template; loaders prepare them.
    fn generate(&self, pos: ChunkPos) -> Chunk;

    /// Topmost solid terrain row at an absolute column, before any edit.
    ///
    /// Settlement layout grounds its buildings on the same generator the world
    /// generates from, so the terrain that decides a site's base row is the
    /// terrain the players will stand on. Generators that cannot answer from an
    /// analytic height field return `None`, and grounding then fails closed
    /// instead of inventing a level.
    fn surface_height(&self, _world_x: i32, _world_z: i32) -> Option<i32> {
        None
    }

    /// Every vanilla village whose start chunk lies in the inclusive chunk
    /// rectangle, in ascending start-chunk order.
    ///
    /// The answer is the generator's own placement decision over a bounded
    /// region: no chunk is generated and no world state is read, so a caller can
    /// enumerate the villages a region holds before any of them exists. A
    /// generator that places no villages returns an empty list; it is not an
    /// error.
    fn village_sites_in_region(
        &self,
        _min_chunk: (i32, i32),
        _max_chunk: (i32, i32),
    ) -> Vec<GeneratedVillageSite> {
        Vec::new()
    }

    /// Non-village starts in a bounded inclusive chunk rectangle.
    fn structure_sites_in_region(
        &self,
        _min_chunk: (i32, i32),
        _max_chunk: (i32, i32),
    ) -> Vec<GeneratedStructureSite> {
        Vec::new()
    }

    /// The generator's 3D noise biome at a block position, before any edit:
    /// the same field chunk palettes and structure gates read. Generators
    /// without a noise biome return `None`, and `/locate biome` then reports
    /// `not_found` instead of inventing an answer.
    fn noise_biome(&self, _world_x: i32, _y: i32, _world_z: i32) -> Option<Identifier> {
        None
    }

    /// Far-terrain LOD summary of a column that was never generated: the
    /// generator's cheap surface (top solid block Y) and biome, without caves,
    /// structures or decoration. `None` when the generator has no such field.
    fn lod_column(&self, _world_x: i32, _world_z: i32) -> Option<(i32, Identifier)> {
        None
    }

    /// Encoded far-terrain generation program that lets a native client
    /// compute [`Self::lod_column`] itself (`mc_worldgen` documents the
    /// format). It reveals the seed's terrain noise; `Ok(None)` when the
    /// generator has none.
    fn lod_program(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}

impl Chunk {
    /// A column filled with `air` blocks and `biome` everywhere, no
    /// block entities, no heightmaps, status `"full"`.
    #[must_use]
    pub fn empty(pos: ChunkPos, air: BlockStateId, biome: Identifier) -> Self {
        Self::empty_with_geometry(pos, air, biome, OVERWORLD_GEOMETRY)
    }

    #[must_use]
    pub fn empty_with_geometry(
        pos: ChunkPos,
        air: BlockStateId,
        biome: Identifier,
        geometry: ChunkGeometry,
    ) -> Self {
        let section_count = geometry.section_count();
        Self {
            pos,
            geometry,
            sections: (0..section_count)
                .map(|_| ChunkSection::filled(air, air))
                .collect(),
            biomes: (0..section_count)
                .map(|_| BiomeSection::filled(biome.clone()))
                .collect(),
            heightmaps: HashMap::new(),
            highest_opaque: Heightmap::zeroed(),
            block_entities: HashMap::new(),
            furnaces: HashMap::new(),
            chests: HashMap::new(),
            hoppers: HashMap::new(),
            brewing_stands: HashMap::new(),
            scheduled_block_ticks: Vec::new(),
            next_scheduled_block_tick_sequence: 0,
            scheduled_fluid_ticks: Vec::new(),
            next_scheduled_fluid_tick_sequence: 0,
            status: "full".to_string(),
            section_lights: vec![SectionLight::default(); section_count],
            boundary_section_lights: [SectionLight::default(), SectionLight::default()],
            extras: Vec::new(),
            dirty: false,
        }
    }

    pub fn set_generated_entities(&mut self, inhabitants: &[GeneratedEntityMarker]) {
        self.extras.retain(|(key, _)| key != GENERATED_ENTITIES_KEY);
        if inhabitants.is_empty() {
            return;
        }
        let elements = inhabitants
            .iter()
            .map(|inhabitant| {
                Tag::Compound(vec![
                    ("Claim".into(), Tag::String(inhabitant.claim.clone())),
                    ("Entity".into(), Tag::String(inhabitant.entity_type.clone())),
                    ("X".into(), Tag::Double(inhabitant.position[0])),
                    ("Y".into(), Tag::Double(inhabitant.position[1])),
                    ("Z".into(), Tag::Double(inhabitant.position[2])),
                    (
                        "PersistenceRequired".into(),
                        Tag::Byte(i8::from(inhabitant.persistent)),
                    ),
                    (
                        "PersistenceRequired".into(),
                        Tag::Byte(i8::from(inhabitant.persistent)),
                    ),
                    (
                        "VillagerKind".into(),
                        Tag::String(inhabitant.villager_kind.clone()),
                    ),
                    (
                        "Profession".into(),
                        Tag::String(inhabitant.profession.clone()),
                    ),
                    ("Level".into(), Tag::Int(i32::from(inhabitant.level))),
                    ("Age".into(), Tag::Int(inhabitant.age)),
                    (
                        "Yaw".into(),
                        Tag::Float(if inhabitant.yaw.is_finite() {
                            inhabitant.yaw
                        } else {
                            0.0
                        }),
                    ),
                    (
                        "Pitch".into(),
                        Tag::Float(if inhabitant.pitch.is_finite() {
                            inhabitant.pitch
                        } else {
                            0.0
                        }),
                    ),
                ])
            })
            .collect();
        self.extras.push((
            GENERATED_ENTITIES_KEY.into(),
            Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements,
            }),
        ));
    }

    #[must_use]
    pub fn generated_entities(&self) -> Vec<GeneratedEntityMarker> {
        let Some(Tag::List(list)) = self
            .extras
            .iter()
            .find(|(key, _)| key == GENERATED_ENTITIES_KEY)
            .map(|(_, value)| value)
        else {
            return Vec::new();
        };
        list.elements
            .iter()
            .filter_map(|entry| {
                let Tag::Compound(fields) = entry else {
                    return None;
                };
                let string = |name: &str| {
                    fields.iter().find_map(|(key, value)| {
                        (key == name)
                            .then_some(value)
                            .and_then(|value| match value {
                                Tag::String(value) => Some(value.clone()),
                                _ => None,
                            })
                    })
                };
                let double = |name: &str| {
                    fields.iter().find_map(|(key, value)| {
                        (key == name)
                            .then_some(value)
                            .and_then(|value| match value {
                                Tag::Double(value) if value.is_finite() => Some(*value),
                                _ => None,
                            })
                    })
                };
                let level = fields.iter().find_map(|(key, value)| {
                    (key == "Level")
                        .then_some(value)
                        .and_then(|value| match value {
                            Tag::Int(value) => u8::try_from(*value).ok(),
                            _ => None,
                        })
                        .filter(|value| (1..=5).contains(value))
                });
                let integer = |name: &str| {
                    fields.iter().find_map(|(key, value)| {
                        (key == name)
                            .then_some(value)
                            .and_then(|value| match value {
                                Tag::Int(value) => Some(*value),
                                _ => None,
                            })
                    })
                };
                let float = |name: &str| {
                    fields.iter().find_map(|(key, value)| {
                        (key == name)
                            .then_some(value)
                            .and_then(|value| match value {
                                Tag::Float(value) if value.is_finite() => Some(*value),
                                _ => None,
                            })
                    })
                };
                Some(GeneratedEntityMarker {
                    claim: string("Claim")?,
                    entity_type: string("Entity")?,
                    position: [double("X")?, double("Y")?, double("Z")?],
                    persistent: fields.iter().any(|(key, value)| {
                        key == "PersistenceRequired"
                            && matches!(value, Tag::Byte(value) if *value != 0)
                    }),
                    villager_kind: string("VillagerKind")?,
                    profession: string("Profession")?,
                    level: level?,
                    age: integer("Age").unwrap_or(0),
                    yaw: float("Yaw").unwrap_or(0.0),
                    pitch: float("Pitch").unwrap_or(0.0),
                })
            })
            .collect()
    }

    #[must_use]
    pub const fn geometry(&self) -> ChunkGeometry {
        self.geometry
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Number of game ticks players have kept this chunk active.
    #[must_use]
    pub fn inhabited_time(&self) -> i64 {
        self.extras
            .iter()
            .find_map(|(key, value)| {
                (key == "InhabitedTime")
                    .then_some(value)
                    .and_then(|value| match value {
                        Tag::Long(value) => Some(*value),
                        _ => None,
                    })
            })
            .unwrap_or(0)
    }

    /// Add active game ticks without dirtying the chunk, like vanilla ChunkAccess.
    pub fn increment_inhabited_time(&mut self, elapsed_ticks: u64) {
        if elapsed_ticks == 0 {
            return;
        }
        let elapsed_ticks = i64::try_from(elapsed_ticks).unwrap_or(i64::MAX);
        let next = self.inhabited_time().max(0).saturating_add(elapsed_ticks);
        self.extras.retain(|(key, _)| key != "InhabitedTime");
        self.extras
            .push(("InhabitedTime".to_string(), Tag::Long(next)));
    }

    #[must_use]
    pub fn world_journal_lsn(&self) -> u64 {
        self.extras
            .iter()
            .find_map(|(key, value)| (key == WORLD_JOURNAL_LSN_KEY).then_some(value))
            .and_then(|value| match value {
                Tag::Long(value) if *value >= 0 => Some(*value as u64),
                _ => None,
            })
            .unwrap_or(0)
    }

    pub(crate) fn set_world_journal_lsn(&mut self, lsn: u64) {
        let value = i64::try_from(lsn).expect("world journal LSN must fit a nonnegative NBT long");
        if self
            .extras
            .iter()
            .any(|(key, tag)| key == WORLD_JOURNAL_LSN_KEY && *tag == Tag::Long(value))
        {
            return;
        }
        self.extras.retain(|(key, _)| key != WORLD_JOURNAL_LSN_KEY);
        self.extras
            .push((WORLD_JOURNAL_LSN_KEY.to_string(), Tag::Long(value)));
        self.mark_dirty();
    }

    pub fn set_baked_light(&mut self, light: &ChunkLight) {
        if light.write_section_lights(&mut self.section_lights) {
            self.mark_dirty();
        }
    }

    fn clear_baked_light(&mut self) {
        for section in &mut self.section_lights {
            *section = SectionLight::default();
        }
    }

    /// Look up a block by chunk-local (x, z) and absolute world y.
    /// `None` when `y` is outside this chunk's geometry.
    #[must_use]
    #[inline]
    pub fn get_block(&self, x: u8, y: i32, z: u8) -> Option<BlockStateId> {
        let (idx, sy) = self.geometry.world_y_to_section(y)?;
        Some(self.sections[idx].get(x, sy, z))
    }

    /// Set a block; returns the previous state, or `None` if `y` is
    /// outside the chunk.
    pub fn set_block(&mut self, x: u8, y: i32, z: u8, state: BlockStateId) -> Option<BlockStateId> {
        let (idx, sy) = self.geometry.world_y_to_section(y)?;
        Some(self.sections[idx].set(x, sy, z, state))
    }

    /// Set a block *and* refresh every heightmap currently attached
    /// to this chunk so the new top-non-air-column matches reality.
    /// Returns the previous state (or `None` if `y` is outside the
    /// chunk). `air` is the registry's default air state — the
    /// heightmap predicate is `state != air`, matching vanilla's
    /// `MOTION_BLOCKING` / `WORLD_SURFACE` / `OCEAN_FLOOR`
    /// definitions closely enough for M5's flat-preset world.
    ///
    /// Recomputes *every* heightmap currently present on the chunk,
    /// not a hardcoded set — different vanilla generators produce
    /// different subsets (e.g. mid-generation `*_WG` variants), so
    /// touching only known names would silently leave stale values
    /// on partial chunks.
    pub fn set_block_and_update(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        state: BlockStateId,
        air: BlockStateId,
    ) -> Option<BlockStateId> {
        let prev = self.set_block(x, y, z, state)?;
        if prev == state {
            return Some(prev);
        }
        self.mark_dirty();
        self.clear_baked_light();
        // Heightmap entries store `height + 1`-style values (the Y of
        // the first air cell above the column), matching vanilla's
        // on-disk packing. Recompute the column for every present
        // heightmap. Cheap: one walk down per heightmap per edit.
        let names: Vec<String> = self.heightmaps.keys().cloned().collect();
        for name in names {
            let new_top = top_non_air_column(self, x, z, air);
            if let Some(hm) = self.heightmaps.get_mut(&name) {
                hm.set(x, z, new_top);
            }
        }
        Some(prev)
    }

    pub fn rebuild_highest_opaque(&mut self, table: &BlockLightTable) {
        for z in 0..SECTION_DIM as u8 {
            for x in 0..SECTION_DIM as u8 {
                self.update_highest_opaque_column(x, z, table);
            }
        }
    }

    pub fn update_highest_opaque_column(&mut self, x: u8, z: u8, table: &BlockLightTable) {
        let top = top_opaque_column(self, x, z, table);
        self.highest_opaque.set(x, z, top);
    }

    #[must_use]
    pub fn highest_opaque_y(&self, x: u8, z: u8) -> Option<i32> {
        heightmap_value_to_world_y_at(self.geometry.min_y(), self.highest_opaque.get(x, z))
    }

    #[must_use]
    pub fn scheduled_block_ticks(&self) -> &[ScheduledBlockTick] {
        &self.scheduled_block_ticks
    }

    pub fn schedule_block_tick(&mut self, mut tick: ScheduledBlockTick) -> bool {
        if !self.contains_block_pos(tick.pos)
            || self
                .scheduled_block_ticks
                .iter()
                .any(|existing| existing.same_request(&tick))
        {
            return false;
        }
        tick.sequence = self.next_scheduled_block_tick_sequence;
        self.next_scheduled_block_tick_sequence =
            self.next_scheduled_block_tick_sequence.wrapping_add(1);
        self.scheduled_block_ticks.push(tick);
        self.scheduled_block_ticks
            .sort_by_key(ScheduledBlockTick::sort_key);
        self.mark_dirty();
        true
    }

    /// Replace the queue from an authoritative scheduler or decoded storage.
    /// Callers performing a live mutation must also mark the chunk dirty.
    pub fn load_scheduled_block_ticks(&mut self, mut ticks: Vec<ScheduledBlockTick>) {
        ticks.sort_by_key(ScheduledBlockTick::sort_key);
        self.next_scheduled_block_tick_sequence = ticks
            .iter()
            .map(ScheduledBlockTick::sequence)
            .max()
            .map_or(0, |sequence| sequence.wrapping_add(1));
        self.scheduled_block_ticks = ticks;
    }

    pub fn remove_scheduled_block_ticks_at(&mut self, pos: BlockPos) -> Vec<ScheduledBlockTick> {
        let before = self.scheduled_block_ticks.len();
        let mut removed = Vec::new();
        self.scheduled_block_ticks.retain(|tick| {
            if tick.pos == pos {
                removed.push(tick.clone());
                false
            } else {
                true
            }
        });
        if self.scheduled_block_ticks.len() != before {
            self.mark_dirty();
        }
        removed
    }

    pub fn drain_due_block_ticks(
        &mut self,
        world_tick: u64,
        max_ticks: usize,
    ) -> Vec<ScheduledBlockTick> {
        if max_ticks == 0 {
            return Vec::new();
        }
        let due_count = self
            .scheduled_block_ticks
            .partition_point(|tick| tick.trigger_tick <= world_tick)
            .min(max_ticks);
        if due_count == 0 {
            return Vec::new();
        }
        self.mark_dirty();
        self.scheduled_block_ticks.drain(0..due_count).collect()
    }

    #[must_use]
    pub fn scheduled_fluid_ticks(&self) -> &[ScheduledFluidTick] {
        &self.scheduled_fluid_ticks
    }

    pub fn schedule_fluid_tick(&mut self, mut tick: ScheduledFluidTick) -> bool {
        if !self.contains_block_pos(tick.pos)
            || self
                .scheduled_fluid_ticks
                .iter()
                .any(|existing| existing.same_request(&tick))
        {
            return false;
        }
        tick.sequence = self.next_scheduled_fluid_tick_sequence;
        self.next_scheduled_fluid_tick_sequence =
            self.next_scheduled_fluid_tick_sequence.wrapping_add(1);
        self.scheduled_fluid_ticks.push(tick);
        self.scheduled_fluid_ticks
            .sort_by_key(ScheduledFluidTick::sort_key);
        self.mark_dirty();
        true
    }

    pub fn load_scheduled_fluid_ticks(&mut self, mut ticks: Vec<ScheduledFluidTick>) {
        ticks.sort_by_key(ScheduledFluidTick::sort_key);
        self.next_scheduled_fluid_tick_sequence = ticks
            .iter()
            .map(ScheduledFluidTick::sequence)
            .max()
            .map_or(0, |sequence| sequence.wrapping_add(1));
        self.scheduled_fluid_ticks = ticks;
    }

    pub fn remove_scheduled_fluid_ticks_at(&mut self, pos: BlockPos) -> Vec<ScheduledFluidTick> {
        let before = self.scheduled_fluid_ticks.len();
        let mut removed = Vec::new();
        self.scheduled_fluid_ticks.retain(|tick| {
            if tick.pos == pos {
                removed.push(tick.clone());
                false
            } else {
                true
            }
        });
        if self.scheduled_fluid_ticks.len() != before {
            self.mark_dirty();
        }
        removed
    }

    pub fn drain_due_fluid_ticks(
        &mut self,
        world_tick: u64,
        max_ticks: usize,
    ) -> Vec<ScheduledFluidTick> {
        if max_ticks == 0 {
            return Vec::new();
        }
        let due_count = self
            .scheduled_fluid_ticks
            .partition_point(|tick| tick.trigger_tick <= world_tick)
            .min(max_ticks);
        if due_count == 0 {
            return Vec::new();
        }
        self.mark_dirty();
        self.scheduled_fluid_ticks.drain(0..due_count).collect()
    }

    #[must_use]
    pub fn contains_block_pos(&self, pos: BlockPos) -> bool {
        pos.y >= self.geometry.min_y()
            && pos.y < self.geometry.max_y()
            && pos.x.div_euclid(SECTION_DIM as i32) == self.pos.x
            && pos.z.div_euclid(SECTION_DIM as i32) == self.pos.z
    }
}

/// Walk column `(x, z)` from `MAX_Y - 1` down to `MIN_Y` and return
/// the heightmap value vanilla stores: `top_y + 1 - MIN_Y` (the Y
/// of the first air cell above the topmost non-air block, expressed
/// as an offset from the bottom of the world). For an entirely-air
/// column the value is `0`, matching vanilla's "empty column"
/// convention.
fn top_non_air_column(chunk: &Chunk, x: u8, z: u8, air: BlockStateId) -> u32 {
    let geometry = chunk.geometry();
    for y in (geometry.min_y()..geometry.max_y()).rev() {
        if chunk.get_block(x, y, z) != Some(air) {
            // vanilla packs heightmap values as `height - MIN_Y + 1`
            // so a top block at world Y=64 stores as `64 - (-64) + 1`
            // = 129. The +1 makes "the lowest cell above the top
            // block" the value, matching the meaning vanilla uses
            // for chunk-spawning and skylight shortcuts.
            return (y - geometry.min_y() + 1) as u32;
        }
    }
    0
}

pub fn top_opaque_column(chunk: &Chunk, x: u8, z: u8, table: &BlockLightTable) -> u32 {
    let geometry = chunk.geometry();
    for y in (geometry.min_y()..geometry.max_y()).rev() {
        let Some(state) = chunk.get_block(x, y, z) else {
            continue;
        };
        if !table.propagates_sky(state.0).unwrap_or(true) {
            return (y - geometry.min_y() + 1) as u32;
        }
    }
    0
}

#[must_use]
pub fn heightmap_value_to_world_y(value: u32) -> Option<i32> {
    heightmap_value_to_world_y_at(MIN_Y, value)
}

fn heightmap_value_to_world_y_at(min_y: i32, value: u32) -> Option<i32> {
    if value == 0 {
        None
    } else {
        Some(min_y + value as i32 - 1)
    }
}

#[cfg(test)]
#[path = "chunk_tests.rs"]
mod tests;
