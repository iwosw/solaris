//! Minimal biome JSON reader for spawn rules used by Solaris worldgen.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::{Identifier, read_json_file, sorted_json_files, visit_json_files};

mod spawn_placement;
pub use spawn_placement::SpawnPlacement;
#[path = "biome_spawn_costs.rs"]
mod spawn_costs;
pub use spawn_costs::BiomeSpawnCost;

const REQUIRED_BIOME_SPAWNS: &str = include_str!("../data/required_biome_spawns.json");
const WARM_SHEEP_COLOR_BIOMES: &[&str] = &[
    "minecraft:desert",
    "minecraft:warm_ocean",
    "minecraft:bamboo_jungle",
    "minecraft:jungle",
    "minecraft:sparse_jungle",
    "minecraft:savanna",
    "minecraft:savanna_plateau",
    "minecraft:windswept_savanna",
    "minecraft:nether_wastes",
    "minecraft:soul_sand_valley",
    "minecraft:crimson_forest",
    "minecraft:warped_forest",
    "minecraft:basalt_deltas",
    "minecraft:badlands",
    "minecraft:eroded_badlands",
    "minecraft:wooded_badlands",
    "minecraft:mangrove_swamp",
    "minecraft:deep_lukewarm_ocean",
    "minecraft:lukewarm_ocean",
];
const COLD_SHEEP_COLOR_BIOMES: &[&str] = &[
    "minecraft:snowy_plains",
    "minecraft:ice_spikes",
    "minecraft:frozen_peaks",
    "minecraft:jagged_peaks",
    "minecraft:snowy_slopes",
    "minecraft:frozen_ocean",
    "minecraft:deep_frozen_ocean",
    "minecraft:grove",
    "minecraft:deep_dark",
    "minecraft:frozen_river",
    "minecraft:snowy_taiga",
    "minecraft:snowy_beach",
    "minecraft:the_end",
    "minecraft:end_highlands",
    "minecraft:end_midlands",
    "minecraft:small_end_islands",
    "minecraft:end_barrens",
    "minecraft:cold_ocean",
    "minecraft:deep_cold_ocean",
    "minecraft:old_growth_pine_taiga",
    "minecraft:old_growth_spruce_taiga",
    "minecraft:taiga",
    "minecraft:windswept_forest",
    "minecraft:windswept_gravelly_hills",
    "minecraft:windswept_hills",
    "minecraft:stony_peaks",
];

#[derive(Debug, Error)]
pub enum BiomeDataError {
    #[error("biome directory not found at {0}")]
    Missing(PathBuf),
    #[error("biome file io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("biome file parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("biome tag parse error at {path}: {source}")]
    TagParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid biome or entity identifier {0:?}")]
    InvalidIdentifier(String),
    #[error(
        "invalid spawn cost in {biome} for {entity}: charge {charge}, energy_budget {energy_budget}; expected finite nonnegative values"
    )]
    InvalidSpawnCost {
        biome: Identifier,
        entity: Identifier,
        charge: f64,
        energy_budget: f64,
    },
}

#[derive(Debug, Clone, Default)]
pub struct BiomeWorldgenData {
    features_by_biome: BTreeMap<Identifier, Vec<Identifier>>,
    tags: BTreeMap<Identifier, Vec<Identifier>>,
}

impl BiomeWorldgenData {
    #[must_use]
    pub fn from_parts(
        features_by_biome: BTreeMap<Identifier, Vec<Identifier>>,
        tags: BTreeMap<Identifier, Vec<Identifier>>,
    ) -> Self {
        Self {
            features_by_biome,
            tags,
        }
    }

    pub fn biomes(&self) -> impl Iterator<Item = &Identifier> {
        self.features_by_biome.keys()
    }

    #[must_use]
    pub fn tag(&self, tag: &Identifier) -> &[Identifier] {
        self.tags.get(tag).map(Vec::as_slice).unwrap_or(&[])
    }

    #[must_use]
    pub fn tags_len(&self) -> usize {
        self.tags.len()
    }

