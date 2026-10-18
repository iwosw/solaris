//! Generic physical world-feature, structure and registered-worksite contracts.
use crate::{
    MAX_SCRIPT_BLOCK_COORDINATE, MAX_SCRIPT_WORLD_TIME, ScriptDtoError, check_contract_resource_id,
    validate_bounded_nonempty, validate_entity_uuid,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub const MAX_WORLD_FEATURE_PAGE: usize = 64;
pub const MAX_WORLD_FEATURE_BLOCKS: usize = 512;
pub const MAX_WORLD_FEATURE_ENTITIES: usize = 128;
pub const MAX_WORLD_FEATURE_AXIS: i32 = 256;
pub const MAX_WORKSITE_POINTS: usize = 128;
pub const MAX_WORLD_FEATURE_ID_BYTES: usize = 64;
pub const MAX_WORLD_BOUNDS_AXIS: i32 = 256;
pub const MAX_WORLD_BLOCK_QUERY_POSITIONS: usize = 128;
pub const MAX_WORLD_BLOCK_PROPERTIES: usize = 32;
pub const MAX_WORLD_BLOCK_PROPERTY_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorldBlockProperty {
    pub key: String,
    pub value: String,
}

impl ScriptWorldBlockProperty {
    #[must_use]
    pub fn new(key: String, value: String) -> Self {
        Self { key, value }
    }
}

/// Live physical facts. A missing block is unloaded, never an invented air block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorldBlockFacts {
    pub at: [i32; 3],
    pub block: Option<String>,
    pub properties: Vec<ScriptWorldBlockProperty>,
    pub standable: bool,
    pub container: bool,
    pub door: bool,
    pub bed: bool,
}

