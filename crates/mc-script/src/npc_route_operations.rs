//! Finite native routes and exclusive physical executor ownership.
//!
//! Admission is not movement or arrival. Positions, geometry witnesses, epochs,
//! handoff snapshots and receipts are supplied only by the native actor owner.

use serde::{Deserialize, Serialize};

use crate::{
    MAX_SCRIPT_WORLD_TIME, ScriptBlockPosition, ScriptDtoError, ScriptNpcLifecycle,
    ScriptNpcLoadedState, validate_bounded_nonempty, validate_npc_handle,
};

pub const MAX_NPC_ROUTE_WAYPOINTS: usize = 16;
pub const MAX_NPC_ROUTE_SEGMENT_CELLS: u32 = 128;
pub const MAX_NPC_ROUTE_TICKS: u64 = 72_000;
pub const MAX_NPC_ROUTE_RESERVATIONS: usize = 16;
pub const MAX_NPC_ROUTE_REF_BYTES: usize = 64;

/// Both executing modes use the same native collision/navigation and actor.
/// Remote physical execution retains bounded world tickets, not a projected pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptNpcExecutorMode {
    ObservedPhysical,
    RemotePhysical,
    Parked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptNpcRouteState {
    Prepared,
    Running,
    Blocked,
    HandoffPending,
    Parked,
    Arrived,
    Cancelled,
    Dead,
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptNpcRouteBlockReason {
    Unloaded,
    StaleGeometry,
    BlockedRoute,
    ForeignProtection,
    MissingActor,
    StaleIdentity,
    StaleOrder,
    ExecutorUnavailable,
}

/// A caller fences the actual actor identity and existing native order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcRouteAdmission {
    pub handle: String,
    pub identity_revision: u64,
    pub order_revision: u64,
    pub dimension: String,
    pub waypoints: Vec<ScriptBlockPosition>,
    pub max_ticks: u64,
    /// Existing native reservation references authenticated at admission.
    pub reservation_refs: Vec<String>,
}

