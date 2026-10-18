//! Generic NPC physical work, group controls and native combat DTOs.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    MAX_SCRIPT_WORLD_TIME, ScriptDtoError, ScriptInventoryEndpoint, check_contract_resource_id,
    validate_bounded_nonempty, validate_bounded_value,
};

/// Maximum opaque handles one squad order may address.
pub const MAX_NPC_ORDER_HANDLES: usize = 64;
/// Maximum patrol waypoints on one order.
pub const MAX_ORDER_WAYPOINTS: usize = 16;
/// Minimum patrol waypoints on one order.
pub const MIN_ORDER_WAYPOINTS: usize = 2;
/// Maximum allied affiliations on one engagement policy.
pub const MAX_ORDER_AFFILIATIONS: usize = 64;
/// Maximum target references on one attack order.
pub const MAX_ORDER_TARGETS: usize = 64;
/// Maximum byte length of an opaque server-issued target reference.
pub const MAX_TARGET_REF_BYTES: usize = 64;
/// Maximum byte length of a permitted crafting recipe id.
pub const MAX_RECIPE_ID_BYTES: usize = 128;
/// Maximum extent of one work area axis in blocks.
pub const MAX_WORK_AREA_AXIS: u32 = 16;
/// Maximum work units one assignment may commit.
pub const MAX_WORK_UNITS: u64 = 4096;
/// Maximum formation spacing in half-blocks (16 blocks).
pub const MAX_FORMATION_SPACING: u8 = 32;
/// Minimum formation spacing in half-blocks (0.5 blocks).
pub const MIN_FORMATION_SPACING: u8 = 1;
/// Maximum engagement radius in blocks.
pub const MAX_ENGAGEMENT_RADIUS: u8 = 64;
/// Minimum engagement radius in blocks.
pub const MIN_ENGAGEMENT_RADIUS: u8 = 1;
/// Largest absolute block coordinate accepted by a work area.
pub const MAX_SCRIPT_BLOCK_COORDINATE: i32 = 30_000_000;
/// Maximum ordered npcs one combat event batch may carry.
pub const MAX_COMBAT_EVENTS: usize = 16;
/// Maximum committed, unacknowledged craft portions retained per assignment.
pub const MAX_WORK_RECEIPTS: usize = 32;

/// One integer block coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptBlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ScriptBlockPosition {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        for value in [self.x, self.y, self.z] {
            if value.unsigned_abs() > MAX_SCRIPT_BLOCK_COORDINATE as u32 {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}

/// One bounded axis-aligned work area the engine executes against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorkArea {
    pub dimension: String,
    pub min: ScriptBlockPosition,
    pub max: ScriptBlockPosition,
}

impl ScriptWorkArea {
    #[must_use]
    pub fn new(dimension: String, min: ScriptBlockPosition, max: ScriptBlockPosition) -> Self {
        Self {
            dimension,
            min,
            max,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("work dimension", &self.dimension, 128)?;
        self.min.validate()?;
        self.max.validate()?;
        let axes = [
            (self.min.x, self.max.x),
            (self.min.y, self.max.y),
            (self.min.z, self.max.z),
        ];
        for (min, max) in axes {
            if min > max {
                return Err(ScriptDtoError::InvalidBounds);
            }
            let extent = u32::try_from(i64::from(max) - i64::from(min) + 1)
                .map_err(|_| ScriptDtoError::InvalidBounds)?;
            if extent == 0 || extent > MAX_WORK_AREA_AXIS {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }

    /// Number of cells in the area, already bounded by [`MAX_WORK_AREA_AXIS`].
    #[must_use]
    pub fn cell_count(&self) -> u32 {
        let x = (self.max.x - self.min.x + 1).unsigned_abs();
        let y = (self.max.y - self.min.y + 1).unsigned_abs();
        let z = (self.max.z - self.min.z + 1).unsigned_abs();
        x.saturating_mul(y).saturating_mul(z)
    }
}

/// Closed formation vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptFormationKind {
    Line,
    Column,
    Wedge,
    Square,
}

impl ScriptFormationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Column => "column",
            Self::Wedge => "wedge",
            Self::Square => "square",
        }
    }
}

/// Engine-rendered formation for one order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptFormation {
    pub kind: ScriptFormationKind,
    /// Spacing between slots in half-blocks (integer; `4` means two blocks).
    pub spacing: u8,
}

impl ScriptFormation {
    #[must_use]
    pub const fn new(kind: ScriptFormationKind, spacing: u8) -> Self {
        Self { kind, spacing }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if !(MIN_FORMATION_SPACING..=MAX_FORMATION_SPACING).contains(&self.spacing) {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// Closed hostile category a target reference may address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptHostileCategory {
    Hostile,
    Player,
    OwnedNpc,
    NeutralAnimal,
}

impl ScriptHostileCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hostile => "hostile",
            Self::Player => "player",
            Self::OwnedNpc => "owned_npc",
            Self::NeutralAnimal => "neutral_animal",
        }
    }
}

/// Bounded ally/enemy policy supplied with an attack order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptEngagementPolicy {
    pub revision: u64,
    /// Opaque alliance ids (owned npc handles, allied player uuids).
    pub allies: Vec<String>,
    pub permitted: Vec<ScriptHostileCategory>,
}

impl ScriptEngagementPolicy {
    #[must_use]
    pub fn new(revision: u64, allies: Vec<String>, permitted: Vec<ScriptHostileCategory>) -> Self {
        Self {
            revision,
            allies,
            permitted,
        }
    }

