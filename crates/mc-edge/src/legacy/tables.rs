//! Canonical (server) -> legacy (26.1.2) id tables, derived at startup from the
//! two content caches' datagen reports: `packets.json`, `blocks.json` and
//! `registries.json`. Ids are matched by name; names that only exist on the
//! canonical side go through the polyfill rules below.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// New blocks (and their block items) shown to a 26.1.2 client as the nearest
/// existing block. Exact names first; then the `poplar` -> `birch` wood rule and
/// the coloured concrete/wool slab and stairs rule ([`COLOURED_SHAPES`]).
const BLOCKS: &[(&str, &str)] = &[
    ("cinnabar", "granite"),
    ("cinnabar_slab", "granite_slab"),
    ("cinnabar_stairs", "granite_stairs"),
    ("cinnabar_wall", "granite_wall"),
    ("cinnabar_bricks", "red_nether_bricks"),
    ("cinnabar_brick_slab", "red_nether_brick_slab"),
    ("cinnabar_brick_stairs", "red_nether_brick_stairs"),
    ("cinnabar_brick_wall", "red_nether_brick_wall"),
    ("polished_cinnabar", "polished_granite"),
    ("polished_cinnabar_slab", "polished_granite_slab"),
    ("polished_cinnabar_stairs", "polished_granite_stairs"),
    ("polished_cinnabar_wall", "granite_wall"),
    ("chiseled_cinnabar", "chiseled_red_sandstone"),
    ("sulfur", "end_stone"),
    ("sulfur_slab", "end_stone_brick_slab"),
    ("sulfur_stairs", "end_stone_brick_stairs"),
    ("sulfur_wall", "end_stone_brick_wall"),
    ("sulfur_bricks", "end_stone_bricks"),
    ("sulfur_brick_slab", "end_stone_brick_slab"),
    ("sulfur_brick_stairs", "end_stone_brick_stairs"),
    ("sulfur_brick_wall", "end_stone_brick_wall"),
    ("polished_sulfur", "smooth_sandstone"),
    ("polished_sulfur_slab", "smooth_sandstone_slab"),
    ("polished_sulfur_stairs", "smooth_sandstone_stairs"),
    ("polished_sulfur_wall", "sandstone_wall"),
    ("chiseled_sulfur", "chiseled_sandstone"),
    ("potent_sulfur", "yellow_terracotta"),
    ("sulfur_spike", "pointed_dripstone"),
    ("orange_poplar_leaves", "birch_leaves"),
    ("red_poplar_leaves", "birch_leaves"),
    ("yellow_poplar_leaves", "birch_leaves"),
    ("red_shrub", "crimson_roots"),
    ("shelf_mushroom", "brown_mushroom"),
    ("straw_bed", "yellow_bed"),
];

/// Items without a block of the same name.
const ITEMS: &[(&str, &str)] = &[
    ("music_disc_bounce", "music_disc_cat"),
    ("sulfur_cube_bucket", "bucket"),
    ("sulfur_cube_spawn_egg", "slime_spawn_egg"),
];

/// `<colour>_concrete|wool_slab|stairs` (and `<colour>_cushion` items) use an
/// existing material of a similar colour with the same shape.
const COLOURED_SHAPES: &[(&str, &str)] = &[
    ("white", "quartz"),
    ("light_gray", "stone"),
    ("gray", "andesite"),
    ("black", "blackstone"),
    ("brown", "spruce"),
    ("red", "red_nether_brick"),
    ("orange", "acacia"),
    ("yellow", "sandstone"),
    ("lime", "bamboo"),
    ("green", "mossy_cobblestone"),
    ("cyan", "warped"),
    ("light_blue", "prismarine"),
    ("blue", "dark_prismarine"),
    ("purple", "purpur"),
    ("magenta", "crimson"),
    ("pink", "cherry"),
];

/// New entity types and the metadata layout a 26.1.2 client may receive.
const ENTITIES: &[(&str, &str, Metadata)] = &[
    ("sulfur_cube", "slime", Metadata::Cube),
    ("cushion", "interaction", Metadata::Below(8)),
    ("poplar_boat", "birch_boat", Metadata::Same),
    ("poplar_chest_boat", "birch_chest_boat", Metadata::Same),
];

