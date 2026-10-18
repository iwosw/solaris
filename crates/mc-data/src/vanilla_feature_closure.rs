//! Vanilla placed-feature closure reader.
//!
//! This resolves vanilla worldgen JSON **by reference**, starting from entries
//! the caller names (a placed feature id, or the `feature_pool_element` entries
//! of a structure template pool) and following `placed_feature` →
//! `configured_feature` → state providers, placement modifiers, int providers
//! and block predicates down to their leaves. It loads only that closure:
//! entries that are not transitively reachable are never read, so the parts of
//! the vanilla registries Solaris does not implement can never fail a load.
//!
//! Anything the closure *does* reach that Solaris does not implement fails
//! closed ([`ClosureError::UnsupportedType`]), naming the type id and the entry
//! that referenced it. The reader executes nothing; the typed specs it returns
//! are compiled and placed by `mc-worldgen`.
//!
//! ## Checkpoint A1 partial layer
//!
//! The implemented surface is exactly the reachable village decor closure
//! covered so far:
//!
//! - configured features `minecraft:simple_block`, `minecraft:block_pile`,
//!   `minecraft:block_column`, `minecraft:tree` (checkpoint A2: the four
//!   reachable trunk/foliage placer pairs, an empty decorator list, and the
//!   `two_layers_feature_size` minimum size),
//! - state providers `simple`, `weighted`, `rotated`, `noise_threshold`,
//!   `rule_based`.
//! - placement modifiers `count`, `random_offset`, `block_predicate_filter`.
//! - block predicates `would_survive`, `matching_block_tag`, `matching_blocks`,
//!   `all_of`, `not`.
//! - int providers `constant`, `uniform`, `trapezoid`, `biased_to_bottom`,
//!   `weighted_list`.
//!
//! Nothing outside that surface is accepted, and nothing here runs on an
//! ordinary server start: this module is exercised by tests and will be used by
//! checkpoint B when village assembly is wired.

use std::path::PathBuf;

use serde_json::Value;
use thiserror::Error;

use crate::{Identifier, ResourcePathError, read_json_resource};

/// How a reference is named in error messages, e.g. "configured feature".
type EntryKind = &'static str;

const PLACED_FEATURE: EntryKind = "placed_feature";
const CONFIGURED_FEATURE: EntryKind = "configured_feature";
const TEMPLATE_POOL: EntryKind = "template_pool";
const STATE_PROVIDER: EntryKind = "state provider";
const PLACEMENT_MODIFIER: EntryKind = "placement modifier";
const BLOCK_PREDICATE: EntryKind = "block predicate";
const INT_PROVIDER: EntryKind = "int provider";
const TRUNK_PLACER: EntryKind = "trunk placer";
const FOLIAGE_PLACER: EntryKind = "foliage placer";
const FEATURE_SIZE: EntryKind = "feature size";
const TREE_DECORATOR: EntryKind = "tree decorator";
const ROOT_PLACER: EntryKind = "root placer";
const POOL_ELEMENT: EntryKind = "template pool element";

#[derive(Debug, Error)]
pub enum ClosureError {
    #[error("feature closure io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("feature closure parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error(transparent)]
    ResourcePath(#[from] ResourcePathError),
    #[error("missing {kind} {id} referenced by {referrer} at {path}")]
    MissingEntry {
        kind: EntryKind,
        id: Identifier,
        referrer: Identifier,
        path: PathBuf,
    },
    #[error("non-minecraft {kind} {id} referenced by {referrer} is outside the vanilla closure")]
    UnsupportedNamespace {
        kind: EntryKind,
        id: Identifier,
        referrer: Identifier,
    },
    #[error("unsupported {kind} type {type_id} referenced by {referrer}")]
    UnsupportedType {
        kind: EntryKind,
        type_id: Identifier,
        referrer: Identifier,
    },
    #[error("{kind} {entry} is missing field {field}")]
    MissingField {
        kind: EntryKind,
        entry: Identifier,
        field: &'static str,
    },
    #[error("{kind} {entry} has invalid value for {field}: {value}")]
    InvalidField {
        kind: EntryKind,
        entry: Identifier,
        field: &'static str,
        value: String,
    },
}

/// A block state as written in worldgen JSON: block id plus explicit property
/// values. Resolution to a registry state id happens in `mc-worldgen`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockStateSpec {
    pub block: Identifier,
    pub properties: Vec<(String, String)>,
}