impl ScriptWorldBlockFacts {
    #[must_use]
    pub fn new(
        at: [i32; 3],
        block: Option<String>,
        properties: Vec<ScriptWorldBlockProperty>,
        standable: bool,
        container: bool,
        door: bool,
        bed: bool,
    ) -> Self {
        Self {
            at,
            block,
            properties,
            standable,
            container,
            door,
            bed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorldBlockQuery {
    pub world_revision: u64,
    pub blocks: Vec<ScriptWorldBlockFacts>,
}

impl ScriptWorldBlockQuery {
    #[must_use]
    pub fn new(world_revision: u64, blocks: Vec<ScriptWorldBlockFacts>) -> Self {
        Self {
            world_revision,
            blocks,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_revision(self.world_revision)?;
        if self.blocks.is_empty() || self.blocks.len() > MAX_WORLD_BLOCK_QUERY_POSITIONS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut positions = BTreeSet::new();
        for block in &self.blocks {
            validate_position(block.at)?;
            if !positions.insert(block.at) {
                return Err(ScriptDtoError::InvalidBounds);
            }
            if let Some(id) = &block.block {
                check_contract_resource_id(id)?;
                if block.properties.len() > MAX_WORLD_BLOCK_PROPERTIES {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                if block
                    .properties
                    .windows(2)
                    .any(|pair| pair[0].key >= pair[1].key)
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                for property in &block.properties {
                    for value in [&property.key, &property.value] {
                        validate_bounded_nonempty(
                            "block state property",
                            value,
                            MAX_WORLD_BLOCK_PROPERTY_BYTES,
                        )?;
                        if !value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                        {
                            return Err(ScriptDtoError::InvalidBounds);
                        }
                    }
                }
            } else if block.standable
                || block.container
                || block.door
                || block.bed
                || !block.properties.is_empty()
            {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}
/// Maximum byte length of a catalog blueprint id.
pub const MAX_BLUEPRINT_ID_BYTES: usize = 64;
/// Maximum byte length of a durable structure id.
pub const MAX_STRUCTURE_ID_BYTES: usize = 64;
/// Maximum byte length of an opaque survey token.
pub const MAX_SURVEY_TOKEN_BYTES: usize = 64;
/// Maximum extent of one survey bounds axis, in columns.
pub const MAX_SURVEY_BOUNDS_AXIS: i32 = 128;
/// Maximum biome or resource tags on one survey snapshot or column.
pub const MAX_SURVEY_TAGS: usize = 32;
/// Maximum active structures one plugin may hold.
pub const MAX_STRUCTURE_ACTIVE_PER_PLUGIN: usize = 64;
/// Maximum work units committed by one `advance_structure` portion.
pub const MAX_WORLD_COMMIT_PORTION: usize = 512;
/// Maximum distinct resources named by one structure material list.
pub const MAX_STRUCTURE_RESOURCE_TYPES: usize = 16;
/// Maximum stages one structure snapshot may plan.
pub const MAX_STRUCTURE_STAGES: usize = 32;
pub const MAX_BLUEPRINT_FOOTPRINT_AXIS: i32 = 64;
/// Whether the surveyed chunks are currently loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptChunkAvailability {
    Loaded,
    Unloaded,
}

impl ScriptChunkAvailability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Unloaded => "unloaded",
        }
    }
}

/// Durable lifecycle state of one structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptStructureState {
    Prepared,
    Running,
    Paused,
    Committed,
    Cancelled,
}

impl ScriptStructureState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Committed => "committed",
            Self::Cancelled => "cancelled",
        }
    }
}
/// Inclusive axis-aligned survey bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorldBounds {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl ScriptWorldBounds {
    pub fn new(min: [i32; 3], max: [i32; 3]) -> Result<Self, ScriptDtoError> {
        let bounds = Self { min, max };
        bounds.validate()?;
        Ok(bounds)
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_position(self.min)?;
        validate_position(self.max)?;
        for (min, max) in self.min.iter().zip(self.max.iter()) {
            let extent = i64::from(*max) - i64::from(*min) + 1;
            if extent <= 0 || extent > i64::from(MAX_WORLD_BOUNDS_AXIS) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }

    pub fn validate_survey(&self) -> Result<(), ScriptDtoError> {
        self.validate()?;
        for (min, max) in self.min.iter().zip(self.max.iter()) {
            if i64::from(*max) - i64::from(*min) + 1 > i64::from(MAX_SURVEY_BOUNDS_AXIS) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }

    /// Number of surface columns covered by the horizontal extents.
    #[must_use]
    pub fn columns(&self) -> u64 {
        let x = i64::from(self.max[0]) - i64::from(self.min[0]) + 1;
        let z = i64::from(self.max[2]) - i64::from(self.min[2]) + 1;
        (x * z) as u64
    }

    #[must_use]
    pub fn min(&self) -> [i32; 3] {
        self.min
    }

    #[must_use]
    pub fn max(&self) -> [i32; 3] {
        self.max
    }
}

/// One bounded, core-owned terrain survey snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptSurveySnapshot {
    pub dimension: String,
    pub bounds: ScriptWorldBounds,
    pub revision: u64,
    pub world_revision: u64,
    pub chunk_availability: ScriptChunkAvailability,
    pub survey_token: String,
    pub usable_plots: u32,
    pub water_columns: u32,
    pub existing_structures: u32,
    pub biome_tags: Vec<String>,
    pub resource_tags: Vec<String>,
}

impl ScriptSurveySnapshot {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dimension: String,
        bounds: ScriptWorldBounds,
        revision: u64,
        world_revision: u64,
        chunk_availability: ScriptChunkAvailability,
        survey_token: String,
        usable_plots: u32,
        water_columns: u32,
        existing_structures: u32,
        biome_tags: Vec<String>,
        resource_tags: Vec<String>,
    ) -> Self {
        Self {
            dimension,
            bounds,
            revision,
            world_revision,
            chunk_availability,
            survey_token,
            usable_plots,
            water_columns,
            existing_structures,
            biome_tags,
            resource_tags,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        check_contract_resource_id(&self.dimension)?;
        self.bounds.validate_survey()?;
        validate_revision(self.revision)?;
        validate_revision(self.world_revision)?;
        validate_bounded_nonempty("survey token", &self.survey_token, MAX_SURVEY_TOKEN_BYTES)?;
        // Every column the survey read is either a usable plot or water, so the
        // aggregates partition the surveyed tile. The snapshot stays bounded: it
        // carries the aggregate reading, never one record per column, because a
        // 128x128 tile cannot be materialized inside the handler
        // instruction/wall budget (contract 11.4).
        let columns = self.bounds.columns();
        if u64::from(self.usable_plots) + u64::from(self.water_columns) > columns {
            return Err(ScriptDtoError::InvalidBounds);
        }
        validate_tags("survey biome tags", &self.biome_tags)?;
        validate_tags("survey resource tags", &self.resource_tags)
    }
}
/// One authored stage plan for a structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStructureStagePlan {
    pub stage: String,
    pub block_count: u32,
    pub work_units: u64,
    pub materials: Vec<ScriptStructureMaterial>,
}