/// New entries of synced data-driven registries.
const DATA_ENTRIES: &[(&str, &str)] = &[
    ("dappled_forest", "forest"),
    ("sulfur_caves", "dripstone_caves"),
    ("sulfur_cube_hot", "hot_floor"),
];

/// How an entity's `SetEntityData` indices differ for the legacy client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metadata {
    Same,
    /// 26.3 slime-like mobs extend `AgeableMob`: drop baby (16) and age lock
    /// (17), move size 18 -> 16, drop everything after.
    Cube,
    /// Keep only indices below this (the shared base classes).
    Below(u8),
}

fn polyfill_block(name: &str) -> Option<String> {
    if let Some((_, old)) = BLOCKS.iter().find(|(new, _)| *new == name) {
        return Some((*old).to_owned());
    }
    if name.contains("poplar") {
        return Some(name.replace("poplar", "birch"));
    }
    for shape in ["_slab", "_stairs"] {
        if let Some(stem) = name.strip_suffix(shape)
            && let Some(colour) = stem
                .strip_suffix("_concrete")
                .or_else(|| stem.strip_suffix("_wool"))
            && let Some((_, material)) = COLOURED_SHAPES.iter().find(|(c, _)| *c == colour)
        {
            return Some(format!("{material}{shape}"));
        }
    }
    None
}

fn polyfill_item(name: &str) -> Option<String> {
    if let Some((_, old)) = ITEMS.iter().find(|(new, _)| *new == name) {
        return Some((*old).to_owned());
    }
    if let Some(colour) = name.strip_suffix("_cushion") {
        return Some(format!("{colour}_carpet"));
    }
    if name.ends_with("_map") {
        return Some("map".to_owned());
    }
    polyfill_block(name)
}

fn bare(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

fn namespaced(name: &str) -> String {
    format!("minecraft:{name}")
}

// --- report parsing -------------------------------------------------------

#[derive(Deserialize)]
struct ProtocolId {
    protocol_id: i32,
}

#[derive(Deserialize)]
struct RegistryReport {
    entries: HashMap<String, ProtocolId>,
}

#[derive(Deserialize)]
struct BlockReport {
    states: Vec<BlockStateReport>,
}

#[derive(Deserialize)]
struct BlockStateReport {
    id: u32,
    #[serde(default)]
    properties: HashMap<String, String>,
    #[serde(default)]
    default: bool,
}

type PacketReport = HashMap<String, HashMap<String, HashMap<String, ProtocolId>>>;

fn read_json<T: serde::de::DeserializeOwned>(root: &Path, file: &str) -> Result<T> {
    let path = root.join("reports").join(file);
    let raw = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&raw).with_context(|| format!("parsing {}", path.display()))
}

/// Datagen reports of one content cache.
pub struct Reports {
    packets: PacketReport,
    registries: HashMap<String, RegistryReport>,
    blocks: HashMap<String, BlockReport>,
}

impl Reports {
    pub fn load(root: &Path) -> Result<Self> {
        Ok(Self {
            packets: read_json(root, "packets.json")?,
            registries: read_json(root, "registries.json")?,
            blocks: read_json(root, "blocks.json")?,
        })
    }
    /// Reports from inline JSON (tests).
    #[cfg(test)]
    pub fn parse(packets: &str, registries: &str, blocks: &str) -> Result<Self> {
        Ok(Self {
            packets: serde_json::from_str(packets)?,
            registries: serde_json::from_str(registries)?,
            blocks: serde_json::from_str(blocks)?,
        })
    }

    fn packet_names(&self, state: &str, direction: &str) -> Vec<String> {
        let Some(packets) = self.packets.get(state).and_then(|d| d.get(direction)) else {
            return Vec::new();
        };
        let len = packets
            .values()
            .map(|id| id.protocol_id as usize + 1)
            .max()
            .unwrap_or(0);
        let mut names = vec![String::new(); len];
        for (name, id) in packets {
            names[id.protocol_id as usize].clone_from(name);
        }
        names
    }

    fn registry(&self, name: &str) -> Result<&HashMap<String, ProtocolId>> {
        self.registries
            .get(name)
            .map(|registry| &registry.entries)
            .with_context(|| format!("registries.json has no {name}"))
    }
}

// --- packet ids -----------------------------------------------------------