    #[must_use]
    pub fn biomes_for_feature(&self, feature: &Identifier) -> Vec<Identifier> {
        self.features_by_biome
            .iter()
            .filter(|(_, features)| features.contains(feature))
            .map(|(biome, _)| biome.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiomeSpawnEntry {
    pub entity_type: Identifier,
    pub min_count: u32,
    pub max_count: u32,
    pub weight: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SheepColorClimate {
    #[default]
    Temperate,
    Warm,
    Cold,
}

#[derive(Debug, Clone, Default)]
pub struct BiomeSpawnRules {
    by_biome: BTreeMap<Identifier, BTreeMap<String, Vec<BiomeSpawnEntry>>>,
    default_land_biome: Option<Identifier>,
    default_water_biome: Option<Identifier>,
    warm_sheep_color_biomes: BTreeSet<Identifier>,
    cold_sheep_color_biomes: BTreeSet<Identifier>,
    placement: SpawnPlacement,
    spawn_costs: spawn_costs::Costs,
}

impl BiomeSpawnRules {
    pub fn placement(&self) -> SpawnPlacement {
        self.placement
    }

    /// Exact authored local density cost, without a default-biome fallback.
    #[must_use]
    pub fn spawn_cost(&self, biome: &Identifier, entity: &str) -> Option<BiomeSpawnCost> {
        self.spawn_costs.get(biome)?.get(entity).copied()
    }

    pub fn set_placement(&mut self, placement: SpawnPlacement) {
        self.placement = placement;
    }

    pub fn define_biome_spawns(
        &mut self,
        biome: Identifier,
        groups: BTreeMap<String, Vec<BiomeSpawnEntry>>,
    ) {
        self.by_biome.entry(biome).or_default().extend(groups);
    }

    #[must_use]
    pub fn from_entries(
        entries: BTreeMap<Identifier, BTreeMap<String, Vec<BiomeSpawnEntry>>>,
    ) -> Self {
        Self::from_entries_with_sheep_color_climates(entries, BTreeSet::new(), BTreeSet::new())
    }

    #[must_use]
    pub fn from_entries_with_sheep_color_climates(
        entries: BTreeMap<Identifier, BTreeMap<String, Vec<BiomeSpawnEntry>>>,
        warm_sheep_color_biomes: BTreeSet<Identifier>,
        cold_sheep_color_biomes: BTreeSet<Identifier>,
    ) -> Self {
        Self {
            by_biome: entries,
            default_land_biome: None,
            default_water_biome: None,
            warm_sheep_color_biomes,
            cold_sheep_color_biomes,
            placement: SpawnPlacement::default(),
            spawn_costs: BTreeMap::new(),
        }
    }

    pub fn biomes(&self) -> impl Iterator<Item = &Identifier> {
        self.by_biome.keys()
    }

    /// Entries authored for this exact biome; never inherit a default biome.
    #[must_use]
    pub fn exact_entries(&self, biome: &Identifier, group: &str) -> &[BiomeSpawnEntry] {
        self.by_biome
            .get(biome)
            .and_then(|groups| groups.get(group))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    #[must_use]
    pub fn entries(&self, biome: &Identifier, group: &str) -> &[BiomeSpawnEntry] {
        if let Some(entries) = self
            .by_biome
            .get(biome)
            .and_then(|groups| groups.get(group))
            .map(Vec::as_slice)
        {
            return entries;
        }
        self.default_biome_for(biome, group)
            .and_then(|fallback| self.by_biome.get(fallback))
            .and_then(|groups| groups.get(group))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_biome.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_biome.is_empty()
    }

    #[must_use]
    pub fn sheep_color_climate(&self, biome: &Identifier) -> SheepColorClimate {
        if self.warm_sheep_color_biomes.contains(biome) {
            SheepColorClimate::Warm
        } else if self.cold_sheep_color_biomes.contains(biome) {
            SheepColorClimate::Cold
        } else {
            SheepColorClimate::Temperate
        }
    }

    #[must_use]
    fn with_default_biomes(mut self, land: Option<Identifier>, water: Option<Identifier>) -> Self {
        self.default_land_biome = land;
        self.default_water_biome = water;
        self
    }

    fn default_biome_for(&self, biome: &Identifier, group: &str) -> Option<&Identifier> {
        let water_group = matches!(group, "water_ambient" | "water_creature");
        if water_group && water_biome_name(biome.path()) {
            return self.default_water_biome.as_ref();
        }
        let land_group = matches!(group, "creature" | "monster");
        if land_group && !water_biome_name(biome.path()) {
            return self.default_land_biome.as_ref();
        }
        None
    }
}

/// Repo-owned spawn baseline used when biome JSON is absent.
#[must_use]
pub fn solaris_required_biome_spawn_rules() -> BiomeSpawnRules {
    let raw: BTreeMap<String, BTreeMap<String, Vec<RawSpawnEntry>>> =
        serde_json::from_str(REQUIRED_BIOME_SPAWNS)
            .expect("embedded required biome spawn JSON is valid");
    let mut by_biome = BTreeMap::new();
    for (biome, groups) in raw {
        let mut parsed_groups = BTreeMap::new();
        for (group, entries) in groups {
            parsed_groups.insert(
                group,
                entries
                    .into_iter()
                    .map(|entry| BiomeSpawnEntry {
                        entity_type: Identifier::parse(entry.entity_type)
                            .expect("embedded required biome spawn entity id is valid"),
                        min_count: entry.min_count,
                        max_count: entry.max_count,
                        weight: entry.weight,
                    })
                    .collect(),
            );
        }
        by_biome.insert(
            Identifier::parse(biome).expect("embedded required biome id is valid"),
            parsed_groups,
        );
    }
    let mut rules = BiomeSpawnRules::from_entries_with_sheep_color_climates(
        by_biome,
        parse_biome_set(WARM_SHEEP_COLOR_BIOMES),
        parse_biome_set(COLD_SHEEP_COLOR_BIOMES),
    )
    .with_default_biomes(
        Some(Identifier::parse("minecraft:plains").expect("embedded plains id is valid")),
        Some(Identifier::parse("minecraft:ocean").expect("embedded ocean id is valid")),
    );
    rules.spawn_costs = spawn_costs::embedded();
    rules
}

fn parse_biome_set(names: &[&str]) -> BTreeSet<Identifier> {
    names
        .iter()
        .map(|name| Identifier::parse(*name).expect("embedded biome id is valid"))
        .collect()
}

fn water_biome_name(path: &str) -> bool {
    path.contains("ocean") || path.contains("river")
}

pub fn load_biome_spawn_rules(path: impl AsRef<Path>) -> Result<BiomeSpawnRules, BiomeDataError> {
    let path = path.as_ref();
    if !path.is_dir() {
        return Err(BiomeDataError::Missing(path.to_path_buf()));
    }
    let mut entries = BTreeMap::new();
    let mut costs = BTreeMap::new();
    for file_path in sorted_json_files(path, &|path, source| BiomeDataError::Io { path, source })? {
        let stem = file_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| BiomeDataError::InvalidIdentifier(file_path.display().to_string()))?;
        let biome = Identifier::parse(format!("minecraft:{stem}"))
            .map_err(|_| BiomeDataError::InvalidIdentifier(stem.to_string()))?;
        let raw: RawBiome = read_json_file(
            &file_path,
            &|path, source| BiomeDataError::Io { path, source },
            &|path, source| BiomeDataError::Parse { path, source },
        )?;
        costs.insert(biome.clone(), spawn_costs::parse(&biome, raw.spawn_costs)?);
        let groups = raw
            .spawners
            .into_iter()
            .map(|(group, spawns)| {
                let spawns = spawns
                    .into_iter()
                    .map(|spawn| {
                        let entity_type =
                            Identifier::parse(spawn.entity_type.clone()).map_err(|_| {
                                BiomeDataError::InvalidIdentifier(spawn.entity_type.clone())
                            })?;
                        Ok(BiomeSpawnEntry {
                            entity_type,
                            min_count: spawn.min_count,
                            max_count: spawn.max_count,
                            weight: spawn.weight,
                        })
                    })
                    .collect::<Result<Vec<_>, BiomeDataError>>()?;
                Ok((group, spawns))
            })
            .collect::<Result<BTreeMap<_, _>, BiomeDataError>>()?;
        entries.insert(biome, groups);
    }
    let tags_dir = path
        .parent()
        .and_then(Path::parent)
        .map(|minecraft_dir| minecraft_dir.join("tags/worldgen/biome"));
    let tags = match tags_dir {
        Some(tags_dir) => load_biome_tags(&tags_dir)?,
        None => BTreeMap::new(),
    };
    let warm_tag = Identifier::parse("minecraft:spawns_warm_variant_farm_animals")
        .expect("static warm farm animal biome tag is valid");
    let cold_tag = Identifier::parse("minecraft:spawns_cold_variant_farm_animals")
        .expect("static cold farm animal biome tag is valid");
    let warm = tags.get(&warm_tag).into_iter().flatten().cloned().collect();
    let cold = tags.get(&cold_tag).into_iter().flatten().cloned().collect();
    let mut rules = BiomeSpawnRules::from_entries_with_sheep_color_climates(entries, warm, cold);
    rules.spawn_costs = costs;
    Ok(rules)
}

pub fn load_biome_worldgen_data(
    biome_dir: impl AsRef<Path>,
    biome_tags_dir: impl AsRef<Path>,
) -> Result<BiomeWorldgenData, BiomeDataError> {
    let biome_dir = biome_dir.as_ref();
    if !biome_dir.is_dir() {
        return Err(BiomeDataError::Missing(biome_dir.to_path_buf()));
    }

    let mut features_by_biome = BTreeMap::new();
    for file_path in sorted_json_files(biome_dir, &|path, source| BiomeDataError::Io {
        path,
        source,
    })? {
        let biome = id_from_file(&file_path)?;
        let raw: RawBiome = read_json_file(
            &file_path,
            &|path, source| BiomeDataError::Io { path, source },
            &|path, source| BiomeDataError::Parse { path, source },
        )?;
        let features = raw
            .features
            .into_iter()
            .flatten()
            .map(|feature| {
                Identifier::parse(feature.clone())
                    .map_err(|_| BiomeDataError::InvalidIdentifier(feature))
            })
            .collect::<Result<Vec<_>, _>>()?;
        features_by_biome.insert(biome, features);
    }

    let tags = load_biome_tags(biome_tags_dir.as_ref())?;
    Ok(BiomeWorldgenData {
        features_by_biome,
        tags,
    })
}

#[derive(Deserialize)]
struct RawBiome {
    #[serde(default)]
    spawners: BTreeMap<String, Vec<RawSpawnEntry>>,
    #[serde(default)]
    features: Vec<Vec<String>>,
    #[serde(default)]
    spawn_costs: BTreeMap<String, BiomeSpawnCost>,
}

#[derive(Deserialize)]
struct RawBiomeTag {
    #[serde(default)]
    values: Vec<RawBiomeTagValue>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawBiomeTagValue {
    Plain(String),
    Object {
        id: String,
        #[serde(default = "default_required")]
        required: bool,
    },
}

fn default_required() -> bool {
    true
}

fn id_from_file(path: &Path) -> Result<Identifier, BiomeDataError> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| BiomeDataError::InvalidIdentifier(path.display().to_string()))?;
    Identifier::parse(format!("minecraft:{stem}"))
        .map_err(|_| BiomeDataError::InvalidIdentifier(stem.to_string()))
}

fn load_biome_tags(path: &Path) -> Result<BTreeMap<Identifier, Vec<Identifier>>, BiomeDataError> {
    if !path.is_dir() {
        return Ok(BTreeMap::new());
    }
    let mut raw = BTreeMap::new();
    collect_tag_files(path, &mut raw)?;

    let mut resolved = BTreeMap::new();
    for tag_path in raw.keys() {
        let mut visiting = BTreeSet::new();
        let mut values = BTreeSet::new();
        resolve_tag(tag_path, &raw, &mut visiting, &mut values);
        let tag = Identifier::parse(format!("minecraft:{tag_path}"))
            .map_err(|_| BiomeDataError::InvalidIdentifier(tag_path.clone()))?;
        resolved.insert(tag, values.into_iter().collect());
    }
    Ok(resolved)
}

fn collect_tag_files(
    root: &Path,
    raw: &mut BTreeMap<String, (PathBuf, RawBiomeTag)>,
) -> Result<(), BiomeDataError> {
    visit_json_files(
        root,
        &mut |path| {
            let rel = path
                .strip_prefix(root)
                .expect("walk yields paths under root")
                .with_extension("");
            let joined = rel
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let parsed: RawBiomeTag = read_json_file(
                &path,
                &|path, source| BiomeDataError::Io { path, source },
                &|path, source| BiomeDataError::TagParse { path, source },
            )?;
            raw.insert(joined, (path, parsed));
            Ok(())
        },
        &|path, source| BiomeDataError::Io { path, source },
    )
}

fn resolve_tag(
    tag_path: &str,
    raw: &BTreeMap<String, (PathBuf, RawBiomeTag)>,
    visiting: &mut BTreeSet<String>,
    values: &mut BTreeSet<Identifier>,
) {
    if !visiting.insert(tag_path.to_string()) {
        return;
    }
    let Some((_, tag)) = raw.get(tag_path) else {
        visiting.remove(tag_path);
        return;
    };
    for value in &tag.values {
        let (id, required) = match value {
            RawBiomeTagValue::Plain(id) => (id.as_str(), true),
            RawBiomeTagValue::Object { id, required } => (id.as_str(), *required),
        };
        if let Some(inner) = id.strip_prefix('#') {
            let inner = inner
                .strip_prefix("minecraft:")
                .unwrap_or_else(|| inner.split_once(':').map_or(inner, |(_, path)| path));
            resolve_tag(inner, raw, visiting, values);
        } else if let Ok(id) = Identifier::parse(id.to_string()) {
            values.insert(id);
        } else if required {
            values.clear();
        }
    }
    visiting.remove(tag_path);
}

#[derive(Deserialize)]
struct RawSpawnEntry {
    #[serde(rename = "type")]
    entity_type: String,
    #[serde(rename = "minCount")]
    min_count: u32,
    #[serde(rename = "maxCount")]
    max_count: u32,
    weight: u32,
}

#[cfg(test)]
#[path = "biomes_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "biomes_dimension_tests.rs"]
mod dimension_tests;
