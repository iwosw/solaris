//! Pre-2860 chunk fixers: 2527 bit-storage alignment, 2832 height/biomes,
//! 2838 biome renames, 2841 proto tick lists, 2842 chunk renames, 2843
//! overreaching ticks.
//!
//! Bytecode authority: local official 26.1.2 `server-26.1.2.jar`
//! (`.analysis/maps/falcons-rock/vanilla-hop/server/versions/26.1.2/server-26.1.2.jar`),
//! registrations in `.analysis/codex-logs/old-dataversion/DataFixers.javap`
//! (2527 `BitStorageAlignFix`,
//! 2832 `ChunkHeightAndBiomeFix` + `WorldGenSettingsHeightAndBiomeFix`,
//! 2838 `NamespacedTypeRenameFix` "Caves and Cliffs biome renames" over
//! `CavesAndCliffsRenames.RENAMES`, 2841 `ChunkProtoTickListFix`,
//! 2842 `ChunkRenamesFix` ("Chunk Renames; purge Level-tag"),
//! 2843 `OverreachingTickFix` ("Handle ticks saved in the wrong chunk") +
//! `NamespacedTypeRenameFix` "Remove Deep Warm Ocean"; Falcons ground truth
//! needs 2527 only among the pre-2586 chunk versions, so no other pre-2701
//! inventory is attempted).
//! Per-fixer evidence in
//! `.analysis/codex-logs/old-dataversion/chunks-*.javap` (`BitStorageAlignFix`,
//! `ChunkHeightAndBiomeFix`, `ChunkProtoTickListFix`, `ChunkRenamesFix`,
//! `OverreachingTickFix`, `CavesAndCliffsRenames`, `PoorMansPalettedContainer`,
//! `ChunkMap` for `__context`, `SerializableChunkData` for the modern writer).
//! `Dynamic.update` missing-key no-op semantics verified against the DFU
//! `DynamicOps` default (`chunks-DFU-DynamicOps.javap`); every alleged
//! `update` below was checked for set-vs-update behavior.
//!
//! Scope: chunk-record structural transforms only. Simple block/item renames,
//! spawner data, arrow pickup, entity/block-entity/item/text payloads and the
//! tree walk belong to the parent (`OldDataVersion` owns `mod.rs`/`walk.rs`)
//! and siblings. 2701 `SavedDataFeaturePoolElementFix` lives in
//! `old_chunks/feature_pool.rs` (parent-landed, reviewed here, not edited).
//!
//! Integration contract (parent owns `mod.rs`/`walk.rs`): call
//! `old_chunks::apply(tag, version, report)` once per version in
//! `[2527, 2701, 2832, 2838, 2841, 2842, 2843]` for `NodeKind::Chunk`, before
//! walker descent. The caller supplies the legacy wrapper root (`{Level, ...}`)
//! for versions below 2842 and the modern root thereafter. `__context`
//! (`{dimension, generator?}`) is read from the outer root and never mutated:
//! missing `dimension`/`generator` default to `""` exactly like vanilla
//! (`ChunkHeightAndBiomeFix.lambda$makeRule$1`, `orElse("")`), so a
//! dimension-only context is sufficient and no synthetic `minecraft:noise` or
//! `minecraft:flat` default is ever inferred. The four-arg
//! `upgrade_chunk_in_dimension` helper already passes the real per-dimension
//! worldgen generator id through; absence stays vanilla-correct, never an
//! error.
//!
//! 2841 notes: the block-tick output is still `TileTicks` (capital; 2842
//! renames it to `block_ticks`); only the fluid side is lowercase
//! `fluid_ticks` from the start. `yPos` is `Dynamic.update`, so an absent key
//! stays absent and a present key becomes `Tag::Byte` exactly like vanilla
//! (`Dynamic.createByte`); later normalization to `Int` is the modern
//! writer/decoder's job, never this module's. The section-Y minimum tracks
//! sections carrying `biomes`, while the packed palette readers come from
//! `block_states` independently.

use super::{UpgradeError, UpgradeReport, nbt};
use mc_nbt::{ListTag, Tag, tag_type};

mod feature_pool;

/// Migrate one chunk record at one legacy version. All other versions are
/// no-ops; the parent wires version order and the tree walk.
pub(super) fn apply(
    tag: &mut Tag,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    match version {
        2527 => {
            if align_bit_storage(tag) {
                report.applied(2527, "BitStorageAlignFix");
            }
        }
        2701 => {
            if feature_pool::apply(tag) {
                report.applied(2701, "SavedDataFeaturePoolElementFix");
            }
        }
        2832 => {
            height_and_biome(tag);
            report.applied(2832, "ChunkHeightAndBiomeFix");
        }
        2838 => {
            let changed = rename_biomes(sections_mut(&mut *tag, false), &CAVES_AND_CLIFFS_RENAMES);
            if changed {
                report.applied(2838, "NamespacedTypeRenameFix");
            }
        }
        2841 => {
            if proto_tick_list(tag) {
                report.applied(2841, "ChunkProtoTickListFix");
            }
        }
        2842 => {
            if chunk_renames(tag) {
                report.applied(2842, "ChunkRenamesFix");
            }
        }
        2843 => {
            if overreaching_ticks(&mut *tag) {
                report.applied(2843, "OverreachingTickFix");
            }
            let changed = rename_biomes(sections_mut(&mut *tag, true), &DEEP_WARM_OCEAN_RENAME);
            if changed {
                report.applied(2843, "NamespacedTypeRenameFix");
            }
        }
        _ => {}
    }
    Ok(())
}

/// The pre-2842 working compound: the legacy `Level` wrapper when present,
/// otherwise the tag itself.
fn chunk_level_mut(tag: &mut Tag) -> &mut Tag {
    if nbt::get(tag, "Level").is_some_and(|level| nbt::fields(level).is_some()) {
        nbt::get_mut(tag, "Level").expect("checked legacy wrapper")
    } else {
        tag
    }
}

/// Mutable section list for biome renames. Pre-2842 the list lives under the
/// legacy wrapper as `Sections` (inner biome containers are lowercase
/// `biomes` since 2832); post-2842 it is root `sections`.
fn sections_mut(tag: &mut Tag, modern: bool) -> Option<&mut Vec<Tag>> {
    if modern {
        return nbt::get_mut(tag, "sections").and_then(nbt::list_mut);
    }
    if nbt::get(chunk_level_mut(&mut *tag), "Sections").is_some() {
        return nbt::get_mut(chunk_level_mut(&mut *tag), "Sections").and_then(nbt::list_mut);
    }
    nbt::get_mut(chunk_level_mut(&mut *tag), "sections").and_then(nbt::list_mut)
}

fn text(value: &str) -> Tag {
    Tag::String(value.into())
}

fn compound_list(elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: elements.first().map_or(tag_type::END, Tag::type_id),
        elements,
    })
}

fn take_compound_list(tag: &mut Tag, key: &str) -> Vec<Tag> {
    match nbt::take(tag, key) {
        Some(Tag::List(list)) => list.elements,
        _ => Vec::new(),
    }
}

fn int_values(tag: &Tag) -> Option<Vec<i32>> {
    match tag {
        Tag::IntArray(values) => Some(values.clone()),
        Tag::List(list) => list.elements.iter().map(nbt::int).collect(),
        _ => None,
    }
}