/// Packet names of one (state, direction), indexed by wire id.
#[derive(Debug, Default)]
pub struct PacketNames {
    pub canonical: Vec<String>,
    legacy_ids: HashMap<String, i32>,
    pub legacy: Vec<String>,
    canonical_ids: HashMap<String, i32>,
}

impl PacketNames {
    fn new(canonical: Vec<String>, legacy: Vec<String>) -> Self {
        let ids = |names: &[String]| {
            names
                .iter()
                .enumerate()
                .map(|(id, name)| (name.clone(), id as i32))
                .collect()
        };
        Self {
            legacy_ids: ids(&legacy),
            canonical_ids: ids(&canonical),
            canonical,
            legacy,
        }
    }

    /// Canonical packet name for a canonical id.
    pub fn canonical_name(&self, id: i32) -> Option<&str> {
        usize::try_from(id)
            .ok()
            .and_then(|id| self.canonical.get(id))
            .filter(|name| !name.is_empty())
            .map(String::as_str)
    }

    /// Legacy packet name for a legacy id.
    pub fn legacy_name(&self, id: i32) -> Option<&str> {
        usize::try_from(id)
            .ok()
            .and_then(|id| self.legacy.get(id))
            .filter(|name| !name.is_empty())
            .map(String::as_str)
    }

    pub fn legacy_id(&self, name: &str) -> Option<i32> {
        self.legacy_ids.get(name).copied()
    }

    pub fn canonical_id(&self, name: &str) -> Option<i32> {
        self.canonical_ids.get(name).copied()
    }
}

/// Clientbound and serverbound names for every protocol state.
#[derive(Debug, Default)]
pub struct Packets {
    pub login_cb: PacketNames,
    pub login_sb: PacketNames,
    pub config_cb: PacketNames,
    pub config_sb: PacketNames,
    pub play_cb: PacketNames,
    pub play_sb: PacketNames,
}

impl Packets {
    pub fn new(canonical: &Reports, legacy: &Reports) -> Self {
        let pair = |state, direction| {
            PacketNames::new(
                canonical.packet_names(state, direction),
                legacy.packet_names(state, direction),
            )
        };
        Self {
            login_cb: pair("login", "clientbound"),
            login_sb: pair("login", "serverbound"),
            config_cb: pair("configuration", "clientbound"),
            config_sb: pair("configuration", "serverbound"),
            play_cb: pair("play", "clientbound"),
            play_sb: pair("play", "serverbound"),
        }
    }
}

// --- registry ids ---------------------------------------------------------

/// Canonical id -> legacy id for one registry; `None` has no legacy entry.
#[derive(Debug, Default, Clone)]
pub struct IdMap {
    forward: Vec<Option<u32>>,
    reverse: Vec<Option<u32>>,
}