    pub fn canonicalize(&mut self) {
        self.allies.sort_unstable();
        self.allies.dedup();
        self.permitted
            .sort_unstable_by_key(|category| category.as_str());
        self.permitted.dedup();
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if self.revision > MAX_SCRIPT_WORLD_TIME
            || self.allies.len() > MAX_ORDER_AFFILIATIONS
            || self.permitted.is_empty()
            || self.permitted.len() > 4
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut previous = None;
        for ally in &self.allies {
            validate_bounded_nonempty("order ally", ally, MAX_TARGET_REF_BYTES)?;
            if previous.is_some_and(|value: &str| value >= ally.as_str()) {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "order allies",
                });
            }
            previous = Some(ally.as_str());
        }
        let mut previous = None;
        for category in &self.permitted {
            if previous.is_some_and(|value: &str| value >= category.as_str()) {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "permitted categories",
                });
            }
            previous = Some(category.as_str());
        }
        Ok(())
    }
}

/// One opaque, server-issued target reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptOrderTargetRef {
    pub target_ref: String,
    #[serde(default)]
    pub policy_revision: u64,
    #[serde(default)]
    pub expires_revision: u64,
}

impl ScriptOrderTargetRef {
    #[must_use]
    pub fn new(target_ref: String, policy_revision: u64, expires_revision: u64) -> Self {
        Self {
            target_ref,
            policy_revision,
            expires_revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("target ref", &self.target_ref, MAX_TARGET_REF_BYTES)?;
        if self.policy_revision > MAX_SCRIPT_WORLD_TIME
            || self.expires_revision > MAX_SCRIPT_WORLD_TIME
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// Closed work-order union. Each variant names a concrete bounded target and
/// the tool, feed or recipe the engine requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptNpcWorkOrder {
    Harvest {
        area: ScriptWorkArea,
        tool: String,
    },
    Replant {
        area: ScriptWorkArea,
        seed: String,
        tool: String,
    },
    CutTree {
        area: ScriptWorkArea,
        tool: String,
    },
    Mine {
        area: ScriptWorkArea,
        tool: String,
    },
    TendLivestock {
        area: ScriptWorkArea,
        feed: String,
    },
    Haul {
        source: ScriptInventoryEndpoint,
        destination: ScriptInventoryEndpoint,
        /// The exact item this move takes from the source.
        ///
        /// `None` moves the first physical source stack in slot order.
        item: Option<String>,
    },
    Craft {
        recipe: String,
        count: u32,
        /// One loaded station cell whose capability matches the chosen recipe.
        /// Furnace-backed recipes remain in their own fuel/burn state machine.
        station: ScriptWorkArea,
    },
    Construct {
        structure_id: String,
        stage: String,
        expected_revision: u64,
    },
}

impl ScriptNpcWorkOrder {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Harvest { area, tool }
            | Self::CutTree { area, tool }
            | Self::Mine { area, tool } => {
                area.validate()?;
                check_contract_resource_id(tool)
            }
            Self::Replant { area, seed, tool } => {
                area.validate()?;
                check_contract_resource_id(seed)?;
                check_contract_resource_id(tool)
            }
            Self::TendLivestock { area, feed } => {
                area.validate()?;
                check_contract_resource_id(feed)
            }
            Self::Haul {
                source,
                destination,
                item,
            } => {
                source.validate()?;
                destination.validate()?;
                if source == destination {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "haul endpoints",
                    });
                }
                if let Some(item) = item {
                    check_contract_resource_id(item)?;
                }
                Ok(())
            }
            Self::Craft {
                recipe,
                count,
                station,
            } => {
                validate_bounded_nonempty("recipe", recipe, MAX_RECIPE_ID_BYTES)?;
                station.validate()?;
                if station.min != station.max || *count == 0 || *count > i32::MAX as u32 {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Construct {
                structure_id,
                stage,
                expected_revision,
            } => {
                validate_bounded_nonempty("structure id", structure_id, 64)?;
                validate_bounded_nonempty("structure stage", stage, 64)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
        }
    }
}

/// Closed squad-order union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptNpcOrder {
    Follow {
        target_player: u64,
        formation: ScriptFormation,
    },
    Move {
        dimension: String,
        anchor: ScriptBlockPosition,
        heading_degrees: u16,
        formation: ScriptFormation,
    },
    Hold {
        anchor: ScriptBlockPosition,
        heading_degrees: u16,
        formation: ScriptFormation,
        engagement_radius: u8,
    },
    Patrol {
        waypoints: Vec<ScriptBlockPosition>,
        formation: ScriptFormation,
        engagement_radius: u8,
    },
    /// Empty approved targets observes the explicit policy without attacking.
    /// The returned native references can authorize a subsequent attack.
    Attack {
        targets: Vec<ScriptOrderTargetRef>,
        policy: ScriptEngagementPolicy,
    },
    Retreat {
        anchor: ScriptBlockPosition,
        formation: ScriptFormation,
    },
}