impl ScriptNpcRouteAdmission {
    #[must_use]
    pub fn new(
        handle: String,
        identity_revision: u64,
        order_revision: u64,
        dimension: String,
        waypoints: Vec<ScriptBlockPosition>,
        max_ticks: u64,
        reservation_refs: Vec<String>,
    ) -> Self {
        Self {
            handle,
            identity_revision,
            order_revision,
            dimension,
            waypoints,
            max_ticks,
            reservation_refs,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_npc_handle(&self.handle)?;
        crate::check_contract_resource_id(&self.dimension)?;
        if self.identity_revision == 0
            || self.identity_revision > MAX_SCRIPT_WORLD_TIME
            || self.order_revision > MAX_SCRIPT_WORLD_TIME
            || self.waypoints.is_empty()
            || self.waypoints.len() > MAX_NPC_ROUTE_WAYPOINTS
            || self.max_ticks == 0
            || self.max_ticks > MAX_NPC_ROUTE_TICKS
            || self.reservation_refs.len() > MAX_NPC_ROUTE_RESERVATIONS
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        for point in &self.waypoints {
            point.validate()?;
        }
        for pair in self.waypoints.windows(2) {
            let distance = i64::from(pair[0].x)
                .abs_diff(i64::from(pair[1].x))
                .max(i64::from(pair[0].y).abs_diff(i64::from(pair[1].y)))
                .max(i64::from(pair[0].z).abs_diff(i64::from(pair[1].z)));
            if distance > u64::from(MAX_NPC_ROUTE_SEGMENT_CELLS) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        let mut unique = std::collections::BTreeSet::new();
        for reference in &self.reservation_refs {
            validate_route_ref(reference)?;
            if !unique.insert(reference) {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}

/// Required when a mutation touches an actor with exclusive route authority.
/// An arrival receipt authorizes terminal unloading, never guessed proximity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcExecutionPermit {
    pub route_ref: String,
    pub executor_epoch: u64,
}

impl ScriptNpcExecutionPermit {
    pub fn new(route_ref: String, executor_epoch: u64) -> Self {
        Self {
            route_ref,
            executor_epoch,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_route_ref(&self.route_ref)?;
        validate_epoch(self.executor_epoch)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptNpcRouteOperation {
    Admit {
        operation_id: String,
        admission: ScriptNpcRouteAdmission,
    },
    /// Escrow/cargo routes require their real canonical transfer receipt.
    Start {
        operation_id: String,
        route_ref: String,
        expected_epoch: u64,
        cargo_operation_id: Option<String>,
    },
    Query {
        route_ref: String,
    },
    Handoff {
        operation_id: String,
        route_ref: String,
        expected_epoch: u64,
        mode: ScriptNpcExecutorMode,
    },
    Cancel {
        operation_id: String,
        route_ref: String,
        expected_epoch: u64,
    },
}

impl ScriptNpcRouteOperation {
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::Admit { operation_id, .. }
            | Self::Start { operation_id, .. }
            | Self::Handoff { operation_id, .. }
            | Self::Cancel { operation_id, .. } => Some(operation_id),
            Self::Query { .. } => None,
        }
    }
    pub fn canonicalize(&mut self) {} // Waypoint order is physical route order.
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(id) = self.operation_id() {
            crate::validate_script_id_value(id)?;
        }
        match self {
            Self::Admit { admission, .. } => admission.validate(),
            Self::Query { route_ref } => validate_route_ref(route_ref),
            Self::Start {
                route_ref,
                expected_epoch,
                cargo_operation_id,
                ..
            } => {
                validate_route_ref(route_ref)?;
                validate_epoch(*expected_epoch)?;
                cargo_operation_id
                    .as_deref()
                    .map_or(Ok(()), crate::validate_script_id_value)
            }
            Self::Handoff {
                route_ref,
                expected_epoch,
                ..
            }
            | Self::Cancel {
                route_ref,
                expected_epoch,
                ..
            } => {
                validate_route_ref(route_ref)?;
                validate_epoch(*expected_epoch)
            }
        }
    }
}

/// Immutable proof of a native owner transition, not a second inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcRouteHandoff {
    pub snapshot_ref: String,
    pub source_epoch: u64,
    pub target_epoch: u64,
    pub simulation_tick: u64,
    pub dimension: String,
    pub pose: ScriptNpcLoadedState,
    pub inventory_revision: u64,
    pub inventory_image_hash: String,
    pub geometry_hash: String,
    pub reservation_refs: Vec<String>,
}

impl ScriptNpcRouteHandoff {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        snapshot_ref: String,
        source_epoch: u64,
        target_epoch: u64,
        simulation_tick: u64,
        dimension: String,
        pose: ScriptNpcLoadedState,
        inventory_revision: u64,
        inventory_image_hash: String,
        geometry_hash: String,
        reservation_refs: Vec<String>,
    ) -> Self {
        Self {
            snapshot_ref,
            source_epoch,
            target_epoch,
            simulation_tick,
            dimension,
            pose,
            inventory_revision,
            inventory_image_hash,
            geometry_hash,
            reservation_refs,
        }
    }
}

/// Exact arrival at the final stance after native motion and collision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcRouteArrival {
    pub receipt_ref: String,
    pub executor_epoch: u64,
    pub simulation_tick: u64,
    pub dimension: String,
    pub pose: ScriptNpcLoadedState,
    pub inventory_revision: u64,
    pub inventory_image_hash: String,
}

impl ScriptNpcRouteArrival {
    #[must_use]
    pub fn new(
        receipt_ref: String,
        executor_epoch: u64,
        simulation_tick: u64,
        dimension: String,
        pose: ScriptNpcLoadedState,
        inventory_revision: u64,
        inventory_image_hash: String,
    ) -> Self {
        Self {
            receipt_ref,
            executor_epoch,
            simulation_tick,
            dimension,
            pose,
            inventory_revision,
            inventory_image_hash,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptNpcRouteStatus {
    pub route_ref: String,
    pub handle: String,
    pub entity_uuid: String,
    pub entity_id: Option<i32>,
    pub identity_revision: u64,
    pub order_revision: u64,
    pub revision: u64,
    pub executor_epoch: u64,
    pub mode: ScriptNpcExecutorMode,
    pub state: ScriptNpcRouteState,
    pub lifecycle: ScriptNpcLifecycle,
    /// None means there is no currently authoritative loaded pose.
    pub pose: Option<ScriptNpcLoadedState>,
    pub segment: u16,
    /// Native completed route segments, not admitted Move commands.
    pub cursor: u16,
    pub blocked_reason: Option<ScriptNpcRouteBlockReason>,
    pub due_tick: Option<u64>,
    pub native_ticks: u64,
    pub geometry_cells: u32,
    pub geometry_hash: String,
    pub max_ticks: u64,
    pub handoff: Option<ScriptNpcRouteHandoff>,
    pub arrival: Option<ScriptNpcRouteArrival>,
    pub death_cargo: Option<Box<crate::ScriptNpcDeathCargoReceipt>>,
    pub outcome_tick: Option<u64>,
}

impl ScriptNpcRouteStatus {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        route_ref: String,
        handle: String,
        entity_uuid: String,
        entity_id: Option<i32>,
        identity_revision: u64,
        order_revision: u64,
        revision: u64,
        executor_epoch: u64,
        mode: ScriptNpcExecutorMode,
        state: ScriptNpcRouteState,
        lifecycle: ScriptNpcLifecycle,
        pose: Option<ScriptNpcLoadedState>,
        segment: u16,
        cursor: u16,
        blocked_reason: Option<ScriptNpcRouteBlockReason>,
        due_tick: Option<u64>,
        native_ticks: u64,
        geometry_cells: u32,
        geometry_hash: String,
        max_ticks: u64,
        handoff: Option<ScriptNpcRouteHandoff>,
        arrival: Option<ScriptNpcRouteArrival>,
        death_cargo: Option<Box<crate::ScriptNpcDeathCargoReceipt>>,
        outcome_tick: Option<u64>,
    ) -> Self {
        Self {
            route_ref,
            handle,
            entity_uuid,
            entity_id,
            identity_revision,
            order_revision,
            revision,
            executor_epoch,
            mode,
            state,
            lifecycle,
            pose,
            segment,
            cursor,
            blocked_reason,
            due_tick,
            native_ticks,
            geometry_cells,
            geometry_hash,
            max_ticks,
            handoff,
            arrival,
            death_cargo,
            outcome_tick,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_route_ref(&self.route_ref)?;
        validate_npc_handle(&self.handle)?;
        crate::validate_entity_uuid(&self.entity_uuid)?;
        validate_epoch(self.executor_epoch)?;
        validate_route_ref(&self.geometry_hash)?;
        if self.identity_revision == 0
            || self.revision > MAX_SCRIPT_WORLD_TIME
            || self.identity_revision > MAX_SCRIPT_WORLD_TIME
            || self.order_revision > MAX_SCRIPT_WORLD_TIME
            || self.cursor > MAX_NPC_ROUTE_WAYPOINTS as u16
            || self.segment > MAX_NPC_ROUTE_WAYPOINTS as u16
            || self.max_ticks == 0
            || self.max_ticks > MAX_NPC_ROUTE_TICKS
            || self.native_ticks > self.max_ticks
            || self
                .due_tick
                .is_some_and(|tick| tick > MAX_SCRIPT_WORLD_TIME)
            || self
                .outcome_tick
                .is_some_and(|tick| tick > MAX_SCRIPT_WORLD_TIME)
            || (self.state == ScriptNpcRouteState::Arrived && self.arrival.is_none())
            || (self.state == ScriptNpcRouteState::Blocked) != self.blocked_reason.is_some()
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(pose) = &self.pose {
            pose.validate()?;
        }
        if let Some(handoff) = &self.handoff {
            validate_route_ref(&handoff.snapshot_ref)?;
            validate_epoch(handoff.source_epoch)?;
            validate_epoch(handoff.target_epoch)?;
            handoff.pose.validate()?;
            if handoff.target_epoch <= handoff.source_epoch
                || handoff.target_epoch > self.executor_epoch
                || handoff.dimension != handoff.pose.dimension
                || handoff.simulation_tick > MAX_SCRIPT_WORLD_TIME
                || handoff.reservation_refs.len() > MAX_NPC_ROUTE_RESERVATIONS
            {
                return Err(ScriptDtoError::InvalidBounds);
            }
            validate_route_ref(&handoff.geometry_hash)?;
            validate_route_ref(&handoff.inventory_image_hash)?;
            for reference in &handoff.reservation_refs {
                validate_route_ref(reference)?;
            }
        }
        if let Some(arrival) = &self.arrival {
            validate_route_ref(&arrival.receipt_ref)?;
            arrival.pose.validate()?;
            validate_route_ref(&arrival.inventory_image_hash)?;
            if arrival.executor_epoch > self.executor_epoch
                || arrival.dimension != arrival.pose.dimension
                || arrival.simulation_tick > MAX_SCRIPT_WORLD_TIME
            {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        if let Some(receipt) = &self.death_cargo {
            receipt.validate()?;
            if self.state != ScriptNpcRouteState::Dead
                || receipt.handle != self.handle
                || receipt.entity_uuid != self.entity_uuid
                || receipt.executor_epoch != Some(self.executor_epoch)
            {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "route death custody",
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptNpcRouteResult {
    Status { route: Box<ScriptNpcRouteStatus> },
}

impl ScriptNpcRouteResult {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Status { route } => route.validate(),
        }
    }
}

pub fn validate_route_ref(reference: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_nonempty("native route reference", reference, MAX_NPC_ROUTE_REF_BYTES)
}

fn validate_epoch(epoch: u64) -> Result<(), ScriptDtoError> {
    if epoch == 0 || epoch > MAX_SCRIPT_WORLD_TIME {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}