/// A placed feature: its configured feature plus the placement pipeline, in
/// application order.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedFeatureSpec {
    pub id: Identifier,
    pub feature: ConfiguredFeatureSpec,
    pub placement: Vec<PlacementModifierSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguredFeatureSpec {
    pub id: Identifier,
    pub kind: ConfiguredFeatureKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfiguredFeatureKind {
    SimpleBlock {
        to_place: StateProviderSpec,
        schedule_tick: bool,
    },
    BlockPile {
        state_provider: StateProviderSpec,
    },
    BlockColumn {
        layers: Vec<BlockColumnLayer>,
        direction: ColumnDirection,
        allowed_placement: BlockPredicateSpec,
        prioritize_tip: bool,
    },
    Tree(Box<TreeSpec>),
}

/// `minecraft:tree` configuration, restricted to what the village closure
/// reaches.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeSpec {
    pub trunk_provider: StateProviderSpec,
    pub trunk_placer: TrunkPlacerSpec,
    pub foliage_provider: StateProviderSpec,
    pub foliage_placer: FoliagePlacerSpec,
    pub minimum_size: TwoLayersFeatureSize,
    /// `Codec.BOOL.fieldOf("ignore_vines").orElse(false)`.
    pub ignore_vines: bool,
    /// `below_trunk_provider`, defaulting to vanilla's
    /// `PLACE_BELOW_OVERWORLD_TRUNKS`.
    pub below_trunk_provider: StateProviderSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrunkPlacerSpec {
    Straight(TrunkPlacerHeights),
    Forking(TrunkPlacerHeights),
}

/// `base_height`/`height_rand_a`/`height_rand_b`, the shared trunk placer parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrunkPlacerHeights {
    pub base_height: i32,
    pub height_rand_a: i32,
    pub height_rand_b: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FoliagePlacerSpec {
    Blob {
        radius: IntProviderSpec,
        offset: IntProviderSpec,
        height: i32,
    },
    Spruce {
        radius: IntProviderSpec,
        offset: IntProviderSpec,
        trunk_height: IntProviderSpec,
    },
    Pine {
        radius: IntProviderSpec,
        offset: IntProviderSpec,
        height: IntProviderSpec,
    },
    Acacia {
        radius: IntProviderSpec,
        offset: IntProviderSpec,
    },
}

/// `minecraft:two_layers_feature_size` with its codec defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TwoLayersFeatureSize {
    pub limit: i32,
    pub lower_size: i32,
    pub upper_size: i32,
    pub min_clipped_height: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockColumnLayer {
    pub height: IntProviderSpec,
    pub provider: StateProviderSpec,
}

/// `minecraft:block_column` direction. Only `up` is reachable from the village
/// decor closure today; the whole direction enum is modelled so the field
/// resolves from its own domain instead of failing on an unrelated value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnDirection {
    Up,
    Down,
    North,
    South,
    West,
    East,
}