impl ScriptNpcOrder {
    /// Canonical ordering so one operation fingerprint is input-order stable.
    pub fn canonicalize(&mut self) {
        match self {
            Self::Attack { targets, policy } => {
                targets.sort_unstable_by(|left, right| left.target_ref.cmp(&right.target_ref));
                policy.canonicalize();
            }
            Self::Patrol { waypoints, .. } => {
                // Waypoint order is the patrol route and is never reordered.
                waypoints.dedup();
            }
            _ => {}
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Follow {
                target_player,
                formation,
            } => {
                if *target_player == 0 || *target_player > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                formation.validate()
            }
            Self::Move {
                dimension,
                anchor,
                heading_degrees,
                formation,
            } => {
                validate_bounded_nonempty("order dimension", dimension, 128)?;
                anchor.validate()?;
                validate_heading(*heading_degrees)?;
                formation.validate()
            }
            Self::Hold {
                anchor,
                heading_degrees,
                formation,
                engagement_radius,
            } => {
                anchor.validate()?;
                validate_heading(*heading_degrees)?;
                formation.validate()?;
                validate_engagement_radius(*engagement_radius)
            }
            Self::Patrol {
                waypoints,
                formation,
                engagement_radius,
            } => {
                if waypoints.len() < MIN_ORDER_WAYPOINTS || waypoints.len() > MAX_ORDER_WAYPOINTS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut previous = None;
                for waypoint in waypoints {
                    waypoint.validate()?;
                    if previous.is_some_and(|value: &ScriptBlockPosition| value == waypoint) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "patrol waypoints",
                        });
                    }
                    previous = Some(waypoint);
                }
                formation.validate()?;
                validate_engagement_radius(*engagement_radius)
            }
            Self::Attack { targets, policy } => {
                if targets.len() > MAX_ORDER_TARGETS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut previous = None;
                for target in targets {
                    target.validate()?;
                    if previous.is_some_and(|value: &str| value >= target.target_ref.as_str()) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "attack targets",
                        });
                    }
                    previous = Some(target.target_ref.as_str());
                }
                policy.validate()
            }
            Self::Retreat { anchor, formation } => {
                anchor.validate()?;
                formation.validate()
            }
        }
    }
}