impl ScriptStructureStagePlan {
    #[must_use]
    pub fn new(
        stage: String,
        block_count: u32,
        work_units: u64,
        materials: Vec<ScriptStructureMaterial>,
    ) -> Self {
        Self {
            stage,
            block_count,
            work_units,
            materials,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_stage(&self.stage)?;
        validate_work_units(self.work_units)?;
        validate_materials(&self.materials)
    }
}

/// One bounded resource requirement of a structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStructureMaterial {
    pub resource: String,
    pub quantity: u64,
}

impl ScriptStructureMaterial {
    #[must_use]
    pub fn new(resource: String, quantity: u64) -> Self {
        Self { resource, quantity }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        check_contract_resource_id(&self.resource)?;
        if self.quantity == 0 || self.quantity > MAX_SCRIPT_WORLD_TIME {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// One committed portion receipt of a structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStructureReceipt {
    pub structure_id: String,
    pub stage: String,
    pub sequence: u64,
    pub block_count: u32,
    pub work_units: u64,
    pub consumed: Vec<ScriptStructureMaterial>,
    pub revision: u64,
}

impl ScriptStructureReceipt {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        structure_id: String,
        stage: String,
        sequence: u64,
        block_count: u32,
        work_units: u64,
        consumed: Vec<ScriptStructureMaterial>,
        revision: u64,
    ) -> Self {
        Self {
            structure_id,
            stage,
            sequence,
            block_count,
            work_units,
            consumed,
            revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_structure_id(&self.structure_id)?;
        validate_stage(&self.stage)?;
        validate_revision(self.sequence)?;
        validate_commit_portion(self.work_units)?;
        validate_materials(&self.consumed)?;
        validate_revision(self.revision)
    }
}

/// One bounded, core-owned structure snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStructureSnapshot {
    pub structure_id: String,
    pub blueprint_id: String,
    pub dimension: String,
    pub state: ScriptStructureState,
    pub revision: u64,
    pub origin: [i32; 3],
    pub rotation: u16,
    pub reserved_footprint: [i32; 3],
    pub stages: Vec<ScriptStructureStagePlan>,
    /// Zero-based stage currently accepting work, or `stages.len()` when complete.
    pub current_stage_index: u32,
    /// Work committed in `current_stage_index`, or zero at the terminal index.
    pub completed_work_units: u64,
    pub resource_plan_hash: String,
    pub reservation_ref: Option<String>,
    pub watermark: u64,
    pub consumed: Vec<ScriptStructureMaterial>,
    pub remaining: Vec<ScriptStructureMaterial>,
    pub pause_reason: Option<String>,
    pub interaction_points: Vec<ScriptWorksiteInteractionPoint>,
}

impl ScriptStructureSnapshot {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        structure_id: String,
        blueprint_id: String,
        dimension: String,
        state: ScriptStructureState,
        revision: u64,
        origin: [i32; 3],
        rotation: u16,
        reserved_footprint: [i32; 3],
        stages: Vec<ScriptStructureStagePlan>,
        current_stage_index: u32,
        completed_work_units: u64,
        resource_plan_hash: String,
        reservation_ref: Option<String>,
        watermark: u64,
        consumed: Vec<ScriptStructureMaterial>,
        remaining: Vec<ScriptStructureMaterial>,
        pause_reason: Option<String>,
        interaction_points: Vec<ScriptWorksiteInteractionPoint>,
    ) -> Self {
        Self {
            structure_id,
            blueprint_id,
            dimension,
            state,
            revision,
            origin,
            rotation,
            reserved_footprint,
            stages,
            current_stage_index,
            completed_work_units,
            resource_plan_hash,
            reservation_ref,
            watermark,
            consumed,
            remaining,
            pause_reason,
            interaction_points,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_structure_id(&self.structure_id)?;
        validate_blueprint_id(&self.blueprint_id)?;
        check_contract_resource_id(&self.dimension)?;
        validate_revision(self.revision)?;
        validate_rotation(self.rotation)?;
        validate_blueprint_footprint(self.reserved_footprint)?;
        validate_position(self.origin)?;
        if self.interaction_points.len() > MAX_WORKSITE_POINTS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut points = BTreeSet::new();
        for point in &self.interaction_points {
            point.validate()?;
            if !points.insert(point.at()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        if self.stages.len() > MAX_STRUCTURE_STAGES {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut stages = BTreeSet::new();
        for stage in &self.stages {
            stage.validate()?;
            if !stages.insert(stage.stage.as_str()) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        let terminal_stage_index = self.stages.len() as u32;
        if self.current_stage_index > terminal_stage_index
            || (self.current_stage_index == terminal_stage_index && self.completed_work_units != 0)
            || (self.state == ScriptStructureState::Committed
                && self.current_stage_index != terminal_stage_index)
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(stage) = self.stages.get(self.current_stage_index as usize)
            && self.completed_work_units > stage.work_units
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        validate_hex_hash("structure resource plan hash", &self.resource_plan_hash)?;
        if let Some(reservation_ref) = &self.reservation_ref {
            validate_reservation_ref(reservation_ref)?;
        }
        validate_revision(self.watermark)?;
        validate_materials(&self.consumed)?;
        validate_materials(&self.remaining)?;
        if let Some(pause_reason) = &self.pause_reason {
            validate_bounded_nonempty(
                "structure pause reason",
                pause_reason,
                MAX_WORLD_FEATURE_ID_BYTES,
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorldFeatureBlock {
    pub at: [i32; 3],
    pub block: String,
}
impl ScriptWorldFeatureBlock {
    #[must_use]
    pub fn new(at: [i32; 3], block: String) -> Self {
        Self { at, block }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorldFeature {
    pub feature_id: String,
    pub kind: String,
    pub dimension: String,
    pub bounds: ScriptWorldBounds,
    pub contents_known: bool,
    pub blocks: Vec<ScriptWorldFeatureBlock>,
    pub interaction_points: Vec<ScriptWorksiteInteractionPoint>,
    pub entity_ids: Vec<String>,
}
impl ScriptWorldFeature {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        feature_id: String,
        kind: String,
        dimension: String,
        bounds: ScriptWorldBounds,
        contents_known: bool,
        blocks: Vec<ScriptWorldFeatureBlock>,
        interaction_points: Vec<ScriptWorksiteInteractionPoint>,
        entity_ids: Vec<String>,
    ) -> Self {
        Self {
            feature_id,
            kind,
            dimension,
            bounds,
            contents_known,
            blocks,
            interaction_points,
            entity_ids,
        }
    }
    pub fn canonicalize(&mut self) {
        self.blocks
            .sort_unstable_by(|a, b| a.at.cmp(&b.at).then_with(|| a.block.cmp(&b.block)));
        canonicalize_points(&mut self.interaction_points);
        self.entity_ids.sort_unstable();
    }
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_feature_id(&self.feature_id)?;
        check_contract_resource_id(&self.kind)?;
        check_contract_resource_id(&self.dimension)?;
        validate_position(self.bounds.min)?;
        validate_position(self.bounds.max)?;
        for (min, max) in self.bounds.min.iter().zip(self.bounds.max.iter()) {
            let extent = i64::from(*max) - i64::from(*min) + 1;
            if extent <= 0 || extent > i64::from(MAX_WORLD_FEATURE_AXIS) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        if self.blocks.len() > MAX_WORLD_FEATURE_BLOCKS
            || self.interaction_points.len() > MAX_WORKSITE_POINTS
            || self.entity_ids.len() > MAX_WORLD_FEATURE_ENTITIES
            || (!self.contents_known
                && (!self.blocks.is_empty()
                    || !self.interaction_points.is_empty()
                    || !self.entity_ids.is_empty()))
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if !self.interaction_points.is_empty() {
            validate_points(&self.bounds, &self.interaction_points)?;
        }
        let mut positions = BTreeSet::new();
        for block in &self.blocks {
            validate_position(block.at)?;
            check_contract_resource_id(&block.block)?;
            if !positions.insert(block.at) || !point_in_footprint(&self.bounds, block.at) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        let mut ids = BTreeSet::new();
        for id in &self.entity_ids {
            validate_entity_uuid(id)?;
            if !ids.insert(id) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorldFeaturePage {
    pub features: Vec<ScriptWorldFeature>,
    pub cursor: Option<String>,
}
impl ScriptWorldFeaturePage {
    #[must_use]
    pub fn new(features: Vec<ScriptWorldFeature>, cursor: Option<String>) -> Self {
        Self { features, cursor }
    }
    pub fn canonicalize(&mut self) {
        self.features
            .sort_unstable_by(|a, b| a.feature_id.cmp(&b.feature_id));
        for feature in &mut self.features {
            feature.canonicalize();
        }
    }
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if self.features.len() > MAX_WORLD_FEATURE_PAGE {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(cursor) = &self.cursor {
            validate_cursor(cursor)?;
        }
        let mut ids = BTreeSet::new();
        for feature in &self.features {
            feature.validate()?;
            if !ids.insert(&feature.feature_id) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptWorksiteInteractionPoint {
    Entrance { at: [i32; 3] },
    Block { at: [i32; 3], block: String },
    Workstation { at: [i32; 3], capability: String },
    Container { at: [i32; 3] },
}
impl ScriptWorksiteInteractionPoint {
    pub fn at(&self) -> [i32; 3] {
        match self {
            Self::Entrance { at }
            | Self::Block { at, .. }
            | Self::Workstation { at, .. }
            | Self::Container { at } => *at,
        }
    }
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_position(self.at())?;
        match self {
            Self::Block { block, .. } => check_contract_resource_id(block),
            Self::Workstation { capability, .. } => check_contract_resource_id(capability),
            _ => Ok(()),
        }
    }
}
fn canonicalize_points(points: &mut [ScriptWorksiteInteractionPoint]) {
    points.sort_unstable_by_key(ScriptWorksiteInteractionPoint::at);
}
fn validate_points(
    footprint: &ScriptWorldBounds,
    points: &[ScriptWorksiteInteractionPoint],
) -> Result<(), ScriptDtoError> {
    if points.is_empty() || points.len() > MAX_WORKSITE_POINTS {
        return Err(ScriptDtoError::InvalidBounds);
    }
    let mut seen = BTreeSet::new();
    for point in points {
        point.validate()?;
        if !seen.insert(point.at()) || !point_in_footprint(footprint, point.at()) {
            return Err(ScriptDtoError::InvalidBounds);
        }
    }
    Ok(())
}
fn point_in_footprint(footprint: &ScriptWorldBounds, at: [i32; 3]) -> bool {
    (0..3).all(|i| (footprint.min[i]..=footprint.max[i]).contains(&at[i]))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorksiteRequest {
    pub footprint: ScriptWorldBounds,
    pub interaction_points: Vec<ScriptWorksiteInteractionPoint>,
    pub survey_token: String,
    pub expected_world_revision: u64,
}
impl ScriptWorksiteRequest {
    pub fn new(
        footprint: ScriptWorldBounds,
        interaction_points: Vec<ScriptWorksiteInteractionPoint>,
        survey_token: String,
        expected_world_revision: u64,
    ) -> Self {
        Self {
            footprint,
            interaction_points,
            survey_token,
            expected_world_revision,
        }
    }
    pub fn canonicalize(&mut self) {
        canonicalize_points(&mut self.interaction_points);
    }
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        self.footprint.validate_survey()?;
        validate_points(&self.footprint, &self.interaction_points)?;
        validate_bounded_nonempty("survey token", &self.survey_token, MAX_SURVEY_TOKEN_BYTES)?;
        validate_revision(self.expected_world_revision)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorksiteSnapshot {
    pub worksite_id: String,
    pub footprint: ScriptWorldBounds,
    pub interaction_points: Vec<ScriptWorksiteInteractionPoint>,
    pub world_revision: u64,
    pub revision: u64,
}
impl ScriptWorksiteSnapshot {
    pub fn new(
        worksite_id: String,
        footprint: ScriptWorldBounds,
        interaction_points: Vec<ScriptWorksiteInteractionPoint>,
        world_revision: u64,
        revision: u64,
    ) -> Self {
        Self {
            worksite_id,
            footprint,
            interaction_points,
            world_revision,
            revision,
        }
    }
    pub fn canonicalize(&mut self) {
        canonicalize_points(&mut self.interaction_points);
    }
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_structure_id(&self.worksite_id)?;
        self.footprint.validate_survey()?;
        validate_points(&self.footprint, &self.interaction_points)?;
        validate_revision(self.world_revision)?;
        validate_revision(self.revision)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptWorksiteFailure {
    Unloaded,
    MissingBlock,
    MissingWorkstation,
    MissingContainer,
    BlockedEntrance,
    Unwalkable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorksitePointFailure {
    pub at: [i32; 3],
    pub reason: ScriptWorksiteFailure,
}
impl ScriptWorksitePointFailure {
    #[must_use]
    pub fn new(at: [i32; 3], reason: ScriptWorksiteFailure) -> Self {
        Self { at, reason }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorksiteCheck {
    pub worksite_id: String,
    pub world_revision: u64,
    pub valid: bool,
    pub blocked_points: Vec<ScriptWorksitePointFailure>,
}
impl ScriptWorksiteCheck {
    #[must_use]
    pub fn new(
        worksite_id: String,
        world_revision: u64,
        valid: bool,
        blocked_points: Vec<ScriptWorksitePointFailure>,
    ) -> Self {
        Self {
            worksite_id,
            world_revision,
            valid,
            blocked_points,
        }
    }
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_live_point_check(
            &self.worksite_id,
            self.world_revision,
            self.valid,
            &self.blocked_points,
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptStructureCheck {
    pub structure_id: String,
    pub plugin_id: String,
    pub world_revision: u64,
    pub valid: bool,
    pub blocked_points: Vec<ScriptWorksitePointFailure>,
}
impl ScriptStructureCheck {
    #[must_use]
    pub fn new(
        structure_id: String,
        plugin_id: String,
        world_revision: u64,
        valid: bool,
        blocked_points: Vec<ScriptWorksitePointFailure>,
    ) -> Self {
        Self {
            structure_id,
            plugin_id,
            world_revision,
            valid,
            blocked_points,
        }
    }
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("plugin id", &self.plugin_id, 128)?;
        validate_live_point_check(
            &self.structure_id,
            self.world_revision,
            self.valid,
            &self.blocked_points,
        )
    }
}
fn validate_live_point_check(
    id: &str,
    world_revision: u64,
    valid: bool,
    blocked_points: &[ScriptWorksitePointFailure],
) -> Result<(), ScriptDtoError> {
    validate_structure_id(id)?;
    validate_revision(world_revision)?;
    if blocked_points.len() > MAX_WORKSITE_POINTS || valid != blocked_points.is_empty() {
        return Err(ScriptDtoError::InvalidBounds);
    }
    let mut seen = BTreeSet::new();
    for point in blocked_points {
        validate_position(point.at)?;
        if !seen.insert(point.at) {
            return Err(ScriptDtoError::InvalidBounds);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptWorldStructureOperation {
    QueryBlocks {
        dimension: String,
        positions: Vec<[i32; 3]>,
    },
    ListFeatures {
        cursor: Option<String>,
        limit: u8,
    },
    QueryFeature {
        feature_id: String,
    },
    Survey {
        dimension: String,
        bounds: ScriptWorldBounds,
    },
    PrepareStructure {
        operation_id: String,
        dimension: String,
        blueprint_id: String,
        anchor: [i32; 3],
        rotation: u16,
        survey_token: String,
        expected_world_revision: u64,
        expected_blueprint_hash: Option<String>,
    },
    AdvanceStructure {
        operation_id: String,
        structure_id: String,
        stage: String,
        reservation_ref: String,
        expected_revision: u64,
        work_units: u64,
    },
    PauseStructure {
        operation_id: String,
        structure_id: String,
        expected_revision: u64,
    },
    ResumeStructure {
        operation_id: String,
        structure_id: String,
        expected_revision: u64,
    },
    CancelStructure {
        operation_id: String,
        structure_id: String,
        expected_revision: u64,
    },
    Status {
        structure_id: String,
    },
    RegisterWorksite {
        operation_id: String,
        request: Box<ScriptWorksiteRequest>,
    },
    WorksiteStatus {
        worksite_id: String,
    },
    CheckWorksite {
        worksite_id: String,
    },
    CheckStructure {
        structure_id: String,
    },
}
impl ScriptWorldStructureOperation {
    const MAX_RESERVATION_REF_BYTES: usize = 64;
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::PrepareStructure { operation_id, .. }
            | Self::AdvanceStructure { operation_id, .. }
            | Self::PauseStructure { operation_id, .. }
            | Self::ResumeStructure { operation_id, .. }
            | Self::CancelStructure { operation_id, .. }
            | Self::RegisterWorksite { operation_id, .. } => Some(operation_id),
            _ => None,
        }
    }
    pub fn canonicalize(&mut self) {
        if let Self::RegisterWorksite { request, .. } = self {
            request.canonicalize();
        }
    }
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(id) = self.operation_id() {
            crate::validate_script_id_value(id)?;
        }
        match self {
            Self::QueryBlocks {
                dimension,
                positions,
            } => {
                check_contract_resource_id(dimension)?;
                if positions.is_empty() || positions.len() > MAX_WORLD_BLOCK_QUERY_POSITIONS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut seen = BTreeSet::new();
                for position in positions {
                    validate_position(*position)?;
                    if !seen.insert(*position) {
                        return Err(ScriptDtoError::InvalidBounds);
                    }
                }
            }
            Self::ListFeatures { cursor, limit } => {
                if *limit == 0 || usize::from(*limit) > MAX_WORLD_FEATURE_PAGE {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                if let Some(cursor) = cursor {
                    validate_cursor(cursor)?;
                }
            }
            Self::QueryFeature { feature_id } => validate_feature_id(feature_id)?,
            Self::Survey { dimension, bounds } => {
                check_contract_resource_id(dimension)?;
                bounds.validate_survey()?;
            }
            Self::PrepareStructure {
                dimension,
                blueprint_id,
                anchor,
                rotation,
                survey_token,
                expected_world_revision,
                expected_blueprint_hash,
                ..
            } => {
                check_contract_resource_id(dimension)?;
                validate_blueprint_id(blueprint_id)?;
                validate_position(*anchor)?;
                validate_rotation(*rotation)?;
                validate_bounded_nonempty("survey token", survey_token, MAX_SURVEY_TOKEN_BYTES)?;
                validate_revision(*expected_world_revision)?;
                if let Some(hash) = expected_blueprint_hash {
                    validate_hex_hash("expected blueprint hash", hash)?;
                }
            }
            Self::AdvanceStructure {
                structure_id,
                stage,
                reservation_ref,
                expected_revision,
                work_units,
                ..
            } => {
                validate_structure_id(structure_id)?;
                validate_stage(stage)?;
                validate_reservation_ref(reservation_ref)?;
                validate_revision(*expected_revision)?;
                validate_commit_portion(*work_units)?;
            }
            Self::PauseStructure {
                structure_id,
                expected_revision,
                ..
            }
            | Self::ResumeStructure {
                structure_id,
                expected_revision,
                ..
            }
            | Self::CancelStructure {
                structure_id,
                expected_revision,
                ..
            } => {
                validate_structure_id(structure_id)?;
                validate_revision(*expected_revision)?;
            }
            Self::Status { structure_id } | Self::CheckStructure { structure_id } => {
                validate_structure_id(structure_id)?
            }
            Self::RegisterWorksite { request, .. } => request.validate()?,
            Self::WorksiteStatus { worksite_id } | Self::CheckWorksite { worksite_id } => {
                validate_structure_id(worksite_id)?
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptWorldStructureResult {
    Blocks {
        query: Box<ScriptWorldBlockQuery>,
    },
    Features {
        page: Box<ScriptWorldFeaturePage>,
    },
    Feature {
        feature: Box<ScriptWorldFeature>,
    },
    Survey {
        survey: Box<ScriptSurveySnapshot>,
    },
    Structure {
        structure: Box<ScriptStructureSnapshot>,
    },
    Receipt {
        receipt: Box<ScriptStructureReceipt>,
    },
    Worksite {
        worksite: Box<ScriptWorksiteSnapshot>,
    },
    WorksiteCheck {
        check: Box<ScriptWorksiteCheck>,
    },
    StructureCheck {
        check: Box<ScriptStructureCheck>,
    },
}
impl ScriptWorldStructureResult {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Blocks { query } => query.validate(),
            Self::Features { page } => page.validate(),
            Self::Feature { feature } => feature.validate(),
            Self::Survey { survey } => survey.validate(),
            Self::Structure { structure } => structure.validate(),
            Self::Receipt { receipt } => receipt.validate(),
            Self::Worksite { worksite } => worksite.validate(),
            Self::WorksiteCheck { check } => check.validate(),
            Self::StructureCheck { check } => check.validate(),
        }
    }
}
fn validate_rotation(rotation: u16) -> Result<(), ScriptDtoError> {
    if matches!(rotation, 0 | 90 | 180 | 270) {
        Ok(())
    } else {
        Err(ScriptDtoError::InvalidBounds)
    }
}

fn validate_revision(revision: u64) -> Result<(), ScriptDtoError> {
    if revision > MAX_SCRIPT_WORLD_TIME {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_work_units(work_units: u64) -> Result<(), ScriptDtoError> {
    if work_units == 0 || work_units > MAX_SCRIPT_WORLD_TIME {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_commit_portion(work_units: u64) -> Result<(), ScriptDtoError> {
    if work_units == 0 || work_units > MAX_WORLD_COMMIT_PORTION as u64 {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_blueprint_footprint(size: [i32; 3]) -> Result<(), ScriptDtoError> {
    validate_axis_bounded(size, MAX_BLUEPRINT_FOOTPRINT_AXIS)
}

fn validate_axis_bounded(size: [i32; 3], max: i32) -> Result<(), ScriptDtoError> {
    if size.iter().any(|axis| *axis <= 0 || *axis > max) {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_hex_hash(field: &'static str, value: &str) -> Result<(), ScriptDtoError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ScriptDtoError::InvalidId {
            field,
            actual_bytes: value.len(),
        });
    }
    Ok(())
}

fn validate_materials(materials: &[ScriptStructureMaterial]) -> Result<(), ScriptDtoError> {
    if materials.len() > MAX_STRUCTURE_RESOURCE_TYPES {
        return Err(ScriptDtoError::InvalidBounds);
    }
    let mut resources = BTreeSet::new();
    for material in materials {
        material.validate()?;
        if !resources.insert(material.resource.as_str()) {
            return Err(ScriptDtoError::InvalidBounds);
        }
    }
    Ok(())
}

fn validate_tags(field: &'static str, tags: &[String]) -> Result<(), ScriptDtoError> {
    if tags.len() > MAX_SURVEY_TAGS {
        return Err(ScriptDtoError::InvalidBounds);
    }
    for tag in tags {
        validate_bounded_nonempty(field, tag, MAX_SURVEY_TOKEN_BYTES)?;
    }
    Ok(())
}
fn validate_blueprint_id(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("blueprint id", value, MAX_BLUEPRINT_ID_BYTES)?;
    check_contract_resource_id(value)
}

fn validate_structure_id(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("structure id", value, MAX_STRUCTURE_ID_BYTES)
}

fn validate_position(position: [i32; 3]) -> Result<(), ScriptDtoError> {
    if position
        .iter()
        .any(|coordinate| coordinate.unsigned_abs() > MAX_SCRIPT_BLOCK_COORDINATE as u32)
    {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_stage(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("structure stage", value, MAX_STRUCTURE_ID_BYTES)
}

fn validate_cursor(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("world feature cursor", value, MAX_WORLD_FEATURE_ID_BYTES)
}

fn validate_reservation_ref(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty(
        "structure reservation",
        value,
        ScriptWorldStructureOperation::MAX_RESERVATION_REF_BYTES,
    )
}

fn validate_feature_id(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("world feature id", value, MAX_WORLD_FEATURE_ID_BYTES)
}