impl ColumnDirection {
    #[must_use]
    pub fn offset(self) -> (i32, i32, i32) {
        match self {
            Self::Up => (0, 1, 0),
            Self::Down => (0, -1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "up" => Self::Up,
            "down" => Self::Down,
            "north" => Self::North,
            "south" => Self::South,
            "west" => Self::West,
            "east" => Self::East,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateProviderSpec {
    Simple {
        state: BlockStateSpec,
    },
    Weighted {
        entries: Vec<WeightedState>,
    },
    /// `minecraft:rotated_block_provider` keeps only the block: its codec maps
    /// the written state through `Block`, so written properties are dropped and
    /// the axis is drawn per placement.
    Rotated {
        block: Identifier,
    },
    NoiseThreshold(NoiseThresholdSpec),
    RuleBased {
        fallback: Option<Box<StateProviderSpec>>,
        rules: Vec<RuleBasedRule>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeightedState {
    pub state: BlockStateSpec,
    pub weight: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoiseThresholdSpec {
    pub seed: i64,
    pub noise: NoiseParametersSpec,
    pub scale: f32,
    pub threshold: f32,
    pub high_chance: f32,
    pub default_state: BlockStateSpec,
    pub low_states: Vec<BlockStateSpec>,
    pub high_states: Vec<BlockStateSpec>,
}

/// `minecraft:noise` parameters as referenced by `noise_threshold_provider`.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseParametersSpec {
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleBasedRule {
    pub predicate: BlockPredicateSpec,
    pub then: StateProviderSpec,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlacementModifierSpec {
    Count(IntProviderSpec),
    RandomOffset {
        xz_spread: IntProviderSpec,
        y_spread: IntProviderSpec,
    },
    BlockPredicateFilter(BlockPredicateSpec),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IntProviderSpec {
    Constant(i32),
    Uniform {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    Trapezoid {
        min_inclusive: i32,
        max_inclusive: i32,
        plateau: i32,
    },
    BiasedToBottom {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    WeightedList(Vec<WeightedInt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeightedInt {
    pub provider: IntProviderSpec,
    pub weight: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPredicateSpec {
    WouldSurvive {
        offset: (i32, i32, i32),
        state: BlockStateSpec,
    },
    MatchingBlockTag {
        offset: (i32, i32, i32),
        tag: Identifier,
    },
    MatchingBlocks {
        offset: (i32, i32, i32),
        blocks: Vec<Identifier>,
    },
    AllOf {
        predicates: Vec<BlockPredicateSpec>,
    },
    Not {
        predicate: Box<BlockPredicateSpec>,
    },
}

/// One `feature_pool_element` of a structure template pool, resolved to its
/// placed feature.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolFeatureEntry {
    pub pool: Identifier,
    pub placed_feature: Identifier,
    pub weight: u32,
    pub spec: PlacedFeatureSpec,
}

/// Resolve-by-reference reader over a vanilla `worldgen` directory, e.g.
/// `<vanilla_data_dir>/data/minecraft/worldgen`.
pub struct FeatureClosure {
    worldgen_dir: PathBuf,
}

impl FeatureClosure {
    #[must_use]
    pub fn new(worldgen_dir: impl Into<PathBuf>) -> Self {
        Self {
            worldgen_dir: worldgen_dir.into(),
        }
    }

    /// Resolve one placed feature and everything it references.
    pub fn load_placed_feature(&self, id: &Identifier) -> Result<PlacedFeatureSpec, ClosureError> {
        self.load_placed_feature_from(id, id)
    }

    fn load_placed_feature_from(
        &self,
        id: &Identifier,
        referrer: &Identifier,
    ) -> Result<PlacedFeatureSpec, ClosureError> {
        let value = self.read_entry(PLACED_FEATURE, "placed_feature", id, referrer)?;
        let configured_id = parse_id(
            self.path(PLACED_FEATURE, id),
            field_str(&value, "feature", PLACED_FEATURE, id)?,
        )?;
        let configured_value =
            self.read_entry(CONFIGURED_FEATURE, "configured_feature", &configured_id, id)?;
        let feature = self.parse_configured_feature(&configured_id, &configured_value)?;
        let placement = match value.get("placement") {
            None => Vec::new(),
            Some(Value::Array(entries)) => entries
                .iter()
                .map(|entry| self.parse_placement_modifier(id, entry))
                .collect::<Result<Vec<_>, _>>()?,
            Some(other) => {
                return Err(ClosureError::InvalidField {
                    kind: PLACED_FEATURE,
                    entry: id.clone(),
                    field: "placement",
                    value: other.to_string(),
                });
            }
        };
        Ok(PlacedFeatureSpec {
            id: id.clone(),
            feature,
            placement,
        })
    }

    /// Resolve every `feature_pool_element` of a structure template pool.
    ///
    /// Elements that are not placed-feature elements (empty/legacy/single pool
    /// elements) belong to jigsaw assembly rather than to the feature closure
    /// and are skipped. A missing pool or feature file, an unreadable element or
    /// an unsupported reachable type fails closed.
    pub fn load_pool_features(
        &self,
        pool: &Identifier,
    ) -> Result<Vec<PoolFeatureEntry>, ClosureError> {
        let value = self.read_entry(TEMPLATE_POOL, "template_pool", pool, pool)?;
        let elements = field_array(&value, "elements", TEMPLATE_POOL, pool)?;

        let mut entries = Vec::new();
        for entry in elements {
            let element = field_of(entry, "element", TEMPLATE_POOL, pool)?;
            // Vanilla's pool codec requires `weight` in 1..=150 for every element.
            let weight = u32::try_from(i32_field(entry, "weight", TEMPLATE_POOL, pool)?)
                .ok()
                .filter(|weight| (1..=150).contains(weight))
                .ok_or_else(|| ClosureError::InvalidField {
                    kind: TEMPLATE_POOL,
                    entry: pool.clone(),
                    field: "weight",
                    value: entry
                        .get("weight")
                        .map(Value::to_string)
                        .unwrap_or_else(|| String::from("<missing>")),
                })?;
            match field_str(element, "element_type", TEMPLATE_POOL, pool)? {
                "minecraft:feature_pool_element" => {}
                // Jigsaw elements are assembly's business, not the feature
                // closure's.
                "minecraft:empty_pool_element"
                | "minecraft:single_pool_element"
                | "minecraft:legacy_single_pool_element" => continue,
                // A list element can wrap feature elements, so skipping it
                // would silently drop closure entries.
                other => {
                    return Err(ClosureError::UnsupportedType {
                        kind: POOL_ELEMENT,
                        type_id: parse_id(self.path(TEMPLATE_POOL, pool), other)?,
                        referrer: pool.clone(),
                    });
                }
            }
            let placed_feature = parse_id(
                self.path(TEMPLATE_POOL, pool),
                field_str(element, "feature", TEMPLATE_POOL, pool)?,
            )?;
            let spec = self.load_placed_feature_from(&placed_feature, pool)?;
            entries.push(PoolFeatureEntry {
                pool: pool.clone(),
                placed_feature,
                weight,
                spec,
            });
        }
        Ok(entries)
    }

    fn path(&self, dir: &str, id: &Identifier) -> PathBuf {
        self.worldgen_dir.join(dir).join(id.path())
    }

    fn read_entry(
        &self,
        kind: EntryKind,
        dir: &str,
        id: &Identifier,
        referrer: &Identifier,
    ) -> Result<Value, ClosureError> {
        if id.namespace() != "minecraft" {
            return Err(ClosureError::UnsupportedNamespace {
                kind,
                id: id.clone(),
                referrer: referrer.clone(),
            });
        }
        let io_error = |path, source| ClosureError::Io { path, source };
        let parse_error = |path, source| ClosureError::Parse { path, source };
        match crate::open_worldgen_entry(&self.worldgen_dir, dir, id)? {
            crate::WorldgenEntry::Present(opened) => {
                read_json_resource(opened, &io_error, &parse_error)
            }
            crate::WorldgenEntry::Missing(path) => Err(ClosureError::MissingEntry {
                kind,
                id: id.clone(),
                referrer: referrer.clone(),
                path,
            }),
        }
    }

    fn parse_configured_feature(
        &self,
        id: &Identifier,
        value: &Value,
    ) -> Result<ConfiguredFeatureSpec, ClosureError> {
        let type_id = parse_id(
            self.path(CONFIGURED_FEATURE, id),
            field_str(value, "type", CONFIGURED_FEATURE, id)?,
        )?;
        let config = field_of(value, "config", CONFIGURED_FEATURE, id)?;
        let kind = match type_id.path() {
            "simple_block" => ConfiguredFeatureKind::SimpleBlock {
                to_place: self.parse_state_provider(
                    id,
                    field_of(config, "to_place", CONFIGURED_FEATURE, id)?,
                )?,
                schedule_tick: config
                    .get("schedule_tick")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            "block_pile" => ConfiguredFeatureKind::BlockPile {
                state_provider: self.parse_state_provider(
                    id,
                    field_of(config, "state_provider", CONFIGURED_FEATURE, id)?,
                )?,
            },
            "block_column" => {
                let layers = field_array(config, "layers", CONFIGURED_FEATURE, id)?
                    .iter()
                    .map(|layer| {
                        Ok(BlockColumnLayer {
                            height: self.parse_int_provider(
                                id,
                                field_of(layer, "height", CONFIGURED_FEATURE, id)?,
                            )?,
                            provider: self.parse_state_provider(
                                id,
                                field_of(layer, "provider", CONFIGURED_FEATURE, id)?,
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>, ClosureError>>()?;
                let direction_name = field_str(config, "direction", CONFIGURED_FEATURE, id)?;
                let direction = ColumnDirection::parse(direction_name).ok_or_else(|| {
                    ClosureError::InvalidField {
                        kind: CONFIGURED_FEATURE,
                        entry: id.clone(),
                        field: "direction",
                        value: direction_name.to_owned(),
                    }
                })?;
                ConfiguredFeatureKind::BlockColumn {
                    layers,
                    direction,
                    allowed_placement: self.parse_block_predicate(
                        id,
                        field_of(config, "allowed_placement", CONFIGURED_FEATURE, id)?,
                    )?,
                    prioritize_tip: field_of(config, "prioritize_tip", CONFIGURED_FEATURE, id)?
                        .as_bool()
                        .ok_or_else(|| ClosureError::InvalidField {
                            kind: CONFIGURED_FEATURE,
                            entry: id.clone(),
                            field: "prioritize_tip",
                            value: "not a boolean".to_owned(),
                        })?,
                }
            }
            "tree" => ConfiguredFeatureKind::Tree(Box::new(self.parse_tree(id, config)?)),
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: CONFIGURED_FEATURE,
                    type_id,
                    referrer: id.clone(),
                });
            }
        };
        Ok(ConfiguredFeatureSpec {
            id: id.clone(),
            kind,
        })
    }

    /// `minecraft:tree`. The village closure reaches four trunk/foliage placer
    /// pairs, an empty decorator list and a `two_layers_feature_size` minimum
    /// size; every other placer, size, decorator or root placer fails closed.
    fn parse_tree(&self, id: &Identifier, config: &Value) -> Result<TreeSpec, ClosureError> {
        if let Some(root_placer) = config.get("root_placer") {
            return Err(ClosureError::UnsupportedType {
                kind: ROOT_PLACER,
                type_id: parse_id(
                    self.path(ROOT_PLACER, id),
                    field_str(root_placer, "type", ROOT_PLACER, id)?,
                )?,
                referrer: id.clone(),
            });
        }
        let decorators = field_array(config, "decorators", CONFIGURED_FEATURE, id)?;
        if let Some(decorator) = decorators.first() {
            return Err(ClosureError::UnsupportedType {
                kind: TREE_DECORATOR,
                type_id: parse_id(
                    self.path(TREE_DECORATOR, id),
                    field_str(decorator, "type", TREE_DECORATOR, id)?,
                )?,
                referrer: id.clone(),
            });
        }

        let trunk_placer = field_of(config, "trunk_placer", CONFIGURED_FEATURE, id)?;
        let trunk_type = parse_id(
            self.path(TRUNK_PLACER, id),
            field_str(trunk_placer, "type", TRUNK_PLACER, id)?,
        )?;
        let heights = TrunkPlacerHeights {
            base_height: i32_field(trunk_placer, "base_height", TRUNK_PLACER, id)?,
            height_rand_a: i32_field(trunk_placer, "height_rand_a", TRUNK_PLACER, id)?,
            height_rand_b: i32_field(trunk_placer, "height_rand_b", TRUNK_PLACER, id)?,
        };
        let trunk_placer = match trunk_type.path() {
            "straight_trunk_placer" => TrunkPlacerSpec::Straight(heights),
            "forking_trunk_placer" => TrunkPlacerSpec::Forking(heights),
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: TRUNK_PLACER,
                    type_id: trunk_type,
                    referrer: id.clone(),
                });
            }
        };

        let foliage_placer = field_of(config, "foliage_placer", CONFIGURED_FEATURE, id)?;
        let foliage_type = parse_id(
            self.path(FOLIAGE_PLACER, id),
            field_str(foliage_placer, "type", FOLIAGE_PLACER, id)?,
        )?;
        let radius =
            self.parse_int_provider(id, field_of(foliage_placer, "radius", FOLIAGE_PLACER, id)?)?;
        let offset =
            self.parse_int_provider(id, field_of(foliage_placer, "offset", FOLIAGE_PLACER, id)?)?;
        let foliage_placer = match foliage_type.path() {
            "blob_foliage_placer" => FoliagePlacerSpec::Blob {
                radius,
                offset,
                height: i32_field(foliage_placer, "height", FOLIAGE_PLACER, id)?,
            },
            "spruce_foliage_placer" => FoliagePlacerSpec::Spruce {
                radius,
                offset,
                trunk_height: self.parse_int_provider(
                    id,
                    field_of(foliage_placer, "trunk_height", FOLIAGE_PLACER, id)?,
                )?,
            },
            "pine_foliage_placer" => FoliagePlacerSpec::Pine {
                radius,
                offset,
                height: self.parse_int_provider(
                    id,
                    field_of(foliage_placer, "height", FOLIAGE_PLACER, id)?,
                )?,
            },
            "acacia_foliage_placer" => FoliagePlacerSpec::Acacia { radius, offset },
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: FOLIAGE_PLACER,
                    type_id: foliage_type,
                    referrer: id.clone(),
                });
            }
        };

        let minimum_size = field_of(config, "minimum_size", CONFIGURED_FEATURE, id)?;
        let size_type = parse_id(
            self.path(FEATURE_SIZE, id),
            field_str(minimum_size, "type", FEATURE_SIZE, id)?,
        )?;
        if size_type.path() != "two_layers_feature_size" {
            return Err(ClosureError::UnsupportedType {
                kind: FEATURE_SIZE,
                type_id: size_type,
                referrer: id.clone(),
            });
        }
        let optional_i32 = |field: &'static str| -> Result<Option<i32>, ClosureError> {
            match minimum_size.get(field) {
                None => Ok(None),
                Some(_) => Ok(Some(i32_field(minimum_size, field, FEATURE_SIZE, id)?)),
            }
        };

        Ok(TreeSpec {
            trunk_provider: self.parse_state_provider(
                id,
                field_of(config, "trunk_provider", CONFIGURED_FEATURE, id)?,
            )?,
            trunk_placer,
            foliage_provider: self.parse_state_provider(
                id,
                field_of(config, "foliage_provider", CONFIGURED_FEATURE, id)?,
            )?,
            foliage_placer,
            minimum_size: TwoLayersFeatureSize {
                limit: optional_i32("limit")?.unwrap_or(1),
                lower_size: optional_i32("lower_size")?.unwrap_or(0),
                upper_size: optional_i32("upper_size")?.unwrap_or(1),
                min_clipped_height: optional_i32("min_clipped_height")?,
            },
            ignore_vines: config
                .get("ignore_vines")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            below_trunk_provider: match config.get("below_trunk_provider") {
                Some(provider) => self.parse_state_provider(id, provider)?,
                None => below_overworld_trunks_provider(),
            },
        })
    }

    fn parse_state_provider(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<StateProviderSpec, ClosureError> {
        let type_id = parse_id(
            self.path(STATE_PROVIDER, referrer),
            field_str(value, "type", STATE_PROVIDER, referrer)?,
        )?;
        Ok(match type_id.path() {
            "simple_state_provider" => StateProviderSpec::Simple {
                state: self.parse_block_state(
                    referrer,
                    field_of(value, "state", STATE_PROVIDER, referrer)?,
                )?,
            },
            "weighted_state_provider" => StateProviderSpec::Weighted {
                entries: {
                    let entries = field_array(value, "entries", STATE_PROVIDER, referrer)?;
                    let mut weighted = Vec::with_capacity(entries.len());
                    for entry in entries {
                        weighted.push(WeightedState {
                            state: self.parse_block_state(
                                referrer,
                                field_of(entry, "data", STATE_PROVIDER, referrer)?,
                            )?,
                            weight: i32_field(entry, "weight", STATE_PROVIDER, referrer)?,
                        });
                    }
                    weighted
                },
            },
            "rotated_block_provider" => {
                let state = self.parse_block_state(
                    referrer,
                    field_of(value, "state", STATE_PROVIDER, referrer)?,
                )?;
                StateProviderSpec::Rotated { block: state.block }
            }
            "noise_threshold_provider" => StateProviderSpec::NoiseThreshold(NoiseThresholdSpec {
                seed: i64_field(value, "seed", STATE_PROVIDER, referrer)?,
                noise: parse_noise_parameters(
                    field_of(value, "noise", STATE_PROVIDER, referrer)?,
                    referrer,
                )?,
                scale: f32_field(value, "scale", STATE_PROVIDER, referrer)?,
                threshold: f32_field(value, "threshold", STATE_PROVIDER, referrer)?,
                high_chance: f32_field(value, "high_chance", STATE_PROVIDER, referrer)?,
                default_state: self.parse_block_state(
                    referrer,
                    field_of(value, "default_state", STATE_PROVIDER, referrer)?,
                )?,
                low_states: self.parse_block_state_list(referrer, value, "low_states")?,
                high_states: self.parse_block_state_list(referrer, value, "high_states")?,
            }),
            "rule_based_state_provider" => {
                let fallback = value
                    .get("fallback")
                    .map(|fallback| self.parse_state_provider(referrer, fallback))
                    .transpose()?
                    .map(Box::new);
                let rules = field_array(value, "rules", STATE_PROVIDER, referrer)?
                    .iter()
                    .map(|rule| {
                        Ok(RuleBasedRule {
                            predicate: self.parse_block_predicate(
                                referrer,
                                field_of(rule, "if_true", STATE_PROVIDER, referrer)?,
                            )?,
                            then: self.parse_state_provider(
                                referrer,
                                field_of(rule, "then", STATE_PROVIDER, referrer)?,
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>, ClosureError>>()?;
                StateProviderSpec::RuleBased { fallback, rules }
            }
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: STATE_PROVIDER,
                    type_id,
                    referrer: referrer.clone(),
                });
            }
        })
    }

    fn parse_placement_modifier(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<PlacementModifierSpec, ClosureError> {
        let type_id = parse_id(
            self.path(PLACEMENT_MODIFIER, referrer),
            field_str(value, "type", PLACEMENT_MODIFIER, referrer)?,
        )?;
        Ok(match type_id.path() {
            "count" => PlacementModifierSpec::Count(self.parse_int_provider(
                referrer,
                field_of(value, "count", PLACEMENT_MODIFIER, referrer)?,
            )?),
            "random_offset" => PlacementModifierSpec::RandomOffset {
                xz_spread: self.parse_int_provider(
                    referrer,
                    field_of(value, "xz_spread", PLACEMENT_MODIFIER, referrer)?,
                )?,
                y_spread: self.parse_int_provider(
                    referrer,
                    field_of(value, "y_spread", PLACEMENT_MODIFIER, referrer)?,
                )?,
            },
            "block_predicate_filter" => {
                PlacementModifierSpec::BlockPredicateFilter(self.parse_block_predicate(
                    referrer,
                    field_of(value, "predicate", PLACEMENT_MODIFIER, referrer)?,
                )?)
            }
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: PLACEMENT_MODIFIER,
                    type_id,
                    referrer: referrer.clone(),
                });
            }
        })
    }

    fn parse_int_provider(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<IntProviderSpec, ClosureError> {
        if let Some(constant) = value.as_i64() {
            return Ok(IntProviderSpec::Constant(i32::try_from(constant).map_err(
                |_| ClosureError::InvalidField {
                    kind: INT_PROVIDER,
                    entry: referrer.clone(),
                    field: "value",
                    value: constant.to_string(),
                },
            )?));
        }
        let type_id = parse_id(
            self.path(INT_PROVIDER, referrer),
            field_str(value, "type", INT_PROVIDER, referrer)?,
        )?;
        Ok(match type_id.path() {
            "constant" => {
                IntProviderSpec::Constant(i32_field(value, "value", INT_PROVIDER, referrer)?)
            }
            "uniform" => {
                let min_inclusive = i32_field(value, "min_inclusive", INT_PROVIDER, referrer)?;
                let max_inclusive = i32_field(value, "max_inclusive", INT_PROVIDER, referrer)?;
                if max_inclusive < min_inclusive {
                    return Err(ClosureError::InvalidField {
                        kind: INT_PROVIDER,
                        entry: referrer.clone(),
                        field: "max_inclusive",
                        value: format!("{max_inclusive} < {min_inclusive}"),
                    });
                }
                IntProviderSpec::Uniform {
                    min_inclusive,
                    max_inclusive,
                }
            }
            "trapezoid" => IntProviderSpec::Trapezoid {
                min_inclusive: i32_field(value, "min", INT_PROVIDER, referrer)?,
                max_inclusive: i32_field(value, "max", INT_PROVIDER, referrer)?,
                plateau: i32_field(value, "plateau", INT_PROVIDER, referrer)?,
            },
            "biased_to_bottom" => IntProviderSpec::BiasedToBottom {
                min_inclusive: i32_field(value, "min_inclusive", INT_PROVIDER, referrer)?,
                max_inclusive: i32_field(value, "max_inclusive", INT_PROVIDER, referrer)?,
            },
            "weighted_list" => IntProviderSpec::WeightedList({
                let distribution = field_array(value, "distribution", INT_PROVIDER, referrer)?;
                let mut entries = Vec::with_capacity(distribution.len());
                for entry in distribution {
                    entries.push(WeightedInt {
                        provider: self.parse_int_provider(
                            referrer,
                            field_of(entry, "data", INT_PROVIDER, referrer)?,
                        )?,
                        weight: i32_field(entry, "weight", INT_PROVIDER, referrer)?,
                    });
                }
                entries
            }),
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: INT_PROVIDER,
                    type_id,
                    referrer: referrer.clone(),
                });
            }
        })
    }

    fn parse_block_predicate(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<BlockPredicateSpec, ClosureError> {
        let type_id = parse_id(
            self.path(BLOCK_PREDICATE, referrer),
            field_str(value, "type", BLOCK_PREDICATE, referrer)?,
        )?;
        let offset = parse_offset(value, BLOCK_PREDICATE, referrer)?;
        Ok(match type_id.path() {
            "would_survive" => BlockPredicateSpec::WouldSurvive {
                offset,
                state: self.parse_block_state(
                    referrer,
                    field_of(value, "state", BLOCK_PREDICATE, referrer)?,
                )?,
            },
            "matching_block_tag" => BlockPredicateSpec::MatchingBlockTag {
                offset,
                tag: parse_id(
                    self.path(BLOCK_PREDICATE, referrer),
                    field_str(value, "tag", BLOCK_PREDICATE, referrer)?,
                )?,
            },
            "matching_blocks" => BlockPredicateSpec::MatchingBlocks {
                offset,
                blocks: parse_id_list(
                    self.path(BLOCK_PREDICATE, referrer),
                    field_of(value, "blocks", BLOCK_PREDICATE, referrer)?,
                )?,
            },
            "all_of" => BlockPredicateSpec::AllOf {
                predicates: field_array(value, "predicates", BLOCK_PREDICATE, referrer)?
                    .iter()
                    .map(|predicate| self.parse_block_predicate(referrer, predicate))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            "not" => BlockPredicateSpec::Not {
                predicate: Box::new(self.parse_block_predicate(
                    referrer,
                    field_of(value, "predicate", BLOCK_PREDICATE, referrer)?,
                )?),
            },
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: BLOCK_PREDICATE,
                    type_id,
                    referrer: referrer.clone(),
                });
            }
        })
    }

    fn parse_block_state(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<BlockStateSpec, ClosureError> {
        let name = field_str(value, "Name", STATE_PROVIDER, referrer)?;
        let mut properties = Vec::new();
        if let Some(values) = value.get("Properties").and_then(Value::as_object) {
            for (key, value) in values {
                let Some(value) = value.as_str() else {
                    return Err(ClosureError::InvalidField {
                        kind: STATE_PROVIDER,
                        entry: referrer.clone(),
                        field: "Properties",
                        value: format!("{key}={value}"),
                    });
                };
                properties.push((key.clone(), value.to_owned()));
            }
        }
        Ok(BlockStateSpec {
            block: parse_id(self.path(STATE_PROVIDER, referrer), name)?,
            properties,
        })
    }

    fn parse_block_state_list(
        &self,
        referrer: &Identifier,
        parent: &Value,
        field: &'static str,
    ) -> Result<Vec<BlockStateSpec>, ClosureError> {
        field_array(parent, field, STATE_PROVIDER, referrer)?
            .iter()
            .map(|entry| self.parse_block_state(referrer, entry))
            .collect()
    }
}

fn field_of<'a>(
    value: &'a Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<&'a Value, ClosureError> {
    value.get(field).ok_or_else(|| ClosureError::MissingField {
        kind,
        entry: entry.clone(),
        field,
    })
}

fn field_array<'a>(
    value: &'a Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<&'a Vec<Value>, ClosureError> {
    field_of(value, field, kind, entry)?
        .as_array()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not an array".to_owned(),
        })
}

fn field_str<'a>(
    value: &'a Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<&'a str, ClosureError> {
    field_of(value, field, kind, entry)?
        .as_str()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not a string".to_owned(),
        })
}

fn i32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<i32, ClosureError> {
    let raw = field_of(value, field, kind, entry)?
        .as_i64()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not an integer".to_owned(),
        })?;
    i32::try_from(raw).map_err(|_| ClosureError::InvalidField {
        kind,
        entry: entry.clone(),
        field,
        value: raw.to_string(),
    })
}

fn i64_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<i64, ClosureError> {
    field_of(value, field, kind, entry)?
        .as_i64()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not an integer".to_owned(),
        })
}