fn validate_heading(heading_degrees: u16) -> Result<(), ScriptDtoError> {
    if heading_degrees >= 360 {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_engagement_radius(engagement_radius: u8) -> Result<(), ScriptDtoError> {
    if !(MIN_ENGAGEMENT_RADIUS..=MAX_ENGAGEMENT_RADIUS).contains(&engagement_radius) {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

/// One bounded npc work or order call inside the durable operation
/// envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptNpcOrderOperation {
    Query {
        handles: Vec<String>,
    },
    /// Explicit held-item input, independent of movement and engagement orders.
    UseItem {
        operation_id: String,
        handle: String,
        equipment_slot: u8,
        active: bool,
        expected_revision: u64,
        execution_permit: Option<crate::ScriptNpcExecutionPermit>,
    },
    DeployItem {
        operation_id: String,
        handle: String,
        equipment_slot: u8,
        expected_revision: u64,
        execution_permit: Option<crate::ScriptNpcExecutionPermit>,
        entity_uuid: String,
        support: ScriptBlockPosition,
        position: crate::ScriptPosition,
        heading_degrees: u16,
    },
    RetractItem {
        operation_id: String,
        handle: String,
        expected_revision: u64,
        execution_permit: Option<crate::ScriptNpcExecutionPermit>,
        entity_uuid: String,
        equipment_slot: u8,
    },
    CaptureItem {
        operation_id: String,
        handle: String,
        expected_revision: u64,
        execution_permit: Option<crate::ScriptNpcExecutionPermit>,
        entity_uuid: String,
    },
    AssignWork {
        operation_id: String,
        handle: String,
        work: ScriptNpcWorkOrder,
        work_units: u64,
        expected_revision: u64,
    },
    SuspendWork {
        operation_id: String,
        handle: String,
        admitted_operation_id: String,
        reason: String,
        expected_revision: u64,
    },
    ResumeWork {
        operation_id: String,
        handle: String,
        admitted_operation_id: String,
        expected_revision: u64,
    },
    AckWorkReceipts {
        operation_id: String,
        handle: String,
        admitted_operation_id: String,
        through_sequence: u64,
        expected_revision: u64,
    },
    CancelWork {
        operation_id: String,
        handle: String,
        expected_revision: u64,
    },
    IssueOrder {
        operation_id: String,
        handles: Vec<String>,
        expected_order_revisions: Vec<u64>,
        order: ScriptNpcOrder,
    },
    CancelOrder {
        operation_id: String,
        handles: Vec<String>,
        expected_order_revisions: Vec<u64>,
    },
}

impl ScriptNpcOrderOperation {
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::Query { .. } => None,
            Self::UseItem { operation_id, .. }
            | Self::DeployItem { operation_id, .. }
            | Self::RetractItem { operation_id, .. }
            | Self::CaptureItem { operation_id, .. } => Some(operation_id),
            Self::AssignWork { operation_id, .. }
            | Self::SuspendWork { operation_id, .. }
            | Self::ResumeWork { operation_id, .. }
            | Self::AckWorkReceipts { operation_id, .. }
            | Self::CancelWork { operation_id, .. }
            | Self::IssueOrder { operation_id, .. }
            | Self::CancelOrder { operation_id, .. } => Some(operation_id),
        }
    }

    pub fn execution_permit(&self) -> Option<&crate::ScriptNpcExecutionPermit> {
        match self {
            Self::UseItem {
                execution_permit, ..
            }
            | Self::DeployItem {
                execution_permit, ..
            }
            | Self::RetractItem {
                execution_permit, ..
            }
            | Self::CaptureItem {
                execution_permit, ..
            } => execution_permit.as_ref(),
            _ => None,
        }
    }

    pub fn canonicalize(&mut self) {
        match self {
            Self::Query { handles } => {
                handles.sort_unstable();
                handles.dedup();
            }
            // A work order's fields are roles, not an unordered set: a haul is
            // directed, so nothing in it may be reordered. `Craft`, the work
            // area bounds and the tool names are already canonical.
            Self::IssueOrder {
                handles,
                expected_order_revisions,
                order,
                ..
            } => {
                order.canonicalize();
                canonicalize_handles(handles, expected_order_revisions, true);
            }
            Self::CancelOrder {
                handles,
                expected_order_revisions,
                ..
            } => canonicalize_handles(handles, expected_order_revisions, true),
            _ => {}
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(operation_id) = self.operation_id() {
            crate::validate_script_id_value(operation_id)?;
        }
        if let Some(permit) = self.execution_permit() {
            permit.validate()?;
        }
        match self {
            Self::Query { handles } => {
                if handles.is_empty() || handles.len() > MAX_NPC_ORDER_HANDLES {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut previous = None;
                for handle in handles {
                    crate::validate_npc_handle(handle)?;
                    if previous.is_some_and(|value: &str| value >= handle.as_str()) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "npc control handles",
                        });
                    }
                    previous = Some(handle.as_str());
                }
                Ok(())
            }
            Self::AssignWork {
                handle,
                work,
                work_units,
                expected_revision,
                ..
            } => {
                crate::validate_npc_handle(handle)?;
                work.validate()?;
                if *work_units == 0
                    || *work_units > MAX_WORK_UNITS
                    || *expected_revision > MAX_SCRIPT_WORLD_TIME
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::SuspendWork {
                handle,
                admitted_operation_id,
                reason,
                expected_revision,
                ..
            } => {
                crate::validate_npc_handle(handle)?;
                crate::validate_script_id_value(admitted_operation_id)?;
                crate::validate_script_id_value(reason)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::ResumeWork {
                handle,
                admitted_operation_id,
                expected_revision,
                ..
            }
            | Self::AckWorkReceipts {
                handle,
                admitted_operation_id,
                expected_revision,
                ..
            } => {
                crate::validate_npc_handle(handle)?;
                crate::validate_script_id_value(admitted_operation_id)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::UseItem {
                handle,
                equipment_slot,
                expected_revision,
                ..
            } => {
                crate::validate_npc_handle(handle)?;
                if *equipment_slot > 1 || *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::DeployItem {
                handle,
                equipment_slot,
                expected_revision,
                entity_uuid,
                support,
                position,
                heading_degrees,
                ..
            } => {
                crate::validate_npc_handle(handle)?;
                crate::validate_entity_uuid(entity_uuid)?;
                support.validate()?;
                validate_heading(*heading_degrees)?;
                if *equipment_slot > 1
                    || *expected_revision > MAX_SCRIPT_WORLD_TIME
                    || crate::ScriptPosition::try_new(position.x(), position.y(), position.z())
                        .is_none()
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::RetractItem {
                handle,
                equipment_slot,
                expected_revision,
                entity_uuid,
                ..
            } => {
                crate::validate_npc_handle(handle)?;
                crate::validate_entity_uuid(entity_uuid)?;
                if *equipment_slot > 1 || *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::CaptureItem {
                handle,
                expected_revision,
                entity_uuid,
                ..
            } => {
                crate::validate_npc_handle(handle)?;
                crate::validate_entity_uuid(entity_uuid)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::CancelWork {
                handle,
                expected_revision,
                ..
            } => {
                crate::validate_npc_handle(handle)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::IssueOrder {
                handles,
                expected_order_revisions,
                order,
                ..
            } => {
                validate_order_members(handles, expected_order_revisions)?;
                order.validate()
            }
            Self::CancelOrder {
                handles,
                expected_order_revisions,
                ..
            } => validate_order_members(handles, expected_order_revisions),
        }
    }
}

fn canonicalize_handles(
    handles: &mut Vec<String>,
    expected_order_revisions: &mut Vec<u64>,
    sort: bool,
) {
    if !sort {
        return;
    }
    let mut paired = handles
        .iter()
        .cloned()
        .zip(expected_order_revisions.iter().copied())
        .collect::<Vec<_>>();
    paired.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    paired.dedup_by(|left, right| left.0 == right.0);
    *handles = paired.iter().map(|(handle, _)| handle.clone()).collect();
    *expected_order_revisions = paired.into_iter().map(|(_, revision)| revision).collect();
}

fn validate_order_members(
    handles: &[String],
    expected_order_revisions: &[u64],
) -> Result<(), ScriptDtoError> {
    if handles.is_empty()
        || handles.len() > MAX_NPC_ORDER_HANDLES
        || handles.len() != expected_order_revisions.len()
    {
        return Err(ScriptDtoError::InvalidBounds);
    }
    let mut unique = BTreeSet::new();
    for (handle, revision) in handles.iter().zip(expected_order_revisions) {
        crate::validate_npc_handle(handle)?;
        if !unique.insert(handle.as_str()) {
            return Err(ScriptDtoError::DuplicateId {
                field: "order handle",
                actual_bytes: handle.len(),
            });
        }
        if *revision > MAX_SCRIPT_WORLD_TIME {
            return Err(ScriptDtoError::InvalidBounds);
        }
    }
    Ok(())
}

/// Closed pause reason for a work assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptWorkPauseReason {
    Unloaded,
    NoWorkers,
    MissingInput,
    MissingTool,
    MissingStation,
    BlockedRoute,
    Interrupted,
    Protected,
    Unsupported,
    NoStorage,
    ReceiptBackpressure,
}

impl ScriptWorkPauseReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unloaded => "unloaded",
            Self::NoWorkers => "no_workers",
            Self::MissingInput => "missing_input",
            Self::MissingTool => "missing_tool",
            Self::MissingStation => "missing_station",
            Self::BlockedRoute => "blocked_route",
            Self::Interrupted => "interrupted",
            Self::Protected => "protected",
            Self::Unsupported => "unsupported",
            Self::NoStorage => "no_storage",
            Self::ReceiptBackpressure => "receipt_backpressure",
        }
    }
}

/// Lifecycle state of one work assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptWorkState {
    Accepted,
    Running,
    Paused,
    Suspended,
    Committed,
    Cancelled,
}

impl ScriptWorkState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Suspended => "suspended",
            Self::Committed => "committed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One committed inventory change. Only real committed deltas are reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptItemChange {
    pub item_id: String,
    /// Signed change: positive produced, negative consumed.
    pub delta: i64,
}

impl ScriptItemChange {
    #[must_use]
    pub fn new(item_id: String, delta: i64) -> Self {
        Self { item_id, delta }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        check_contract_resource_id(&self.item_id)?;
        if self.delta == 0 || self.delta == i64::MIN {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// Exact canonical stack at one native inventory slot. Counts are gross, not net.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptCraftStack {
    pub endpoint: ScriptInventoryEndpoint,
    pub slot: u8,
    pub item: crate::ScriptInventoryItem,
}

impl ScriptCraftStack {
    #[must_use]
    pub fn new(
        endpoint: ScriptInventoryEndpoint,
        slot: u8,
        item: crate::ScriptInventoryItem,
    ) -> Self {
        Self {
            endpoint,
            slot,
            item,
        }
    }
}

/// Actual durability transition of one physical tool, including breakage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptCraftToolWear {
    pub before: ScriptCraftStack,
    pub after: Option<crate::ScriptInventoryItem>,
    pub damage: u32,
}

impl ScriptCraftToolWear {
    #[must_use]
    pub fn new(
        before: ScriptCraftStack,
        after: Option<crate::ScriptInventoryItem>,
        damage: u32,
    ) -> Self {
        Self {
            before,
            after,
            damage,
        }
    }
}

/// A committed craft portion. Replaying/querying this record never crafts again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptCraftReceipt {
    pub operation_id: String,
    pub handle: String,
    pub portion_id: String,
    pub sequence: u64,
    pub recipe: String,
    pub recipe_revision: String,
    pub seed: u64,
    pub simulation_tick: u64,
    pub duration_ticks: u64,
    pub station: ScriptWorkArea,
    pub inputs: Vec<ScriptCraftStack>,
    pub outputs: Vec<ScriptCraftStack>,
    pub wear: Vec<ScriptCraftToolWear>,
    pub before: Vec<crate::ScriptInventoryExpectedRevision>,
    pub after: Vec<crate::ScriptInventoryExpectedRevision>,
    pub revision: u64,
}

impl ScriptCraftReceipt {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        operation_id: String,
        handle: String,
        portion_id: String,
        sequence: u64,
        recipe: String,
        recipe_revision: String,
        seed: u64,
        simulation_tick: u64,
        duration_ticks: u64,
        station: ScriptWorkArea,
        inputs: Vec<ScriptCraftStack>,
        outputs: Vec<ScriptCraftStack>,
        wear: Vec<ScriptCraftToolWear>,
        before: Vec<crate::ScriptInventoryExpectedRevision>,
        after: Vec<crate::ScriptInventoryExpectedRevision>,
        revision: u64,
    ) -> Self {
        Self {
            operation_id,
            handle,
            portion_id,
            sequence,
            recipe,
            recipe_revision,
            seed,
            simulation_tick,
            duration_ticks,
            station,
            inputs,
            outputs,
            wear,
            before,
            after,
            revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_script_id_value(&self.operation_id)?;
        crate::validate_script_id_value(&self.portion_id)?;
        crate::validate_npc_handle(&self.handle)?;
        check_contract_resource_id(&self.recipe)?;
        validate_bounded_nonempty("recipe revision", &self.recipe_revision, 64)?;
        self.station.validate()?;
        if self.sequence == 0
            || self.duration_ticks == 0
            || self.inputs.is_empty()
            || self.outputs.is_empty()
            || self.inputs.len() > 64
            || self.outputs.len() > 64
            || self.wear.len() > 16
            || self.before.len() != 2
            || self.after.len() != 2
            || self.simulation_tick > MAX_SCRIPT_WORLD_TIME
            || self.revision > MAX_SCRIPT_WORLD_TIME
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        for stack in self
            .inputs
            .iter()
            .chain(&self.outputs)
            .chain(self.wear.iter().map(|wear| &wear.before))
        {
            stack.endpoint.validate()?;
            stack.item.validate()?;
            let valid_slot = match &stack.endpoint {
                ScriptInventoryEndpoint::NpcEquipment { handle } => {
                    handle == &self.handle && stack.slot < crate::MAX_NPC_EQUIPMENT_SLOTS
                }
                ScriptInventoryEndpoint::NpcCarry { handle } => {
                    handle == &self.handle && stack.slot < crate::MAX_NPC_CARRY_SLOTS
                }
                _ => false,
            };
            if !valid_slot {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        for wear in &self.wear {
            if wear.damage == 0 {
                return Err(ScriptDtoError::InvalidBounds);
            }
            if let Some(after) = &wear.after {
                after.validate()?;
            }
        }
        for fence in self.before.iter().chain(&self.after) {
            fence.endpoint.validate()?;
            fence.fence.validate()?;
        }
        Ok(())
    }
}

/// One work assignment result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorkAssignment {
    pub handle: String,
    /// Original admitted work identity, unchanged by native resume requests.
    pub operation_id: Option<String>,
    pub receipts: Vec<ScriptCraftReceipt>,
    pub acknowledged_sequence: u64,
    /// Native causal deadline for the next portion, never plugin elapsed time.
    pub due_tick: Option<u64>,
    pub state: ScriptWorkState,
    pub reason: Option<ScriptWorkPauseReason>,
    /// Opaque owner reason while explicitly suspended; never native product policy.
    pub suspension_reason: Option<String>,
    pub work_units_done: u64,
    pub work_units_planned: u64,
    pub changes: Vec<ScriptItemChange>,
    pub revision: u64,
}

impl ScriptWorkAssignment {
    #[must_use]
    pub fn new(
        handle: String,
        state: ScriptWorkState,
        reason: Option<ScriptWorkPauseReason>,
        work_units_done: u64,
        work_units_planned: u64,
        mut changes: Vec<ScriptItemChange>,
        revision: u64,
    ) -> Self {
        changes.sort_unstable_by(|left, right| left.item_id.cmp(&right.item_id));
        Self {
            handle,
            operation_id: None,
            receipts: Vec::new(),
            acknowledged_sequence: 0,
            due_tick: None,
            state,
            reason,
            suspension_reason: None,
            work_units_done,
            work_units_planned,
            changes,
            revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_npc_handle(&self.handle)?;
        if let Some(operation_id) = &self.operation_id {
            crate::validate_script_id_value(operation_id)?;
        }
        if let Some(reason) = &self.suspension_reason {
            crate::validate_script_id_value(reason)?;
        }
        if self.receipts.len() > MAX_WORK_RECEIPTS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut sequence = self.acknowledged_sequence;
        for receipt in &self.receipts {
            receipt.validate()?;
            if receipt.sequence <= sequence
                || self.operation_id.as_deref() != Some(receipt.operation_id.as_str())
                || receipt.sequence > self.work_units_done
                || receipt.handle != self.handle
            {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "work receipts",
                });
            }
            sequence = receipt.sequence;
        }
        if self.revision > MAX_SCRIPT_WORLD_TIME
            || self.work_units_planned == 0
            || self.work_units_planned > MAX_WORK_UNITS
            || self.work_units_done > self.work_units_planned
            || self.acknowledged_sequence > self.work_units_done
            || self
                .due_tick
                .is_some_and(|tick| tick > MAX_SCRIPT_WORLD_TIME)
            || (self.state == ScriptWorkState::Paused) != self.reason.is_some()
            || (self.state == ScriptWorkState::Suspended) != self.suspension_reason.is_some()
            || (self.state == ScriptWorkState::Suspended
                && (self.operation_id.is_none()
                    || self.due_tick.is_some()
                    || self.work_units_done >= self.work_units_planned))
            || self.changes.len() > MAX_ORDER_TARGETS
        {
            return Err(ScriptDtoError::InconsistentResult {
                field: "work assignment",
            });
        }
        for change in &self.changes {
            change.validate()?;
        }
        Ok(())
    }
}

/// Closed observable state of one squad member after an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptOrderMemberState {
    Applied,
    BlockedRoute,
    Unloaded,
    Dead,
    Migrating,
    Forbidden,
    StaleRevision,
}

impl ScriptOrderMemberState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::BlockedRoute => "blocked_route",
            Self::Unloaded => "unloaded",
            Self::Dead => "dead",
            Self::Migrating => "migrating",
            Self::Forbidden => "forbidden",
            Self::StaleRevision => "stale_revision",
        }
    }
}