fn long_values(tag: &Tag) -> Vec<i64> {
    match tag {
        Tag::LongArray(values) => values.clone(),
        Tag::List(list) => list
            .elements
            .iter()
            .map(|value| match value {
                Tag::Long(value) => *value,
                Tag::Int(value) => i64::from(*value),
                Tag::Short(value) => i64::from(*value),
                Tag::Byte(value) => i64::from(*value),
                _ => 0,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn moved_longs(tag: Tag) -> Vec<i64> {
    match tag {
        Tag::LongArray(values) => values,
        Tag::List(list) => list
            .elements
            .iter()
            .map(|value| match value {
                Tag::Long(value) => *value,
                Tag::Int(value) => i64::from(*value),
                Tag::Short(value) => i64::from(*value),
                Tag::Byte(value) => i64::from(*value),
                _ => 0,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn section_y(section: &Tag) -> Option<i32> {
    nbt::get(section, "Y").and_then(nbt::int)
}

fn context_value<'a>(tag: &'a Tag, key: &str) -> &'a str {
    nbt::get(tag, "__context")
        .and_then(|context| nbt::get(context, key))
        .and_then(nbt::string)
        .unwrap_or("")
}
// -----------------------------------------------------------------------
// 2527 BitStorageAlignFix
// -----------------------------------------------------------------------

/// Convert a 1.15 straddling packed array to aligned per-long storage.
/// Values are read continuously (`index * bits` bit offset, spanning longs
/// when the entry crosses a word boundary) and rewritten so no value spans
/// two longs (`(index % per_long) * bits`). Reads past the stored words
/// decode as 0, exactly like vanilla's bounds-checked fetches. Empty input
/// is returned empty (`addPadding` returns the input when length is 0).
fn align_storage(size: usize, bits: u32, data: &[i64]) -> Vec<i64> {
    if data.is_empty() {
        return Vec::new();
    }
    let bits = bits as usize;
    let mask = (1_u64 << bits) - 1;
    let per_long = 64 / bits;
    let mut out = vec![0_i64; size.div_ceil(per_long)];
    for index in 0..size {
        let start = index * bits;
        let word = start / 64;
        let offset = start % 64;
        let low = data.get(word).copied().unwrap_or(0) as u64;
        let mut value = low >> offset;
        if offset + bits > 64 {
            let high = data.get(word + 1).copied().unwrap_or(0) as u64;
            value |= high << (64 - offset);
        }
        value &= mask;
        let slot = index / per_long;
        let at = (index % per_long) * bits;
        out[slot] |= (value as i64) << at;
    }
    out
}

/// 2527 `BitStorageAlignFix`: realign section `BlockStates` (4096 entries,
/// `max(4, ceillog2(palette))` bits) and every `Heightmaps` entry (256
/// entries, 9 bits). Power-of-two widths need no work; missing keys are
/// no-ops (`Dynamic.update`). Empty arrays stay empty.
fn align_bit_storage(root: &mut Tag) -> bool {
    let mut changed = false;
    if let Some(sections) =
        nbt::get_mut(chunk_level_mut(&mut *root), "Sections").and_then(nbt::list_mut)
    {
        for section in sections.iter_mut() {
            let bits = nbt::get(section, "Palette")
                .and_then(nbt::list)
                .map_or(4, |palette| ceil_log2(palette.len()).max(4));
            if bits.is_power_of_two() {
                continue;
            }
            let Some(old) = nbt::get(section, "BlockStates").map(long_values) else {
                continue;
            };
            if old.is_empty() {
                continue;
            }
            let aligned = align_storage(4096, bits, &old);
            if aligned != old {
                nbt::set(section, "BlockStates", Tag::LongArray(aligned));
                changed = true;
            }
        }
    }
    if let Some(maps) =
        nbt::get_mut(chunk_level_mut(&mut *root), "Heightmaps").and_then(nbt::fields_mut)
    {
        for (_, value) in maps.iter_mut() {
            let old = long_values(value);
            if old.is_empty() {
                continue;
            }
            let aligned = align_storage(256, 9, &old);
            if aligned != old {
                *value = Tag::LongArray(aligned);
                changed = true;
            }
        }
    }
    changed
}

const PLAINS: &str = "minecraft:plains";
const AIR: &str = "minecraft:air";

/// Old numeric biome id to name, transcribed from
/// `chunks-ChunkHeightAndBiomeFix.javap` (`BIOMES_BY_ID` static block).
/// Unknown ids fall back to `minecraft:plains` (`getOrDefault`).
fn biome_by_id(id: i32) -> &'static str {
    match id {
        0 => "minecraft:ocean",
        1 => "minecraft:plains",
        2 => "minecraft:desert",
        3 => "minecraft:mountains",
        4 => "minecraft:forest",
        5 => "minecraft:taiga",
        6 => "minecraft:swamp",
        7 => "minecraft:river",
        8 => "minecraft:nether_wastes",
        9 => "minecraft:the_end",
        10 => "minecraft:frozen_ocean",
        11 => "minecraft:frozen_river",
        12 => "minecraft:snowy_tundra",
        13 => "minecraft:snowy_mountains",
        14 => "minecraft:mushroom_fields",
        15 => "minecraft:mushroom_field_shore",
        16 => "minecraft:beach",
        17 => "minecraft:desert_hills",
        18 => "minecraft:wooded_hills",
        19 => "minecraft:taiga_hills",
        20 => "minecraft:mountain_edge",
        21 => "minecraft:jungle",
        22 => "minecraft:jungle_hills",
        23 => "minecraft:jungle_edge",
        24 => "minecraft:deep_ocean",
        25 => "minecraft:stone_shore",
        26 => "minecraft:snowy_beach",
        27 => "minecraft:birch_forest",
        28 => "minecraft:birch_forest_hills",
        29 => "minecraft:dark_forest",
        30 => "minecraft:snowy_taiga",
        31 => "minecraft:snowy_taiga_hills",
        32 => "minecraft:giant_tree_taiga",
        33 => "minecraft:giant_tree_taiga_hills",
        34 => "minecraft:wooded_mountains",
        35 => "minecraft:savanna",
        36 => "minecraft:savanna_plateau",
        37 => "minecraft:badlands",
        38 => "minecraft:wooded_badlands_plateau",
        39 => "minecraft:badlands_plateau",
        40 => "minecraft:small_end_islands",
        41 => "minecraft:end_midlands",
        42 => "minecraft:end_highlands",
        43 => "minecraft:end_barrens",
        44 => "minecraft:warm_ocean",
        45 => "minecraft:lukewarm_ocean",
        46 => "minecraft:cold_ocean",
        47 => "minecraft:deep_warm_ocean",
        48 => "minecraft:deep_lukewarm_ocean",
        49 => "minecraft:deep_cold_ocean",
        50 => "minecraft:deep_frozen_ocean",
        127 => "minecraft:the_void",
        129 => "minecraft:sunflower_plains",
        130 => "minecraft:desert_lakes",
        131 => "minecraft:gravelly_mountains",
        132 => "minecraft:flower_forest",
        133 => "minecraft:taiga_mountains",
        134 => "minecraft:swamp_hills",
        140 => "minecraft:ice_spikes",
        149 => "minecraft:modified_jungle",
        151 => "minecraft:modified_jungle_edge",
        155 => "minecraft:tall_birch_forest",
        156 => "minecraft:tall_birch_hills",
        157 => "minecraft:dark_forest_hills",
        158 => "minecraft:snowy_taiga_mountains",
        160 => "minecraft:giant_spruce_taiga",
        161 => "minecraft:giant_spruce_taiga_hills",
        162 => "minecraft:modified_gravelly_mountains",
        163 => "minecraft:shattered_savanna",
        164 => "minecraft:shattered_savanna_plateau",
        165 => "minecraft:eroded_badlands",
        166 => "minecraft:modified_wooded_badlands_plateau",
        167 => "minecraft:modified_badlands_plateau",
        168 => "minecraft:bamboo_jungle",
        169 => "minecraft:bamboo_jungle_hills",
        170 => "minecraft:soul_sand_valley",
        171 => "minecraft:crimson_forest",
        172 => "minecraft:warped_forest",
        173 => "minecraft:basalt_deltas",
        174 => "minecraft:dripstone_caves",
        175 => "minecraft:lush_caves",
        177 => "minecraft:meadow",
        178 => "minecraft:grove",
        179 => "minecraft:snowy_slopes",
        180 => "minecraft:snowcapped_peaks",
        181 => "minecraft:lofty_peaks",
        182 => "minecraft:stony_peaks",
        _ => PLAINS,
    }
}

/// Integer `ceillog2` matching vanilla's `ceil(log(n)/log(2))`: 0 for 0 and 1.
fn ceil_log2(value: usize) -> u32 {
    if value <= 1 {
        return 0;
    }
    usize::BITS - (value - 1).leading_zeros()
}

fn air_block_state() -> Tag {
    nbt::compound(vec![("Name", text(AIR))])
}

fn single_block_container() -> Tag {
    nbt::compound(vec![("palette", compound_list(vec![air_block_state()]))])
}

fn single_biome_container() -> Tag {
    nbt::compound(vec![("palette", compound_list(vec![text(PLAINS)]))])
}

fn empty_containers(count: usize) -> Vec<Tag> {
    (0..count).map(|_| single_biome_container()).collect()
}

fn pack_indices(palette_size: usize, values: &[usize]) -> Vec<i64> {
    let bits = ceil_log2(palette_size).max(1) as usize;
    let per_long = 64 / bits;
    let longs = values.len().div_ceil(per_long);
    let mut data = vec![0_i64; longs];
    for (index, value) in values.iter().enumerate() {
        let slot = index / per_long;
        let offset = (index % per_long) * bits;
        data[slot] |= (*value as i64) << offset;
    }
    data
}

/// `{palette, data?}` from 64 biome ids in first-seen order.
fn biome_container(ids: &[i32]) -> Tag {
    debug_assert_eq!(ids.len(), 64);
    let mut palette: Vec<i32> = Vec::new();
    let mut indices = [0_usize; 64];
    for (slot, id) in ids.iter().enumerate() {
        let position = match palette.iter().position(|known| known == id) {
            Some(position) => position,
            None => {
                palette.push(*id);
                palette.len() - 1
            }
        };
        indices[slot] = position;
    }
    let names: Vec<Tag> = palette.iter().map(|id| text(biome_by_id(*id))).collect();
    if palette.len() <= 1 {
        return nbt::compound(vec![("palette", compound_list(names))]);
    }
    let data = pack_indices(palette.len(), &indices[..ids.len()]);
    nbt::compound(vec![
        ("palette", compound_list(names)),
        ("data", Tag::LongArray(data)),
    ])
}

/// 24 (overworld) or 16 biome containers from the legacy `Biomes` int array.
/// `has_biomes` mirrors vanilla's `MutableBoolean`: true only for the already
/// 24-layer 1536-entry shape.
fn biome_containers(old: Option<Vec<i32>>, overworld: bool, min_y: i32) -> (Vec<Tag>, bool) {
    let Some(values) = old else {
        return (empty_containers(if overworld { 24 } else { 16 }), false);
    };
    if values.len() == 1536 {
        let containers = (0..24)
            .map(|section| {
                let mut ids = [0_i32; 64];
                for (slot, id) in ids.iter_mut().enumerate() {
                    *id = values[section * 64 + slot] & 255;
                }
                biome_container(&ids)
            })
            .collect();
        return (containers, true);
    }
    if values.len() == 1024 {
        let mut containers = empty_containers(if overworld { 24 } else { 16 });
        for section in 0..16_i32 {
            let mut ids = [0_i32; 64];
            for (slot, id) in ids.iter_mut().enumerate() {
                *id = values[section as usize * 64 + slot] & 255;
            }
            let target = (section - min_y) as usize;
            if target < containers.len() {
                containers[target] = biome_container(&ids);
            }
        }
        if overworld {
            let mut bottom = [0_i32; 64];
            for (slot, id) in bottom.iter_mut().enumerate() {
                *id = values[slot % 16] & 255;
            }
            let bottom = biome_container(&bottom);
            let mut top = [0_i32; 64];
            for (slot, id) in top.iter_mut().enumerate() {
                *id = values[1008 + slot % 16] & 255;
            }
            let top = biome_container(&top);
            for container in containers.iter_mut().take(4) {
                *container = bottom.clone();
            }
            for container in containers.iter_mut().skip(20) {
                *container = top.clone();
            }
        }
        return (containers, false);
    }
    (empty_containers(if overworld { 24 } else { 16 }), false)
}

/// Optimized block container from an old palette and `BlockStates` longs.
/// Single-entry palettes drop the data array; otherwise the palette is padded
/// with air when the stored bits-per-block exceed `ceillog2(palette)`.
fn optimized_block_container(palette: Vec<Tag>, longs: Vec<i64>) -> Tag {
    if palette.len() == 1 {
        return nbt::compound(vec![("palette", compound_list(palette))]);
    }
    let stored_bits = longs.len() as u64 * 64 / 4096;
    if stored_bits > ceil_log2(palette.len()) as u64 {
        let target = (1_usize << (stored_bits - 1)) + 1;
        let mut padded = palette;
        while padded.len() < target {
            padded.push(air_block_state());
        }
        return nbt::compound(vec![
            ("palette", compound_list(padded)),
            ("data", Tag::LongArray(longs)),
        ]);
    }
    nbt::compound(vec![
        ("palette", compound_list(palette)),
        ("data", Tag::LongArray(longs)),
    ])
}

/// Minimal packed-array reader for the y=0 bedrock scan. `bits` matches
/// vanilla (`max(4, ceillog2(palette))`); out-of-range reads are null.
struct PoorMansContainer {
    palette: Vec<Tag>,
    data: Vec<i64>,
}

impl PoorMansContainer {
    fn get(&self, x: usize, y: usize, z: usize) -> Option<&Tag> {
        if self.palette.is_empty() {
            return None;
        }
        if self.palette.len() == 1 {
            return self.palette.first();
        }
        let bits = ceil_log2(self.palette.len()).max(4) as usize;
        let per_long = 64 / bits;
        let mask = (1_i64 << bits) - 1;
        let index = (y * 16 + z) * 16 + x;
        let slot = index / per_long;
        let entry = *self.data.get(slot)?;
        let offset = (index % per_long) * bits;
        let palette_index = ((entry >> offset) & mask) as usize;
        self.palette.get(palette_index)
    }
}

fn block_name(entry: Option<&Tag>) -> &str {
    entry
        .and_then(|entry| nbt::get(entry, "Name"))
        .and_then(nbt::string)
        .unwrap_or("")
}

/// Chunk statuses at or after surface generation (bare names, vanilla
/// `STATUS_IS_OR_AFTER_SURFACE`); noise adds `noise`.
fn at_or_after_surface(status: &str) -> bool {
    matches!(
        status,
        "surface"
            | "carvers"
            | "liquid_carvers"
            | "features"
            | "light"
            | "spawn"
            | "heightmaps"
            | "full"
    )
}

fn at_or_after_noise(status: &str) -> bool {
    status == "noise" || at_or_after_surface(status)
}

/// Blocks that do not prove pre-feature generation (vanilla
/// `BLOCKS_BEFORE_FEATURE_STATUS`, `minecraft:`-prefixed).
fn before_feature_block(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air"
            | "minecraft:basalt"
            | "minecraft:bedrock"
            | "minecraft:blackstone"
            | "minecraft:calcite"
            | "minecraft:cave_air"
            | "minecraft:coarse_dirt"
            | "minecraft:crimson_nylium"
            | "minecraft:dirt"
            | "minecraft:end_stone"
            | "minecraft:grass_block"
            | "minecraft:gravel"
            | "minecraft:ice"
            | "minecraft:lava"
            | "minecraft:mycelium"
            | "minecraft:nether_wart_block"
            | "minecraft:netherrack"
            | "minecraft:orange_terracotta"
            | "minecraft:packed_ice"
            | "minecraft:podzol"
            | "minecraft:powder_snow"
            | "minecraft:red_sand"
            | "minecraft:red_sandstone"
            | "minecraft:sand"
            | "minecraft:sandstone"
            | "minecraft:snow_block"
            | "minecraft:soul_sand"
            | "minecraft:soul_soil"
            | "minecraft:stone"
            | "minecraft:terracotta"
            | "minecraft:warped_nylium"
            | "minecraft:warped_wart_block"
            | "minecraft:water"
            | "minecraft:white_terracotta"
    )
}

/// Status prediction for overworld chunks without 24-layer biomes
/// (`predictChunkStatusBeforeSurface`).
fn predict_status(
    status: &str,
    blocks: &mut std::collections::HashSet<String>,
) -> Option<&'static str> {
    if at_or_after_surface(status) {
        return None;
    }
    blocks.remove(AIR);
    let non_air = !blocks.is_empty();
    blocks.retain(|name| !before_feature_block(name));
    if !blocks.is_empty() {
        return Some("liquid_carvers");
    }
    if status == "noise" || non_air {
        return Some("noise");
    }
    if status == "biomes" {
        return Some("structure_references");
    }
    None
}

const HEIGHTMAP_TYPES: [&str; 7] = [
    "WORLD_SURFACE_WG",
    "WORLD_SURFACE",
    "WORLD_SURFACE_IGNORE_SNOW",
    "OCEAN_FLOOR_WG",
    "OCEAN_FLOOR",
    "MOTION_BLOCKING",
    "MOTION_BLOCKING_NO_LEAVES",
];

/// Height values shift +64 capped at 511; zeros stay 0
/// (`lambda$getFixedHeightmap$0`: 9-bit fields over each long).
fn fix_heightmap_value(value: i64) -> i64 {
    let mut out = 0_i64;
    let mut shift = 0;
    while shift + 9 <= 64 {
        let field = (value >> shift) & 511;
        let fixed = if field == 0 { 0 } else { (field + 64).min(511) };
        out |= fixed << shift;
        shift += 9;
    }
    out
}

fn fix_heightmaps(level: &mut Tag) {
    let Some(heightmaps) = nbt::get_mut(level, "Heightmaps").and_then(nbt::fields_mut) else {
        return;
    };
    for (name, value) in heightmaps {
        if !HEIGHTMAP_TYPES.contains(&name.as_str()) {
            continue;
        }
        if let Tag::LongArray(longs) = value {
            for entry in longs.iter_mut() {
                *entry = fix_heightmap_value(*entry);
            }
        }
    }
}

/// Resize carving masks (`updateCarvingMasks`): old bytes are a little-endian
/// bitset; the new long array has `64 * sections` entries with the old words
/// copied at offset `64 * shift`.
fn update_carving_masks(level: &mut Tag, sections: usize, shift: usize) {
    let old = nbt::get(level, "CarvingMasks")
        .and_then(nbt::fields)
        .map(|fields| {
            fields
                .iter()
                .map(|(name, value)| {
                    let bytes = match value {
                        Tag::ByteArray(bytes) => bytes.clone(),
                        _ => Vec::new(),
                    };
                    (name.clone(), bytes)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut masks = Vec::with_capacity(old.len());
    for (name, bytes) in old {
        let words: Vec<i64> = bytes
            .chunks(8)
            .map(|chunk| {
                let mut word = [0_u8; 8];
                for (slot, byte) in word.iter_mut().zip(chunk.iter()) {
                    *slot = *byte as u8;
                }
                i64::from_le_bytes(word)
            })
            .collect();
        let mut longs = vec![0_i64; 64 * sections];
        let offset = 64 * shift;
        let room = longs.len().saturating_sub(offset);
        let count = words.len().min(room);
        longs[offset..offset + count].copy_from_slice(&words[..count]);
        masks.push((name, Tag::LongArray(longs)));
    }
    nbt::set(level, "CarvingMasks", Tag::Compound(masks));
}

/// Pad 16-entry tick/post-processing lists to 24 with 4 empty lists on each
/// side (`addPaddingEntries`); 24-entry lists are untouched. Missing keys are
/// created with 8 empty lists, exactly like vanilla (`orElseEmptyList`).
fn pad_tick_padding(level: &mut Tag) {
    for key in ["LiquidsToBeTicked", "PostProcessing", "ToBeTicked"] {
        let current = nbt::get(level, key)
            .and_then(nbt::list)
            .map_or(Vec::new(), <[Tag]>::to_vec);
        if current.len() == 24 {
            continue;
        }
        let mut padded = Vec::with_capacity(current.len() + 8);
        for _ in 0..4 {
            padded.push(Tag::List(ListTag::empty()));
        }
        padded.extend(current);
        for _ in 0..4 {
            padded.push(Tag::List(ListTag::empty()));
        }
        nbt::set(level, key, compound_list(padded));
    }
}

/// Shift `UpgradeData.Indices` keys by +4, dropping non-numeric keys
/// (`shiftUpgradeData`; `NumberFormatException` entries are skipped).
fn shift_upgrade_data(level: &mut Tag) {
    let Some(upgrade) = nbt::get_mut(level, "UpgradeData") else {
        return;
    };
    let Some(indices) = nbt::take(upgrade, "Indices") else {
        return;
    };
    let Tag::Compound(fields) = indices else {
        nbt::set(upgrade, "Indices", Tag::Compound(Vec::new()));
        return;
    };
    let mut shifted = Vec::with_capacity(fields.len());
    for (key, value) in fields {
        let Ok(number) = key.parse::<i32>() else {
            continue;
        };
        shifted.push((number.wrapping_add(4).to_string(), value));
    }
    nbt::set(upgrade, "Indices", Tag::Compound(shifted));
}

fn bitset_longs(words: &[u64]) -> Vec<i64> {
    let mut end = words.len();
    while end > 0 && words[end - 1] == 0 {
        end -= 1;
    }
    words[..end].iter().map(|word| *word as i64).collect()
}

#[allow(clippy::too_many_lines)]
fn height_and_biome(root: &mut Tag) {
    let dimension = context_value(&*root, "dimension").to_owned();
    let generator = context_value(&*root, "generator").to_owned();
    let overworld = dimension == "minecraft:overworld";
    let min_y = if overworld { -4 } else { 0 };
    let is_noise = generator == "minecraft:noise";

    let old_biomes = nbt::get(chunk_level_mut(&mut *root), "Biomes").and_then(int_values);
    let (containers, has_biomes) = biome_containers(old_biomes, overworld, min_y);

    // Section conversion first: block names feed status prediction and the
    // y=0 palette feeds below-zero retrogen. Sections are moved out and
    // rebuilt; only the y=0 bedrock scan keeps one extra palette copy.
    let mut blocks: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let mut y0: Option<PoorMansContainer> = None;
    let mut converted: Vec<Tag> = Vec::new();
    let key = if nbt::get(chunk_level_mut(&mut *root), "Sections").is_some() {
        "Sections"
    } else {
        "sections"
    };
    let sections = take_compound_list(chunk_level_mut(&mut *root), key);
    for mut section in sections {
        if nbt::fields(&section).is_none() {
            converted.push(section);
            continue;
        }
        let y = section_y(&section).unwrap_or(0);
        let palette: Option<Vec<Tag>> =
            nbt::take(&mut section, "Palette").and_then(|tag| match tag {
                Tag::List(list) => Some(list.elements),
                _ => None,
            });
        let longs: Vec<i64> = nbt::take(&mut section, "BlockStates")
            .map(moved_longs)
            .unwrap_or_default();
        if let Some(entries) = &palette {
            for entry in entries {
                blocks.insert(block_name(Some(entry)).to_owned());
            }
        }
        if y == 0
            && let Some(entries) = &palette
        {
            y0 = Some(PoorMansContainer {
                palette: entries.clone(),
                data: longs.clone(),
            });
        }
        let states = match palette {
            Some(entries) => optimized_block_container(entries, longs),
            None => single_block_container(),
        };
        nbt::set(&mut section, "block_states", states);
        let index = y - min_y;
        if index >= 0 && (index as usize) < containers.len() {
            nbt::set(&mut section, "biomes", containers[index as usize].clone());
        }
        seen.insert(y);
        converted.push(section);
    }
    for (index, container) in containers.iter().enumerate() {
        let y = index as i32 + min_y;
        if seen.insert(y) {
            converted.push(nbt::compound(vec![
                ("Y", Tag::Int(y)),
                ("block_states", single_block_container()),
                ("biomes", container.clone()),
            ]));
        }
    }
    nbt::set(chunk_level_mut(&mut *root), key, compound_list(converted));

    if overworld {
        let status = nbt::get(chunk_level_mut(&mut *root), "Status")
            .and_then(nbt::string)
            .unwrap_or("empty")
            .to_owned();
        if let Some(predicted) = predict_status(&status, &mut blocks) {
            nbt::set(chunk_level_mut(&mut *root), "Status", text(predicted));
        }
    }

    nbt::remove(chunk_level_mut(&mut *root), "Biomes");
    if !overworld {
        update_carving_masks(chunk_level_mut(&mut *root), 16, 0);
        return;
    }
    if has_biomes {
        update_carving_masks(chunk_level_mut(&mut *root), 24, 0);
        return;
    }
    fix_heightmaps(chunk_level_mut(&mut *root));
    pad_tick_padding(chunk_level_mut(&mut *root));
    update_carving_masks(chunk_level_mut(&mut *root), 24, 4);
    shift_upgrade_data(chunk_level_mut(&mut *root));

    if !is_noise {
        return;
    }
    let status = nbt::get(chunk_level_mut(&mut *root), "Status")
        .and_then(nbt::string)
        .unwrap_or("")
        .to_owned();
    if status.is_empty() || status == "empty" {
        return;
    }
    let old_noise = at_or_after_noise(&status);
    nbt::set(
        chunk_level_mut(&mut *root),
        "blending_data",
        nbt::compound(vec![("old_noise", Tag::Byte(i8::from(old_noise)))]),
    );
    let Some(container) = y0 else { return };
    let mut missing = [0_u64; 4];
    let mut bedrock_or_noise = status == "noise";
    for x in 0..16 {
        for z in 0..16 {
            let entry = container.get(x, 0, z);
            if block_name(entry) == "minecraft:bedrock" {
                bedrock_or_noise = true;
            }
            if block_name(entry) == AIR {
                missing[(x + z * 16) / 64] |= 1 << ((x + z * 16) % 64);
            }
        }
    }
    let air_columns: u32 = missing.iter().map(|word| word.count_ones()).sum();
    if !bedrock_or_noise || air_columns == 256 {
        return;
    }
    let target = if status == "full" {
        "heightmaps".to_owned()
    } else {
        status
    };
    nbt::set(
        chunk_level_mut(&mut *root),
        "below_zero_retrogen",
        nbt::compound(vec![
            ("target_status", text(&target)),
            ("missing_bedrock", Tag::LongArray(bitset_longs(&missing))),
        ]),
    );
    nbt::set(chunk_level_mut(&mut *root), "Status", text("empty"));
    nbt::set(chunk_level_mut(&mut *root), "isLightOn", Tag::Byte(0));
}

// -----------------------------------------------------------------------
// 2838 / 2843 biome renames
// -----------------------------------------------------------------------

/// Transcribed from `chunks-CavesAndCliffsRenames.javap` (`RENAMES`).
const CAVES_AND_CLIFFS_RENAMES: [(&str, &str); 38] = [
    ("minecraft:badlands_plateau", "minecraft:badlands"),
    ("minecraft:bamboo_jungle_hills", "minecraft:bamboo_jungle"),
    ("minecraft:birch_forest_hills", "minecraft:birch_forest"),
    ("minecraft:dark_forest_hills", "minecraft:dark_forest"),
    ("minecraft:desert_hills", "minecraft:desert"),
    ("minecraft:desert_lakes", "minecraft:desert"),
    (
        "minecraft:giant_spruce_taiga_hills",
        "minecraft:old_growth_spruce_taiga",
    ),
    (
        "minecraft:giant_spruce_taiga",
        "minecraft:old_growth_spruce_taiga",
    ),
    (
        "minecraft:giant_tree_taiga_hills",
        "minecraft:old_growth_pine_taiga",
    ),
    (
        "minecraft:giant_tree_taiga",
        "minecraft:old_growth_pine_taiga",
    ),
    (
        "minecraft:gravelly_mountains",
        "minecraft:windswept_gravelly_hills",
    ),
    ("minecraft:jungle_edge", "minecraft:sparse_jungle"),
    ("minecraft:jungle_hills", "minecraft:jungle"),
    ("minecraft:modified_badlands_plateau", "minecraft:badlands"),
    (
        "minecraft:modified_gravelly_mountains",
        "minecraft:windswept_gravelly_hills",
    ),
    ("minecraft:modified_jungle_edge", "minecraft:sparse_jungle"),
    ("minecraft:modified_jungle", "minecraft:jungle"),
    (
        "minecraft:modified_wooded_badlands_plateau",
        "minecraft:wooded_badlands",
    ),
    ("minecraft:mountain_edge", "minecraft:windswept_hills"),
    ("minecraft:mountains", "minecraft:windswept_hills"),
    (
        "minecraft:mushroom_field_shore",
        "minecraft:mushroom_fields",
    ),
    ("minecraft:shattered_savanna", "minecraft:windswept_savanna"),
    (
        "minecraft:shattered_savanna_plateau",
        "minecraft:windswept_savanna",
    ),
    ("minecraft:snowy_mountains", "minecraft:snowy_plains"),
    ("minecraft:snowy_taiga_hills", "minecraft:snowy_taiga"),
    ("minecraft:snowy_taiga_mountains", "minecraft:snowy_taiga"),
    ("minecraft:snowy_tundra", "minecraft:snowy_plains"),
    ("minecraft:stone_shore", "minecraft:stony_shore"),
    ("minecraft:swamp_hills", "minecraft:swamp"),
    ("minecraft:taiga_hills", "minecraft:taiga"),
    ("minecraft:taiga_mountains", "minecraft:taiga"),
    (
        "minecraft:tall_birch_forest",
        "minecraft:old_growth_birch_forest",
    ),
    (
        "minecraft:tall_birch_hills",
        "minecraft:old_growth_birch_forest",
    ),
    (
        "minecraft:wooded_badlands_plateau",
        "minecraft:wooded_badlands",
    ),
    ("minecraft:wooded_hills", "minecraft:forest"),
    ("minecraft:wooded_mountains", "minecraft:windswept_forest"),
    ("minecraft:lofty_peaks", "minecraft:jagged_peaks"),
    ("minecraft:snowcapped_peaks", "minecraft:frozen_peaks"),
];

const DEEP_WARM_OCEAN_RENAME: [(&str, &str); 1] =
    [("minecraft:deep_warm_ocean", "minecraft:warm_ocean")];

fn rename_biome_value(value: &mut Tag, table: &[(&str, &str)]) -> bool {
    let Some(name) = nbt::string(value) else {
        return false;
    };
    for (old, new) in table {
        if name == *old {
            *value = text(new);
            return true;
        }
    }
    false
}

/// Rename string biome palette entries in every section's `biomes`
/// container. Vanilla's `NamespacedTypeRenameFix` runs over the string
/// `BIOME` schema, so compound palette values are opaque and left alone.
fn rename_biomes(sections: Option<&mut Vec<Tag>>, table: &[(&str, &str)]) -> bool {
    let Some(sections) = sections else {
        return false;
    };
    let mut changed = false;
    for section in sections {
        let Some(palette) = nbt::get_mut(section, "biomes")
            .and_then(|biomes| nbt::get_mut(biomes, "palette"))
            .and_then(nbt::list_mut)
        else {
            continue;
        };
        for entry in palette.iter_mut() {
            if rename_biome_value(entry, table) {
                changed = true;
            }
        }
    }
    changed
}

// -----------------------------------------------------------------------
// 2841 ChunkProtoTickListFix
// -----------------------------------------------------------------------

/// Blocks that always imply water (`ALWAYS_WATERLOGGED`).
fn always_waterlogged(name: &str) -> bool {
    matches!(
        name,
        "minecraft:bubble_column"
            | "minecraft:kelp"
            | "minecraft:kelp_plant"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
    )
}

fn properties(entry: &Tag) -> Option<&Tag> {
    nbt::get(entry, "Properties")
}

/// Vanilla `OptionalDynamic.asInt(0)`: non-numeric tags (including the
/// string `"0"` vanilla stores in `Properties`) fall back to 0. Do not
/// "repair" this into string parsing.
fn property_level(entry: &Tag) -> i32 {
    properties(entry)
        .and_then(|properties| nbt::get(properties, "level"))
        .and_then(nbt::int)
        .unwrap_or(0)
}

/// Vanilla `OptionalDynamic.asBoolean(false)`: only a nonzero byte counts.
/// String `"true"` stays false, exactly like vanilla.
fn property_waterlogged(entry: &Tag) -> bool {
    properties(entry)
        .and_then(|properties| nbt::get(properties, "waterlogged"))
        .and_then(|value| match value {
            Tag::Byte(value) => Some(*value != 0),
            _ => None,
        })
        .unwrap_or(false)
}

fn tick_block_name(entry: Option<&Tag>) -> String {
    entry
        .and_then(|entry| nbt::get(entry, "Name"))
        .and_then(nbt::string)
        .unwrap_or(AIR)
        .to_owned()
}

fn tick_liquid_name(entry: Option<&Tag>) -> String {
    let Some(entry) = entry else {
        return "minecraft:empty".to_owned();
    };
    let name = nbt::get(entry, "Name").and_then(nbt::string).unwrap_or("");
    if name == "minecraft:water" {
        if property_level(entry) == 0 {
            return "minecraft:water".to_owned();
        }
        return "minecraft:flowing_water".to_owned();
    }
    if name == "minecraft:lava" {
        if property_level(entry) == 0 {
            return "minecraft:lava".to_owned();
        }
        return "minecraft:flowing_lava".to_owned();
    }
    if always_waterlogged(name) || property_waterlogged(entry) {
        return "minecraft:water".to_owned();
    }
    "minecraft:empty".to_owned()
}

/// One scheduled-tick entry: packed shorts hold x/y/z nibbles (`v & 15`,
/// `(v >> 4) & 15`, `(v >> 8) & 15`); the block name resolves through the
/// section's packed palette at the nibble position.
fn create_tick(x_pos: i32, section_y: i32, z_pos: i32, packed: i32, name: String) -> Tag {
    let x = packed & 15;
    let y = (packed >> 4) & 15;
    let z = (packed >> 8) & 15;
    nbt::compound(vec![
        ("i", text(&name)),
        ("x", Tag::Int(x_pos * 16 + x)),
        ("y", Tag::Int(section_y * 16 + y)),
        ("z", Tag::Int(z_pos * 16 + z)),
        ("t", Tag::Int(0)),
        ("p", Tag::Int(0)),
    ])
}

fn tick_list(
    level: &Tag,
    containers: &std::collections::HashMap<i32, PoorMansContainer>,
    min_y: i32,
    x_pos: i32,
    z_pos: i32,
    key: &str,
    liquid: bool,
) -> Vec<Tag> {
    let Some(source) = nbt::get(level, key).and_then(nbt::list) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, section_ticks) in source.iter().enumerate() {
        let section_y = index as i32 + min_y;
        let container = containers.get(&section_y);
        let Some(ticks) = nbt::list(section_ticks) else {
            continue;
        };
        for packed in ticks {
            // `asShort(-1)` then `> 0`: unparseable entries are skipped.
            let packed = nbt::int(packed).unwrap_or(-1);
            if packed <= 0 {
                continue;
            }
            let x = (packed & 15) as usize;
            let y = ((packed >> 4) & 15) as usize;
            let z = ((packed >> 8) & 15) as usize;
            let entry = container.and_then(|container| container.get(x, y, z));
            let name = if liquid {
                tick_liquid_name(entry)
            } else {
                tick_block_name(entry)
            };
            out.push(create_tick(x_pos, section_y, z_pos, packed, name));
        }
    }
    out
}

/// Minimum section Y over sections carrying `biomes` (vanilla's
/// `getOptionalTyped(biomes)` gate), starting from 0.
fn proto_min_y(level: &Tag) -> i32 {
    let sections: &[Tag] = nbt::get(level, "Sections")
        .or_else(|| nbt::get(level, "sections"))
        .and_then(nbt::list)
        .unwrap_or(&[]);
    let mut min_y = 0_i32;
    for section in sections {
        if nbt::get(section, "biomes").is_none() {
            continue;
        }
        if let Some(y) = section_y(section) {
            min_y = min_y.min(y);
        }
    }
    min_y
}

/// Packed palette readers keyed by section Y, from `block_states`
/// independently of the `biomes` gate.
fn proto_containers(level: &Tag) -> std::collections::HashMap<i32, PoorMansContainer> {
    let sections: &[Tag] = nbt::get(level, "Sections")
        .or_else(|| nbt::get(level, "sections"))
        .and_then(nbt::list)
        .unwrap_or(&[]);
    let mut containers = std::collections::HashMap::new();
    for section in sections {
        let Some(y) = section_y(section) else {
            continue;
        };
        let Some(states) = nbt::get(section, "block_states") else {
            continue;
        };
        let palette = nbt::get(states, "palette")
            .and_then(nbt::list)
            .map_or(Vec::new(), <[Tag]>::to_vec);
        let data = nbt::get(states, "data")
            .map(long_values)
            .unwrap_or_default();
        containers.insert(y, PoorMansContainer { palette, data });
    }
    containers
}

/// Build `TileTicks`/`fluid_ticks` from `ToBeTicked`/`LiquidsToBeTicked`.
/// Order matches vanilla: `LiquidTicks` moves to `fluid_ticks` verbatim
/// first; a present `TileTicks` list or `fluid_ticks` then keeps the existing
/// lists. `yPos` is `Dynamic.update`, so an absent key stays absent.
/// Returns whether anything changed.
fn proto_tick_list(root: &mut Tag) -> bool {
    let mut changed = false;
    let min_y = proto_min_y(chunk_level_mut(&mut *root));
    {
        let present = nbt::get(chunk_level_mut(&mut *root), "yPos").is_some();
        if present {
            let stamped = Tag::Byte(min_y as i8);
            if nbt::get(chunk_level_mut(&mut *root), "yPos") != Some(&stamped) {
                nbt::set(chunk_level_mut(&mut *root), "yPos", stamped);
                changed = true;
            }
        }
    }
    {
        let liquid = nbt::take(chunk_level_mut(&mut *root), "LiquidTicks");
        if let Some(liquid) = liquid {
            nbt::set(chunk_level_mut(&mut *root), "fluid_ticks", liquid);
            changed = true;
        }
    }
    {
        let has_tile = nbt::get(chunk_level_mut(&mut *root), "TileTicks")
            .and_then(nbt::list)
            .is_some();
        if has_tile || nbt::get(chunk_level_mut(&mut *root), "fluid_ticks").is_some() {
            return changed;
        }
    }
    let x_pos = nbt::get(chunk_level_mut(&mut *root), "xPos")
        .and_then(nbt::int)
        .unwrap_or(0);
    let z_pos = nbt::get(chunk_level_mut(&mut *root), "zPos")
        .and_then(nbt::int)
        .unwrap_or(0);
    let containers = proto_containers(chunk_level_mut(&mut *root));
    let blocks = tick_list(
        chunk_level_mut(&mut *root),
        &containers,
        min_y,
        x_pos,
        z_pos,
        "ToBeTicked",
        false,
    );
    let fluids = tick_list(
        chunk_level_mut(&mut *root),
        &containers,
        min_y,
        x_pos,
        z_pos,
        "LiquidsToBeTicked",
        true,
    );
    nbt::set(
        chunk_level_mut(&mut *root),
        "TileTicks",
        compound_list(blocks),
    );
    nbt::remove(chunk_level_mut(&mut *root), "ToBeTicked");
    nbt::remove(chunk_level_mut(&mut *root), "LiquidsToBeTicked");
    nbt::set(
        chunk_level_mut(&mut *root),
        "fluid_ticks",
        compound_list(fluids),
    );
    true
}

// -----------------------------------------------------------------------
// 2842 ChunkRenamesFix
// -----------------------------------------------------------------------

/// Purge the `Level` wrapper into the root with renames:
/// `TileEntities` to `block_entities`, `TileTicks` to `block_ticks`,
/// `Entities` to `entities`, `Sections` to `sections`,
/// `Structures` to `structures` with `Starts` to `starts`.
fn chunk_renames(root: &mut Tag) -> bool {
    let Some(level) = nbt::take(root, "Level") else {
        return false;
    };
    let Tag::Compound(entries) = level else {
        return false;
    };
    for (name, mut value) in entries {
        if name == "Structures" {
            nbt::rename(&mut value, "Starts", "starts");
            nbt::set(root, "structures", value);
            continue;
        }
        let renamed = match name.as_str() {
            "TileEntities" => "block_entities",
            "TileTicks" => "block_ticks",
            "Entities" => "entities",
            "Sections" => "sections",
            "Structures" => "structures",
            _ => name.as_str(),
        };
        nbt::set(root, renamed, value);
    }
    true
}

// -----------------------------------------------------------------------
// 2843 OverreachingTickFix
// -----------------------------------------------------------------------

/// Ticks whose chunk differs from this chunk but reaches at most one chunk
/// away: `(|dx| != 0 || |dz| != 0) && |dx| <= 1 && |dz| <= 1` on
/// block coordinates shifted by 4. Matches are *copied* into
/// `UpgradeData.neighbor_*_ticks`; the source lists are left in place.
fn overreaching(entries: &[Tag], x_pos: i32, z_pos: i32) -> Vec<Tag> {
    entries
        .iter()
        .filter(|entry| {
            let x = nbt::get(entry, "x").and_then(nbt::int).unwrap_or(0);
            let z = nbt::get(entry, "z").and_then(nbt::int).unwrap_or(0);
            let dx = (x_pos - (x >> 4)).abs();
            let dz = (z_pos - (z >> 4)).abs();
            (dx != 0 || dz != 0) && dx <= 1 && dz <= 1
        })
        .cloned()
        .collect()
}

fn overreaching_ticks(root: &mut Tag) -> bool {
    let x_pos = nbt::get(root, "xPos").and_then(nbt::int).unwrap_or(0);
    let z_pos = nbt::get(root, "zPos").and_then(nbt::int).unwrap_or(0);
    let blocks = nbt::get(root, "block_ticks")
        .and_then(nbt::list)
        .map(|list| overreaching(list, x_pos, z_pos))
        .unwrap_or_default();
    let fluids = nbt::get(root, "fluid_ticks")
        .and_then(nbt::list)
        .map(|list| overreaching(list, x_pos, z_pos))
        .unwrap_or_default();
    if blocks.is_empty() && fluids.is_empty() {
        return false;
    }
    if nbt::get(root, "UpgradeData").is_none() {
        nbt::set(root, "UpgradeData", Tag::Compound(Vec::new()));
    }
    if let Some(upgrade) = nbt::get_mut(root, "UpgradeData") {
        if !blocks.is_empty() {
            nbt::set(upgrade, "neighbor_block_ticks", compound_list(blocks));
        }
        if !fluids.is_empty() {
            nbt::set(upgrade, "neighbor_fluid_ticks", compound_list(fluids));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_at(tag: &mut Tag, version: i32) -> UpgradeReport {
        let mut report = UpgradeReport::default();
        apply(tag, version, &mut report).expect("legacy chunk fix must not fail");
        report
    }

    fn legacy_root(level: Tag) -> Tag {
        nbt::compound(vec![("Level", level)])
    }

    fn legacy_section(y: i32, palette: Vec<Tag>, longs: Vec<i64>) -> Tag {
        nbt::compound(vec![
            ("Y", Tag::Int(y)),
            ("Palette", compound_list(palette)),
            ("BlockStates", Tag::LongArray(longs)),
        ])
    }

    fn stone_section(y: i32) -> Tag {
        legacy_section(
            y,
            vec![nbt::compound(vec![("Name", text("minecraft:stone"))])],
            vec![0; 64],
        )
    }

    fn overworld_context() -> Tag {
        nbt::compound(vec![
            ("dimension", text("minecraft:overworld")),
            ("generator", text("minecraft:noise")),
        ])
    }

    fn legacy_level(sections: Vec<Tag>) -> Tag {
        nbt::compound(vec![
            ("xPos", Tag::Int(3)),
            ("zPos", Tag::Int(-2)),
            ("Status", text("full")),
            ("Sections", compound_list(sections)),
            (
                "Heightmaps",
                nbt::compound(vec![("MOTION_BLOCKING", Tag::LongArray(vec![0; 32]))]),
            ),
            ("CarvingMasks", Tag::Compound(Vec::new())),
            ("ToBeTicked", compound_list(vec![compound_list(vec![])])),
            (
                "LiquidsToBeTicked",
                compound_list(vec![compound_list(vec![])]),
            ),
            ("PostProcessing", compound_list(vec![compound_list(vec![])])),
            (
                "UpgradeData",
                nbt::compound(vec![("Indices", nbt::compound(vec![("0", Tag::Int(1))]))]),
            ),
        ])
    }
    // -- 2527 alignment --------------------------------------------------

    #[test]
    fn straddling_entries_realign_without_spanning_longs() {
        // 5-bit storage, 4096 entries: entry 12 spans longs 0 and 1 in the
        // old layout. Values must survive the rewrite into aligned longs.
        let size = 4096;
        let bits = 5;
        let values: Vec<u64> = (0..size).map(|i| (i * 7 % 32) as u64).collect();
        let mut straddling = vec![0_i64; 321];
        for (index, value) in values.iter().enumerate() {
            let start = index * bits;
            let word = start / 64;
            let offset = start % 64;
            straddling[word] |= (*value as i64) << offset;
            if offset + bits > 64 {
                straddling[word + 1] |= (*value as i64) >> (64 - offset);
            }
        }
        let aligned = align_storage(size, bits as u32, &straddling);
        assert_eq!(aligned.len(), 342);
        for (index, value) in values.iter().enumerate() {
            let per_long = 64 / bits;
            let slot = index / per_long;
            let at = (index % per_long) * bits;
            assert_eq!(((aligned[slot] as u64) >> at) & 31, *value);
        }
    }

    #[test]
    fn aligned_widths_and_empty_arrays_are_untouched() {
        let mut root = legacy_root(nbt::compound(vec![
            (
                "Sections",
                compound_list(vec![legacy_section(
                    0,
                    vec![air_block_state()],
                    vec![0; 64],
                )]),
            ),
            (
                "Heightmaps",
                nbt::compound(vec![("MOTION_BLOCKING", Tag::LongArray(vec![]))]),
            ),
        ]));
        let before = root.clone();
        let report = apply_at(&mut root, 2527);
        assert_eq!(root, before);
        assert!(report.steps.is_empty());
    }

    // -- 2832 containers ------------------------------------------------

    #[test]
    fn single_entry_palette_drops_data_array() {
        let container = optimized_block_container(
            vec![nbt::compound(vec![("Name", text("minecraft:stone"))])],
            vec![7; 64],
        );
        assert!(nbt::get(&container, "data").is_none());
        assert_eq!(
            nbt::get(&container, "palette")
                .and_then(nbt::list)
                .map(<[Tag]>::len),
            Some(1)
        );
    }

    #[test]
    fn biome_id_table_and_fallback() {
        assert_eq!(biome_by_id(21), "minecraft:jungle");
        assert_eq!(biome_by_id(3), "minecraft:mountains");
        assert_eq!(biome_by_id(999), PLAINS);
    }

    #[test]
    fn heightmap_fix_adds_64_and_keeps_zero() {
        // One 9-bit field holding 1 becomes 65; a zero field stays 0.
        assert_eq!(fix_heightmap_value(1), 65);
        assert_eq!(fix_heightmap_value(0), 0);
        assert_eq!(fix_heightmap_value(511), 511);
    }

    // -- 2832 heights ----------------------------------------------------

    #[test]
    fn overworld_grows_to_24_sections_with_biomes() {
        let sections: Vec<Tag> = (0..16).map(stone_section).collect();
        let mut root = legacy_root(legacy_level(sections));
        nbt::set(&mut root, "__context", overworld_context());
        let report = apply_at(&mut root, 2832);
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.fix == "ChunkHeightAndBiomeFix")
        );
        let sections = nbt::get(chunk_level_mut(&mut root), "Sections")
            .and_then(nbt::list)
            .expect("sections");
        assert_eq!(sections.len(), 24);
        let ys: Vec<i32> = sections.iter().filter_map(section_y).collect();
        assert!(ys.contains(&-4) && ys.contains(&19));
        for section in sections {
            assert!(nbt::get(section, "block_states").is_some());
            assert!(nbt::get(section, "biomes").is_some());
            assert!(nbt::get(section, "Palette").is_none());
        }
        assert!(nbt::get(chunk_level_mut(&mut root), "Biomes").is_none());
    }

    #[test]
    fn non_overworld_stays_16_sections_without_status_change() {
        let sections: Vec<Tag> = (0..16).map(stone_section).collect();
        let mut root = legacy_root(legacy_level(sections));
        nbt::set(
            &mut root,
            "__context",
            nbt::compound(vec![("dimension", text("minecraft:the_nether"))]),
        );
        apply_at(&mut root, 2832);
        let sections = nbt::get(chunk_level_mut(&mut root), "Sections")
            .and_then(nbt::list)
            .expect("sections");
        assert_eq!(sections.len(), 16);
        assert_eq!(
            nbt::get(chunk_level_mut(&mut root), "Status").and_then(nbt::string),
            Some("full")
        );
        assert!(nbt::get(chunk_level_mut(&mut root), "blending_data").is_none());
    }

    #[test]
    fn missing_context_defaults_to_non_overworld() {
        let sections: Vec<Tag> = (0..16).map(stone_section).collect();
        let mut root = legacy_root(legacy_level(sections));
        apply_at(&mut root, 2832);
        let sections = nbt::get(chunk_level_mut(&mut root), "Sections")
            .and_then(nbt::list)
            .expect("sections");
        assert_eq!(sections.len(), 16);
    }

    #[test]
    fn below_zero_retrogen_needs_bedrock_and_noise() {
        // y=0 section with bedrock: retrogen is written, status empties.
        let bedrock = legacy_section(
            0,
            vec![nbt::compound(vec![("Name", text("minecraft:bedrock"))])],
            vec![0; 64],
        );
        let mut sections = vec![bedrock];
        sections.extend((1..16).map(stone_section));
        let mut root = legacy_root(legacy_level(sections));
        nbt::set(&mut root, "__context", overworld_context());
        apply_at(&mut root, 2832);
        assert!(nbt::get(chunk_level_mut(&mut root), "below_zero_retrogen").is_some());
        assert_eq!(
            nbt::get(chunk_level_mut(&mut root), "Status").and_then(nbt::string),
            Some("empty")
        );
        assert!(nbt::get(chunk_level_mut(&mut root), "blending_data").is_some());
    }

    #[test]
    fn all_stone_column_skips_retrogen_but_keeps_blending() {
        let sections: Vec<Tag> = (0..16).map(stone_section).collect();
        let mut root = legacy_root(legacy_level(sections));
        nbt::set(&mut root, "__context", overworld_context());
        apply_at(&mut root, 2832);
        assert!(nbt::get(chunk_level_mut(&mut root), "below_zero_retrogen").is_none());
        assert!(nbt::get(chunk_level_mut(&mut root), "blending_data").is_some());
    }

    // -- 2838 renames ----------------------------------------------------

    fn renamed_palette_section(old: &str) -> Tag {
        nbt::compound(vec![
            ("Y", Tag::Int(0)),
            (
                "biomes",
                nbt::compound(vec![("palette", compound_list(vec![text(old)]))]),
            ),
        ])
    }

    #[test]
    fn caves_and_cliffs_biome_renames_apply_in_wrapped_sections() {
        let mut root = legacy_root(nbt::compound(vec![(
            "Sections",
            compound_list(vec![renamed_palette_section("minecraft:mountains")]),
        )]));
        let report = apply_at(&mut root, 2838);
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.fix == "NamespacedTypeRenameFix")
        );
        let sections = nbt::get(chunk_level_mut(&mut root), "Sections")
            .and_then(nbt::list)
            .expect("sections");
        let name = sections
            .first()
            .and_then(|section| nbt::get(section, "biomes"))
            .and_then(|biomes| nbt::get(biomes, "palette"))
            .and_then(nbt::list)
            .and_then(|palette| palette.first())
            .and_then(nbt::string);
        assert_eq!(name, Some("minecraft:windswept_hills"));
    }

    #[test]
    fn unknown_biomes_are_left_alone_without_receipt() {
        let mut root = legacy_root(nbt::compound(vec![(
            "Sections",
            compound_list(vec![renamed_palette_section("minecraft:plains")]),
        )]));
        let report = apply_at(&mut root, 2838);
        assert!(
            !report
                .steps
                .iter()
                .any(|step| step.fix == "NamespacedTypeRenameFix")
        );
    }

    #[test]
    fn compound_palette_entries_are_opaque_to_biome_renames() {
        // Vanilla's rename runs over the string BIOME schema: a compound
        // palette value is not a biome name and must pass through untouched.
        let compound = nbt::compound(vec![("Name", text("minecraft:mountains"))]);
        let mut root = legacy_root(nbt::compound(vec![(
            "Sections",
            compound_list(vec![nbt::compound(vec![
                ("Y", Tag::Int(0)),
                (
                    "biomes",
                    nbt::compound(vec![("palette", compound_list(vec![compound]))]),
                ),
            ])]),
        )]));
        let before = root.clone();
        let report = apply_at(&mut root, 2838);
        assert_eq!(root, before);
        assert!(report.steps.is_empty());
    }

    // -- 2841 ticks ------------------------------------------------------

    fn proto_section(y: i32) -> Tag {
        nbt::compound(vec![
            ("Y", Tag::Int(y)),
            (
                "block_states",
                nbt::compound(vec![(
                    "palette",
                    compound_list(vec![nbt::compound(vec![("Name", text("minecraft:stone"))])]),
                )]),
            ),
            ("biomes", single_biome_container()),
        ])
    }

    fn proto_level() -> Tag {
        // One y=0 section whose palette is [stone]; ToBeTicked holds packed 0
        // (filtered) and packed 1 (x=1, kept as stone).
        nbt::compound(vec![
            ("xPos", Tag::Int(0)),
            ("zPos", Tag::Int(0)),
            ("Sections", compound_list(vec![proto_section(0)])),
            (
                "ToBeTicked",
                compound_list(vec![compound_list(vec![Tag::Short(0), Tag::Short(1)])]),
            ),
            (
                "LiquidsToBeTicked",
                compound_list(vec![compound_list(vec![])]),
            ),
        ])
    }

    #[test]
    fn proto_ticks_build_lists_and_leave_absent_ypos_absent() {
        let mut root = legacy_root(proto_level());
        let report = apply_at(&mut root, 2841);
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.fix == "ChunkProtoTickListFix")
        );
        // `yPos` is `Dynamic.update`: absent stays absent.
        assert!(nbt::get(chunk_level_mut(&mut root), "yPos").is_none());
        let (tick_i, tick_x) = {
            let ticks = nbt::get(chunk_level_mut(&mut root), "TileTicks")
                .and_then(nbt::list)
                .expect("ticks");
            assert_eq!(ticks.len(), 1);
            (
                nbt::get(&ticks[0], "i")
                    .and_then(nbt::string)
                    .unwrap_or("")
                    .to_owned(),
                nbt::get(&ticks[0], "x").and_then(nbt::int).unwrap_or(-1),
            )
        };
        assert_eq!(tick_i, "minecraft:stone");
        assert_eq!(tick_x, 1);
        assert!(nbt::get(chunk_level_mut(&mut root), "ToBeTicked").is_none());
        assert!(nbt::get(chunk_level_mut(&mut root), "LiquidsToBeTicked").is_none());
        assert!(nbt::get(chunk_level_mut(&mut root), "fluid_ticks").is_some());
    }

    #[test]
    fn proto_ticks_stamp_present_ypos_as_byte() {
        let mut level = proto_level();
        nbt::set(&mut level, "yPos", Tag::Int(0));
        let mut root = legacy_root(level);
        apply_at(&mut root, 2841);
        assert!(nbt::get(chunk_level_mut(&mut root), "yPos").is_some_and(|v| *v == Tag::Byte(0)));
    }

    #[test]
    fn proto_min_y_ignores_sections_without_biomes() {
        // A y=-4 section without `biomes` must not move the minimum: the
        // remaining y=0 section keeps min 0.
        let mut bare = proto_section(-4);
        nbt::remove(&mut bare, "biomes");
        let mut level = proto_level();
        if let Some(sections) = nbt::get_mut(&mut level, "Sections").and_then(nbt::list_mut) {
            sections.insert(0, bare);
        }
        nbt::set(&mut level, "yPos", Tag::Int(9));
        let mut root = legacy_root(level);
        apply_at(&mut root, 2841);
        assert!(nbt::get(chunk_level_mut(&mut root), "yPos").is_some_and(|v| *v == Tag::Byte(0)));
    }

    #[test]
    fn proto_ticks_waterlogged_resolves_to_water() {
        let mut level = proto_level();
        // Palette entry: kelp (always waterlogged) at index 0.
        let sections = nbt::get_mut(&mut level, "Sections")
            .and_then(nbt::list_mut)
            .expect("sections");
        if let Some(states) = nbt::get_mut(&mut sections[0], "block_states") {
            nbt::set(
                states,
                "palette",
                compound_list(vec![nbt::compound(vec![("Name", text("minecraft:kelp"))])]),
            );
        }
        nbt::set(
            &mut level,
            "LiquidsToBeTicked",
            compound_list(vec![compound_list(vec![Tag::Short(1)])]),
        );
        let mut root = legacy_root(level);
        apply_at(&mut root, 2841);
        let fluids = nbt::get(chunk_level_mut(&mut root), "fluid_ticks")
            .and_then(nbt::list)
            .expect("fluids");
        assert_eq!(fluids.len(), 1);
        assert_eq!(
            nbt::get(&fluids[0], "i").and_then(nbt::string),
            Some("minecraft:water")
        );
    }

    #[test]
    fn liquid_ticks_move_verbatim_and_keep_pending_lists() {
        // `LiquidTicks` renames to `fluid_ticks` first; the guard then keeps
        // the existing tick lists, so pending proto lists stay in place.
        let mut level = proto_level();
        let liquids = compound_list(vec![nbt::compound(vec![("i", text("minecraft:water"))])]);
        nbt::set(&mut level, "LiquidTicks", liquids.clone());
        let mut root = legacy_root(level);
        let report = apply_at(&mut root, 2841);
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.fix == "ChunkProtoTickListFix")
        );
        assert!(nbt::get(chunk_level_mut(&mut root), "fluid_ticks").is_some_and(|v| *v == liquids));
        assert!(nbt::get(chunk_level_mut(&mut root), "LiquidTicks").is_none());
        assert!(nbt::get(chunk_level_mut(&mut root), "TileTicks").is_none());
        assert!(nbt::get(chunk_level_mut(&mut root), "ToBeTicked").is_some());
    }

    #[test]
    fn present_tile_ticks_keep_existing_lists() {
        let mut level = proto_level();
        nbt::set(&mut level, "TileTicks", compound_list(vec![]));
        let mut root = legacy_root(level);
        apply_at(&mut root, 2841);
        assert!(nbt::get(chunk_level_mut(&mut root), "ToBeTicked").is_some());
        assert!(nbt::get(chunk_level_mut(&mut root), "fluid_ticks").is_none());
    }

    // -- 2842 renames -----------------------------------------------------

    #[test]
    fn chunk_renames_purge_level_wrapper() {
        let mut root = legacy_root(nbt::compound(vec![
            ("xPos", Tag::Int(1)),
            ("TileEntities", compound_list(vec![])),
            ("TileTicks", compound_list(vec![])),
            ("Entities", compound_list(vec![])),
            ("Sections", compound_list(vec![])),
            (
                "Structures",
                nbt::compound(vec![("Starts", nbt::compound(vec![]))]),
            ),
        ]));
        let report = apply_at(&mut root, 2842);
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.fix == "ChunkRenamesFix")
        );
        assert!(nbt::get(&root, "Level").is_none());
        for key in [
            "block_entities",
            "block_ticks",
            "entities",
            "sections",
            "structures",
        ] {
            assert!(nbt::get(&root, key).is_some(), "missing {key}");
        }
        assert!(
            nbt::get(&root, "structures")
                .and_then(|structures| nbt::get(structures, "starts"))
                .is_some()
        );
    }

    // -- 2843 overreach + deep warm ocean ---------------------------------

    fn tick_at(x: i32, z: i32) -> Tag {
        nbt::compound(vec![
            ("i", text("minecraft:stone")),
            ("x", Tag::Int(x)),
            ("y", Tag::Int(64)),
            ("z", Tag::Int(z)),
            ("t", Tag::Int(0)),
            ("p", Tag::Int(0)),
        ])
    }

    #[test]
    fn overreaching_ticks_are_copied_not_moved() {
        let mut root = nbt::compound(vec![
            ("xPos", Tag::Int(0)),
            ("zPos", Tag::Int(0)),
            (
                "block_ticks",
                compound_list(vec![tick_at(8, 8), tick_at(16, 8)]),
            ),
        ]);
        let report = apply_at(&mut root, 2843);
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.fix == "OverreachingTickFix")
        );
        // Source list keeps both entries.
        assert_eq!(
            nbt::get(&root, "block_ticks")
                .and_then(nbt::list)
                .map(<[Tag]>::len),
            Some(2)
        );
        let neighbors = nbt::get(&root, "UpgradeData")
            .and_then(|upgrade| nbt::get(upgrade, "neighbor_block_ticks"))
            .and_then(nbt::list)
            .expect("neighbors");
        assert_eq!(neighbors.len(), 1);
        assert_eq!(nbt::get(&neighbors[0], "x").and_then(nbt::int), Some(16));
    }

    #[test]
    fn local_ticks_produce_no_upgrade_data_or_receipt() {
        let mut root = nbt::compound(vec![
            ("xPos", Tag::Int(0)),
            ("zPos", Tag::Int(0)),
            ("block_ticks", compound_list(vec![tick_at(8, 8)])),
        ]);
        let report = apply_at(&mut root, 2843);
        assert!(
            !report
                .steps
                .iter()
                .any(|step| step.fix == "OverreachingTickFix")
        );
        assert!(nbt::get(&root, "UpgradeData").is_none());
    }

    #[test]
    fn deep_warm_ocean_renames_in_modern_sections() {
        let mut root = nbt::compound(vec![(
            "sections",
            compound_list(vec![renamed_palette_section("minecraft:deep_warm_ocean")]),
        )]);
        let report = apply_at(&mut root, 2843);
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.fix == "NamespacedTypeRenameFix")
        );
        let name = nbt::get(&root, "sections")
            .and_then(nbt::list)
            .and_then(|list| list.first())
            .and_then(|section| nbt::get(section, "biomes"))
            .and_then(|biomes| nbt::get(biomes, "palette"))
            .and_then(nbt::list)
            .and_then(|palette| palette.first())
            .and_then(nbt::string);
        assert_eq!(name, Some("minecraft:warm_ocean"));
    }

    // -- dispatcher -------------------------------------------------------

    #[test]
    fn unrelated_versions_are_no_ops() {
        let mut root = legacy_root(legacy_level(vec![stone_section(0)]));
        let before = root.clone();
        let report = apply_at(&mut root, 2831);
        assert_eq!(root, before);
        assert!(report.steps.is_empty());
    }
}