fn f32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<f32, ClosureError> {
    let raw = field_of(value, field, kind, entry)?
        .as_f64()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not a number".to_owned(),
        })?;
    Ok(raw as f32)
}

fn parse_id(path: PathBuf, raw: &str) -> Result<Identifier, ClosureError> {
    Identifier::parse(raw.to_owned()).map_err(|_| ClosureError::InvalidIdentifier {
        path,
        value: raw.to_owned(),
    })
}

fn parse_id_list(path: PathBuf, value: &Value) -> Result<Vec<Identifier>, ClosureError> {
    match value {
        Value::String(single) => Ok(vec![parse_id(path, single)?]),
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                let Some(raw) = entry.as_str() else {
                    return Err(ClosureError::InvalidIdentifier {
                        path: path.clone(),
                        value: entry.to_string(),
                    });
                };
                parse_id(path.clone(), raw)
            })
            .collect(),
        other => Err(ClosureError::InvalidIdentifier {
            path,
            value: other.to_string(),
        }),
    }
}

fn parse_offset(
    value: &Value,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<(i32, i32, i32), ClosureError> {
    let Some(offset) = value.get("offset") else {
        return Ok((0, 0, 0));
    };
    let invalid = || ClosureError::InvalidField {
        kind,
        entry: entry.clone(),
        field: "offset",
        value: offset.to_string(),
    };
    let Some(entries) = offset.as_array() else {
        return Err(invalid());
    };
    if entries.len() != 3 {
        return Err(invalid());
    }
    let mut axes = [0i32; 3];
    for (index, axis) in entries.iter().enumerate() {
        let raw = axis.as_i64().ok_or_else(invalid)?;
        let raw = i32::try_from(raw).map_err(|_| invalid())?;
        // `Vec3i.offsetCodec(16)`.
        if !(-16..=16).contains(&raw) {
            return Err(invalid());
        }
        axes[index] = raw;
    }
    Ok((axes[0], axes[1], axes[2]))
}

fn parse_noise_parameters(
    value: &Value,
    referrer: &Identifier,
) -> Result<NoiseParametersSpec, ClosureError> {
    let amplitudes = field_array(value, "amplitudes", STATE_PROVIDER, referrer)?
        .iter()
        .map(|amplitude| {
            amplitude
                .as_f64()
                .ok_or_else(|| ClosureError::InvalidField {
                    kind: STATE_PROVIDER,
                    entry: referrer.clone(),
                    field: "amplitudes",
                    value: amplitude.to_string(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(NoiseParametersSpec {
        first_octave: i32_field(value, "firstOctave", STATE_PROVIDER, referrer)?,
        amplitudes,
    })
}

/// Vanilla's `TreeConfiguration.PLACE_BELOW_OVERWORLD_TRUNKS`, used when a tree
/// config omits `below_trunk_provider`:
/// `if_true(not(matching_tag(cannot_replace_below_tree_trunk))) -> dirt`.
fn below_overworld_trunks_provider() -> StateProviderSpec {
    StateProviderSpec::RuleBased {
        fallback: None,
        rules: vec![RuleBasedRule {
            predicate: BlockPredicateSpec::Not {
                predicate: Box::new(BlockPredicateSpec::MatchingBlockTag {
                    offset: (0, 0, 0),
                    tag: identifier("minecraft:cannot_replace_below_tree_trunk"),
                }),
            },
            then: StateProviderSpec::Simple {
                state: BlockStateSpec {
                    block: identifier("minecraft:dirt"),
                    properties: Vec::new(),
                },
            },
        }],
    }
}

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).expect("vanilla identifiers are valid")
}

#[cfg(test)]
#[path = "vanilla_feature_closure_tests.rs"]
mod tests;