/// One server-issued target reference surfaced to the plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptOrderTarget {
    pub target_ref: String,
    #[serde(default)]
    pub policy_revision: u64,
    #[serde(default)]
    pub expires_revision: u64,
    pub category: ScriptHostileCategory,
    pub position: ScriptBlockPosition,
}

impl ScriptOrderTarget {
    #[must_use]
    pub fn new(
        target_ref: String,
        policy_revision: u64,
        expires_revision: u64,
        category: ScriptHostileCategory,
        position: ScriptBlockPosition,
    ) -> Self {
        Self {
            target_ref,
            policy_revision,
            expires_revision,
            category,
            position,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("target ref", &self.target_ref, MAX_TARGET_REF_BYTES)?;
        if self.policy_revision > MAX_SCRIPT_WORLD_TIME
            || self.expires_revision > MAX_SCRIPT_WORLD_TIME
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        self.position.validate()
    }
}

/// One member's observable outcome after a squad order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptOrderMemberOutcome {
    pub handle: String,
    pub state: ScriptOrderMemberState,
    pub formation_slot: Option<u16>,
    pub targets: Vec<ScriptOrderTarget>,
}

impl ScriptOrderMemberOutcome {
    #[must_use]
    pub fn new(
        handle: String,
        state: ScriptOrderMemberState,
        formation_slot: Option<u16>,
        mut targets: Vec<ScriptOrderTarget>,
    ) -> Self {
        targets.sort_unstable_by(|left, right| left.target_ref.cmp(&right.target_ref));
        Self {
            handle,
            state,
            formation_slot,
            targets,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_npc_handle(&self.handle)?;
        if (self.state == ScriptOrderMemberState::Applied) != self.formation_slot.is_some() {
            return Err(ScriptDtoError::InconsistentResult {
                field: "order member outcome",
            });
        }
        if self.targets.len() > MAX_ORDER_TARGETS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "order member targets",
                max: MAX_ORDER_TARGETS,
            });
        }
        for target in &self.targets {
            target.validate()?;
        }
        Ok(())
    }
}