impl IdMap {
    /// Pair two `name -> id` listings, substituting through `polyfill` (names
    /// without the namespace) when the canonical name has no legacy entry.
    pub fn by_name<'a>(
        canonical: impl IntoIterator<Item = (&'a str, u32)>,
        legacy: impl IntoIterator<Item = (&'a str, u32)>,
        polyfill: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let legacy: HashMap<&str, u32> = legacy.into_iter().map(|(n, id)| (bare(n), id)).collect();
        let mut map = Self::default();
        for (name, id) in canonical {
            let name = bare(name);
            let target = legacy
                .get(name)
                .copied()
                .or_else(|| polyfill(name).and_then(|old| legacy.get(old.as_str()).copied()));
            map.set(id, target, legacy.contains_key(name));
        }
        map
    }

    fn set(&mut self, canonical: u32, legacy: Option<u32>, exact: bool) {
        let index = canonical as usize;
        if self.forward.len() <= index {
            self.forward.resize(index + 1, None);
        }
        self.forward[index] = legacy;
        if let Some(legacy) = legacy.filter(|_| exact) {
            let index = legacy as usize;
            if self.reverse.len() <= index {
                self.reverse.resize(index + 1, None);
            }
            self.reverse[index] = Some(canonical);
        }
    }

    /// Legacy id for a canonical id.
    pub fn get(&self, canonical: i32) -> Option<i32> {
        usize::try_from(canonical)
            .ok()
            .and_then(|index| self.forward.get(index).copied().flatten())
            .map(|id| id as i32)
    }

    /// Canonical id for a legacy id (exact name matches only).
    pub fn back(&self, legacy: i32) -> Option<i32> {
        usize::try_from(legacy)
            .ok()
            .and_then(|index| self.reverse.get(index).copied().flatten())
            .map(|id| id as i32)
    }
}

/// Every id table the translator needs.
#[derive(Debug, Default)]
pub struct Tables {
    pub packets: Packets,
    /// Global block-state ids; total (every canonical state has a legacy one).
    pub block_states: IdMap,
    /// Bits a legacy client uses for direct (global) block palettes.
    pub legacy_block_bits: u8,
    pub blocks: IdMap,
    pub legacy_brick: i32,
    pub items: IdMap,
    pub entities: IdMap,
    /// Canonical entity type -> metadata policy (absent = same layout).
    pub entity_metadata: HashMap<i32, Metadata>,
    pub block_entities: IdMap,
    pub sounds: IdMap,
    pub particles: IdMap,
    pub components: IdMap,
    pub attributes: IdMap,
    pub argument_types: IdMap,
    pub argument_names: Vec<String>,
    /// Other built-in and synced registry ids used by components and displays.
    pub registry_ids: HashMap<String, IdMap>,
    pub biomes: IdMap,
    pub legacy_biome_bits: u8,
    pub damage_types: IdMap,
    /// Canonical entity type id of `falling_block` (its spawn data is a state).
    pub falling_block: i32,
    /// Canonical sound names, for sounds a legacy client lacks (sent inline).
    pub sound_names: Vec<String>,
    /// Canonical particle names (their payload shape is chosen by name).
    pub particle_names: Vec<String>,
    /// Item component payload shapes on both sides.
    pub component_shapes: super::items::Components,
}

/// Registry entry names indexed by protocol id.
fn names_by_id(map: &HashMap<String, ProtocolId>) -> Vec<String> {
    let mut names = vec![String::new(); map.len()];
    for (name, id) in map {
        if let Some(slot) = names.get_mut(id.protocol_id as usize) {
            slot.clone_from(name);
        }
    }
    names
}

fn entries(map: &HashMap<String, ProtocolId>) -> impl Iterator<Item = (&str, u32)> {
    map.iter()
        .map(|(name, id)| (name.as_str(), id.protocol_id as u32))
}

fn bits_for(count: usize) -> u8 {
    (usize::BITS - count.saturating_sub(1).leading_zeros()).max(1) as u8
}

impl Tables {
    /// `canonical_data`/`legacy_data` are the synced data-driven registries
    /// each side announces in Configuration (entry order = wire id).
    pub fn new(
        canonical: &Reports,
        legacy: &Reports,
        canonical_data: &mc_data::VanillaData,
        legacy_data: &mc_data::VanillaData,
    ) -> Result<Self> {
        let none = |_: &str| None;
        let registry = |name: &str, polyfill: &dyn Fn(&str) -> Option<String>| -> Result<IdMap> {
            Ok(IdMap::by_name(
                entries(canonical.registry(name)?),
                entries(legacy.registry(name)?),
                polyfill,
            ))
        };
        let entity_polyfill = |name: &str| {
            ENTITIES
                .iter()
                .find(|(new, _, _)| *new == name)
                .map(|(_, old, _)| (*old).to_owned())
        };
        let entities = registry("minecraft:entity_type", &entity_polyfill)?;
        let entity_ids = canonical.registry("minecraft:entity_type")?;
        let mut entity_metadata = HashMap::new();
        for (name, _, metadata) in ENTITIES {
            if *metadata != Metadata::Same
                && let Some(id) = entity_ids.get(&namespaced(name))
            {
                entity_metadata.insert(id.protocol_id, *metadata);
            }
        }
        for name in ["slime", "magma_cube"] {
            if let Some(id) = entity_ids.get(&namespaced(name)) {
                entity_metadata.insert(id.protocol_id, Metadata::Cube);
            }
        }
        let falling_block = entity_ids
            .get("minecraft:falling_block")
            .map_or(-1, |id| id.protocol_id);
        let sound_names = names_by_id(canonical.registry("minecraft:sound_event")?);
        let particle_names = names_by_id(canonical.registry("minecraft:particle_type")?);
        let component_shapes = super::items::Components::new(
            &names_by_id(canonical.registry("minecraft:data_component_type")?),
            &names_by_id(legacy.registry("minecraft:data_component_type")?),
        );
        let data_registry = |name: &str| -> IdMap {
            let listing = |data: &mc_data::VanillaData| -> Vec<(String, u32)> {
                data.registry(name)
                    .map(|registry| {
                        registry
                            .entries
                            .iter()
                            .enumerate()
                            .map(|(id, entry)| (entry.as_str().to_owned(), id as u32))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let (c, l) = (listing(canonical_data), listing(legacy_data));
            IdMap::by_name(
                c.iter().map(|(n, id)| (n.as_str(), *id)),
                l.iter().map(|(n, id)| (n.as_str(), *id)),
                |name| {
                    DATA_ENTRIES
                        .iter()
                        .find(|(new, _)| *new == name)
                        .map(|(_, old)| (*old).to_owned())
                },
            )
        };
        let legacy_biomes = legacy_data
            .registry("worldgen/biome")
            .map_or(0, |registry| registry.entries.len());
        Ok(Self {
            packets: Packets::new(canonical, legacy),
            block_states: block_states(canonical, legacy)?,
            legacy_block_bits: bits_for(legacy.blocks.values().map(|b| b.states.len()).sum()),
            blocks: registry("minecraft:block", &polyfill_block)?,
            items: registry("minecraft:item", &polyfill_item)?,
            legacy_brick: legacy
                .registry("minecraft:item")?
                .get("minecraft:brick")
                .context("legacy item registry has no brick")?
                .protocol_id,
            entities,
            entity_metadata,
            block_entities: registry("minecraft:block_entity_type", &none)?,
            sounds: registry("minecraft:sound_event", &none)?,
            particles: registry("minecraft:particle_type", &none)?,
            components: registry("minecraft:data_component_type", &none)?,
            attributes: registry("minecraft:attribute", &none)?,
            argument_types: registry("minecraft:command_argument_type", &|name| {
                (name == "team_color").then(|| "color".into())
            })?,
            argument_names: names_by_id(canonical.registry("minecraft:command_argument_type")?),
            registry_ids: {
                let mut ids = HashMap::new();
                for name in [
                    "custom_stat",
                    "stat_type",
                    "menu",
                    "map_decoration_type",
                    "slot_display",
                    "recipe_display",
                    "recipe_book_category",
                    "potion",
                    "mob_effect",
                ] {
                    if canonical.registries.contains_key(&namespaced(name))
                        && legacy.registries.contains_key(&namespaced(name))
                    {
                        ids.insert(name.to_owned(), registry(&namespaced(name), &none)?);
                    }
                }
                for (name, _) in mc_data::KNOWN_REGISTRIES {
                    ids.insert((*name).to_owned(), data_registry(name));
                }
                ids
            },
            biomes: data_registry("worldgen/biome"),
            legacy_biome_bits: bits_for(legacy_biomes),
            damage_types: data_registry("damage_type"),
            falling_block,
            sound_names,
            particle_names,
            component_shapes,
        })
    }
}

/// Map every canonical block state to the legacy state of the same (or
/// polyfilled) block whose properties agree most, preferring its default.
/// Blocks with no legacy counterpart at all become stone.
fn block_states(canonical: &Reports, legacy: &Reports) -> Result<IdMap> {
    let fallback = legacy
        .blocks
        .get("minecraft:stone")
        .and_then(|block| block.states.first())
        .map(|state| state.id)
        .context("legacy blocks.json has no minecraft:stone")?;
    let mut map = IdMap::default();
    for (name, block) in &canonical.blocks {
        let exact = legacy.blocks.get(name);
        let target = exact.or_else(|| {
            let old = polyfill_block(bare(name))?;
            legacy.blocks.get(&namespaced(&old))
        });
        for state in &block.states {
            let legacy_id = target.map_or(fallback, |target| closest_state(state, target));
            map.set(state.id, Some(legacy_id), exact.is_some());
        }
    }
    Ok(map)
}

fn closest_state(state: &BlockStateReport, target: &BlockReport) -> u32 {
    target
        .states
        .iter()
        .max_by_key(|candidate| {
            let agree = candidate
                .properties
                .iter()
                .filter(|(key, value)| state.properties.get(*key) == Some(*value))
                .count();
            (agree, candidate.default)
        })
        .map_or(0, |candidate| candidate.id)
}

#[cfg(test)]
#[path = "tables_tests.rs"]
mod tests;