/// Physical observation of a server-issued target reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcTargetObservation {
    pub target_ref: String,
    pub position: Option<crate::ScriptPosition>,
    pub alive: Option<bool>,
}

impl ScriptNpcTargetObservation {
    #[must_use]
    pub fn new(
        target_ref: String,
        position: Option<crate::ScriptPosition>,
        alive: Option<bool>,
    ) -> Self {
        Self {
            target_ref,
            position,
            alive,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("target ref", &self.target_ref, MAX_TARGET_REF_BYTES)?;
        if self.position.is_some() != self.alive.is_some() {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(position) = self.position
            && crate::ScriptPosition::try_new(position.x(), position.y(), position.z()).is_none()
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// The remaining canonical physical work request, never completed units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcPendingWork {
    pub work: ScriptNpcWorkOrder,
    pub work_units: u64,
}

impl ScriptNpcPendingWork {
    #[must_use]
    pub fn new(work: ScriptNpcWorkOrder, work_units: u64) -> Self {
        Self { work, work_units }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcItemUseControl {
    pub equipment_slot: Option<u8>,
    pub started_tick: Option<u64>,
    pub disabled_until: u64,
}

impl ScriptNpcItemUseControl {
    #[must_use]
    pub fn new(equipment_slot: Option<u8>, started_tick: Option<u64>, disabled_until: u64) -> Self {
        Self {
            equipment_slot,
            started_tick,
            disabled_until,
        }
    }
}

/// Honest native control progress, never a substitute for a committed damage event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcCombatControl {
    pub operation_id: String,
    pub started_tick: u64,
    pub ready_tick: u64,
    pub failure: Option<crate::ScriptOperationFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event: Option<Box<ScriptCombatEvent>>,
}

impl ScriptNpcCombatControl {
    #[must_use]
    pub fn new(
        operation_id: String,
        started_tick: u64,
        ready_tick: u64,
        failure: Option<crate::ScriptOperationFailure>,
        last_event: Option<Box<ScriptCombatEvent>>,
    ) -> Self {
        Self {
            operation_id,
            started_tick,
            ready_tick,
            failure,
            last_event,
        }
    }
}

/// Native control fences and accepted payloads for one owned NPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcControlSnapshot {
    pub handle: String,
    pub revision: u64,
    pub order_revision: u64,
    pub order: Option<ScriptNpcOrder>,
    pub work: Option<ScriptWorkAssignment>,
    pub pending_work: Option<ScriptNpcPendingWork>,
    pub death_cargo: Option<Box<crate::ScriptNpcDeathCargoReceipt>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combat: Option<Box<ScriptNpcCombatControl>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_use: Option<ScriptNpcItemUseControl>,
    pub targets: Vec<ScriptNpcTargetObservation>,
}

impl ScriptNpcControlSnapshot {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        handle: String,
        revision: u64,
        order_revision: u64,
        order: Option<ScriptNpcOrder>,
        work: Option<ScriptWorkAssignment>,
        pending_work: Option<ScriptNpcPendingWork>,
        death_cargo: Option<Box<crate::ScriptNpcDeathCargoReceipt>>,
        combat: Option<Box<ScriptNpcCombatControl>>,
        item_use: Option<ScriptNpcItemUseControl>,
        targets: Vec<ScriptNpcTargetObservation>,
    ) -> Self {
        Self {
            handle,
            revision,
            order_revision,
            order,
            work,
            pending_work,
            death_cargo,
            combat,
            item_use,
            targets,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_npc_handle(&self.handle)?;
        if self.revision > MAX_SCRIPT_WORLD_TIME
            || self.order_revision > self.revision
            || self.order.is_none() != (self.order_revision == 0)
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(combat) = &self.combat {
            validate_bounded_nonempty(
                "combat operation",
                &combat.operation_id,
                crate::MAX_SCRIPT_ID_BYTES,
            )?;
            if combat.started_tick > MAX_SCRIPT_WORLD_TIME
                || combat.ready_tick > MAX_SCRIPT_WORLD_TIME
                || combat.ready_tick < combat.started_tick
            {
                return Err(ScriptDtoError::InvalidBounds);
            }
            if let Some(event) = &combat.last_event {
                event.validate()?;
                if event.attacker_handle != self.handle
                    || event.revision > self.revision
                    || event.order_revision == 0
                    || event.order_revision > event.revision
                {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "npc control combat",
                    });
                }
            }
        }
        if let Some(item_use) = &self.item_use
            && (item_use.equipment_slot.is_some() != item_use.started_tick.is_some()
                || item_use.equipment_slot.is_some_and(|slot| slot > 1)
                || item_use
                    .started_tick
                    .is_some_and(|tick| tick > MAX_SCRIPT_WORLD_TIME)
                || item_use.disabled_until > MAX_SCRIPT_WORLD_TIME)
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(order) = &self.order {
            order.validate()?;
        }
        if let Some(work) = &self.work {
            work.validate()?;
            if work.handle != self.handle || work.revision > self.revision {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "npc control work",
                });
            }
        }
        if let Some(receipt) = &self.death_cargo {
            receipt.validate()?;
            if receipt.handle != self.handle
                || receipt.source_inventory_revision > self.revision
                || self.order.is_some()
                || self.pending_work.is_some()
                || self
                    .work
                    .as_ref()
                    .is_some_and(|work| work.state != ScriptWorkState::Cancelled)
            {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "npc death cargo",
                });
            }
        }
        if let Some(pending) = &self.pending_work {
            pending.work.validate()?;
            validate_work_units(pending.work_units)?;
            let Some(work) = &self.work else {
                return Err(ScriptDtoError::InvalidBounds);
            };
            if pending.work_units > work.work_units_planned - work.work_units_done
                || !matches!(
                    work.state,
                    ScriptWorkState::Accepted
                        | ScriptWorkState::Running
                        | ScriptWorkState::Paused
                        | ScriptWorkState::Suspended
                )
            {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        if self.targets.len() > MAX_ORDER_TARGETS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut refs = BTreeSet::new();
        for target in &self.targets {
            target.validate()?;
            if !refs.insert(target.target_ref.as_str()) {
                return Err(ScriptDtoError::DuplicateId {
                    field: "control target",
                    actual_bytes: target.target_ref.len(),
                });
            }
        }
        Ok(())
    }
}

/// Closed typed payload for one committed npc work/order call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptNpcOrderResult {
    ItemUsed {
        handle: String,
        revision: u64,
    },
    ItemDeployed {
        handle: String,
        entity_uuid: String,
        revision: u64,
    },
    ItemRetracted {
        handle: String,
        entity_uuid: String,
        revision: u64,
    },
    ItemCaptured {
        handle: String,
        entity_uuid: String,
        revision: u64,
    },
    Controls {
        npcs: Vec<ScriptNpcControlSnapshot>,
    },
    Work {
        assignment: Box<ScriptWorkAssignment>,
    },
    WorkCancelled {
        handle: String,
        revision: u64,
    },
    Order {
        order_revision: u64,
        members: Vec<ScriptOrderMemberOutcome>,
        /// Committed combat outcomes correlated to this order revision, so the
        /// plugin awards experience exactly once per event id.
        combat: Vec<ScriptCombatEvent>,
    },
    OrderCancelled {
        order_revision: u64,
        members: Vec<ScriptOrderMemberOutcome>,
    },
}

impl ScriptNpcOrderResult {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::ItemUsed { handle, revision } => {
                crate::validate_npc_handle(handle)?;
                if *revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::ItemDeployed {
                handle,
                entity_uuid,
                revision,
            }
            | Self::ItemRetracted {
                handle,
                entity_uuid,
                revision,
            }
            | Self::ItemCaptured {
                handle,
                entity_uuid,
                revision,
            } => {
                crate::validate_npc_handle(handle)?;
                crate::validate_entity_uuid(entity_uuid)?;
                if *revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Controls { npcs } => {
                if npcs.len() > MAX_NPC_ORDER_HANDLES {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut previous = None;
                for npc in npcs {
                    npc.validate()?;
                    if previous.is_some_and(|value: &str| value >= npc.handle.as_str()) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "npc control page",
                        });
                    }
                    previous = Some(npc.handle.as_str());
                }
                Ok(())
            }
            Self::Work { assignment } => assignment.validate(),
            Self::WorkCancelled { handle, revision } => {
                crate::validate_npc_handle(handle)?;
                if *revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Order {
                members, combat, ..
            } => {
                validate_order_member_outcomes(members)?;
                if combat.len() > MAX_COMBAT_EVENTS {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "combat events",
                        max: MAX_COMBAT_EVENTS,
                    });
                }
                for event in combat {
                    event.validate()?;
                }
                Ok(())
            }
            Self::OrderCancelled { members, .. } => validate_order_member_outcomes(members),
        }
    }
}

/// One committed combat outcome. Damage is in thousandths of a health point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptCombatEvent {
    pub event_id: u64,
    pub revision: u64,
    pub attacker_handle: String,
    pub victim_target_ref: String,
    pub order_revision: u64,
    pub damage_milli: u32,
    pub killed: bool,
}

impl ScriptCombatEvent {
    #[must_use]
    pub fn new(
        event_id: u64,
        revision: u64,
        attacker_handle: String,
        victim_target_ref: String,
        order_revision: u64,
        damage_milli: u32,
        killed: bool,
    ) -> Self {
        Self {
            event_id,
            revision,
            attacker_handle,
            victim_target_ref,
            order_revision,
            damage_milli,
            killed,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_npc_handle(&self.attacker_handle)?;
        validate_bounded_nonempty("target ref", &self.victim_target_ref, MAX_TARGET_REF_BYTES)?;
        if self.event_id == 0
            || self.event_id > MAX_SCRIPT_WORLD_TIME
            || self.revision > MAX_SCRIPT_WORLD_TIME
            || self.order_revision > MAX_SCRIPT_WORLD_TIME
            || self.damage_milli == 0
            || self.damage_milli > u32::try_from(MAX_WORK_UNITS * 1000).unwrap_or(u32::MAX)
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// Validate a bounded, sorted, strictly increasing member list.
fn validate_order_member_outcomes(
    members: &[ScriptOrderMemberOutcome],
) -> Result<(), ScriptDtoError> {
    if members.len() > MAX_NPC_ORDER_HANDLES {
        return Err(ScriptDtoError::TooManyEntries {
            field: "order members",
            max: MAX_NPC_ORDER_HANDLES,
        });
    }
    let mut previous = None;
    for member in members {
        member.validate()?;
        if previous.is_some_and(|handle: &str| handle >= member.handle.as_str()) {
            return Err(ScriptDtoError::InconsistentResult {
                field: "order member order",
            });
        }
        previous = Some(member.handle.as_str());
    }
    Ok(())
}

/// Validate one bounded work unit budget for an assignment.
pub fn validate_work_units(work_units: u64) -> Result<(), ScriptDtoError> {
    if work_units == 0 || work_units > MAX_WORK_UNITS {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

/// Validate one bounded work-area axis extent.
pub fn validate_work_area_axis(extent: u32) -> Result<(), ScriptDtoError> {
    if extent == 0 || extent > MAX_WORK_AREA_AXIS {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

/// Validate a bounded recipe identifier.
pub fn validate_recipe(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_value("recipe", value, MAX_RECIPE_ID_BYTES)
}
