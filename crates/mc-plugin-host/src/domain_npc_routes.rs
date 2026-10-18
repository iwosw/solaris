//! Lossless finite-route DTO conversion. Native authority owns all validation,
//! physical observations, epochs and receipt production.

use crate::bindings::solaris::plugin::{npc_routes as wire, npcs};
use mc_script::{
    ScriptNpcExecutorMode as Mode, ScriptNpcLifecycle, ScriptNpcLoadedState,
    ScriptNpcRouteBlockReason as Reason, ScriptNpcRouteOperation, ScriptNpcRouteResult,
    ScriptNpcRouteState as State, ScriptNpcRouteStatus,
};

pub(crate) fn decode(
    operation: wire::NpcRouteOperation,
) -> Result<ScriptNpcRouteOperation, mc_script::ScriptDtoError> {
    Ok(match operation {
        wire::NpcRouteOperation::Admit(value) => ScriptNpcRouteOperation::Admit {
            operation_id: value.operation_id,
            admission: mc_script::ScriptNpcRouteAdmission::new(
                value.admission.handle,
                value.admission.identity_revision,
                value.admission.order_revision,
                value.admission.dimension,
                value
                    .admission
                    .waypoints
                    .into_iter()
                    .map(|point| mc_script::ScriptBlockPosition::new(point.x, point.y, point.z))
                    .collect(),
                value.admission.max_ticks,
                value.admission.reservation_refs,
            ),
        },
        wire::NpcRouteOperation::Start(value) => ScriptNpcRouteOperation::Start {
            operation_id: value.operation_id,
            route_ref: value.route_ref,
            expected_epoch: value.expected_epoch,
            cargo_operation_id: value.cargo_operation_id,
        },
        wire::NpcRouteOperation::Query(value) => ScriptNpcRouteOperation::Query {
            route_ref: value.route_ref,
        },
        wire::NpcRouteOperation::Handoff(value) => ScriptNpcRouteOperation::Handoff {
            operation_id: value.operation_id,
            route_ref: value.route_ref,
            expected_epoch: value.expected_epoch,
            mode: match value.mode {
                wire::NpcExecutorMode::ObservedPhysical => Mode::ObservedPhysical,
                wire::NpcExecutorMode::RemotePhysical => Mode::RemotePhysical,
                wire::NpcExecutorMode::Parked => Mode::Parked,
            },
        },
        wire::NpcRouteOperation::Cancel(value) => ScriptNpcRouteOperation::Cancel {
            operation_id: value.operation_id,
            route_ref: value.route_ref,
            expected_epoch: value.expected_epoch,
        },
    })
}

pub(crate) fn encode_result(result: &ScriptNpcRouteResult) -> Option<wire::NpcRouteResult> {
    match result {
        ScriptNpcRouteResult::Status { route } => {
            Some(wire::NpcRouteResult::Status(status(route)?))
        }
        _ => None,
    }
}

fn status(value: &ScriptNpcRouteStatus) -> Option<wire::NpcRouteStatus> {
    Some(wire::NpcRouteStatus {
        route_ref: value.route_ref.clone(),
        handle: value.handle.clone(),
        entity_uuid: value.entity_uuid.clone(),
        entity_id: value.entity_id,
        identity_revision: value.identity_revision,
        order_revision: value.order_revision,
        revision: value.revision,
        executor_epoch: value.executor_epoch,
        mode: match value.mode {
            Mode::ObservedPhysical => wire::NpcExecutorMode::ObservedPhysical,
            Mode::RemotePhysical => wire::NpcExecutorMode::RemotePhysical,
            Mode::Parked => wire::NpcExecutorMode::Parked,
            _ => return None,
        },
        state: match value.state {
            State::Prepared => wire::NpcRouteState::Prepared,
            State::Running => wire::NpcRouteState::Running,
            State::Blocked => wire::NpcRouteState::Blocked,
            State::HandoffPending => wire::NpcRouteState::HandoffPending,
            State::Parked => wire::NpcRouteState::Parked,
            State::Arrived => wire::NpcRouteState::Arrived,
            State::Cancelled => wire::NpcRouteState::Cancelled,
            State::Dead => wire::NpcRouteState::Dead,
            State::Exhausted => wire::NpcRouteState::Exhausted,
            _ => return None,
        },
        lifecycle: match value.lifecycle {
            ScriptNpcLifecycle::AliveLoaded => npcs::NpcLifecycle::AliveLoaded,
            ScriptNpcLifecycle::AliveUnloaded => npcs::NpcLifecycle::AliveUnloaded,
            ScriptNpcLifecycle::Dead => npcs::NpcLifecycle::Dead,
            ScriptNpcLifecycle::Released => npcs::NpcLifecycle::Released,
            _ => return None,
        },
        pose: value.pose.as_ref().map(pose),
        segment: value.segment,
        cursor: value.cursor,
        blocked_reason: match value.blocked_reason {
            Some(reason) => Some(match reason {
                Reason::Unloaded => wire::NpcRouteBlockReason::Unloaded,
                Reason::StaleGeometry => wire::NpcRouteBlockReason::StaleGeometry,
                Reason::BlockedRoute => wire::NpcRouteBlockReason::BlockedRoute,
                Reason::ForeignProtection => wire::NpcRouteBlockReason::ForeignProtection,
                Reason::MissingActor => wire::NpcRouteBlockReason::MissingActor,
                Reason::StaleIdentity => wire::NpcRouteBlockReason::StaleIdentity,
                Reason::StaleOrder => wire::NpcRouteBlockReason::StaleOrder,
                Reason::ExecutorUnavailable => wire::NpcRouteBlockReason::ExecutorUnavailable,
                _ => return None,
            }),
            None => None,
        },
        due_tick: value.due_tick,
        native_ticks: value.native_ticks,
        geometry_cells: value.geometry_cells,
        geometry_hash: value.geometry_hash.clone(),
        max_ticks: value.max_ticks,
        handoff: value.handoff.as_ref().map(|handoff| wire::NpcRouteHandoff {
            snapshot_ref: handoff.snapshot_ref.clone(),
            source_epoch: handoff.source_epoch,
            target_epoch: handoff.target_epoch,
            simulation_tick: handoff.simulation_tick,
            dimension: handoff.dimension.clone(),
            pose: pose(&handoff.pose),
            inventory_revision: handoff.inventory_revision,
            inventory_image_hash: handoff.inventory_image_hash.clone(),
            geometry_hash: handoff.geometry_hash.clone(),
            reservation_refs: handoff.reservation_refs.clone(),
        }),
        arrival: value.arrival.as_ref().map(|arrival| wire::NpcRouteArrival {
            receipt_ref: arrival.receipt_ref.clone(),
            executor_epoch: arrival.executor_epoch,
            simulation_tick: arrival.simulation_tick,
            dimension: arrival.dimension.clone(),
            pose: pose(&arrival.pose),
            inventory_revision: arrival.inventory_revision,
            inventory_image_hash: arrival.inventory_image_hash.clone(),
        }),
        death_cargo: match &value.death_cargo {
            Some(receipt) => Some(crate::domain_inventories::encode_npc_death_cargo_receipt(
                receipt,
            )?),
            None => None,
        },
        outcome_tick: value.outcome_tick,
    })
}

fn pose(value: &ScriptNpcLoadedState) -> npcs::NpcLoadedState {
    npcs::NpcLoadedState {
        dimension: value.dimension.clone(),
        x: value.x,
        y: value.y,
        z: value.z,
        health: value.health,
        max_health: value.max_health,
        yaw: value.yaw,
        native_age_ticks: value.native_age_ticks,
        birth_origin: value
            .birth_origin
            .as_ref()
            .map(|origin| npcs::NativeBirthOrigin {
                parents: (origin.parents[0].clone(), origin.parents[1].clone()),
                started_tick: origin.started_tick,
                birth_tick: origin.birth_tick,
            }),
        carried: value
            .carried
            .iter()
            .map(|item| npcs::NpcItemSummary {
                item_id: item.item_id.clone(),
                count: item.count,
            })
            .collect(),
    }
}

pub(crate) fn max_text_bytes(value: &wire::NpcRouteOperation) -> usize {
    match value {
        wire::NpcRouteOperation::Admit(value) => [
            value.operation_id.len(),
            value.admission.handle.len(),
            value.admission.dimension.len(),
            value
                .admission
                .reservation_refs
                .iter()
                .map(String::len)
                .max()
                .unwrap_or(0),
        ]
        .into_iter()
        .max()
        .unwrap_or(0),
        wire::NpcRouteOperation::Start(value) => value
            .operation_id
            .len()
            .max(value.route_ref.len())
            .max(value.cargo_operation_id.as_ref().map_or(0, String::len)),
        wire::NpcRouteOperation::Query(value) => value.route_ref.len(),
        wire::NpcRouteOperation::Handoff(value) => {
            value.operation_id.len().max(value.route_ref.len())
        }
        wire::NpcRouteOperation::Cancel(value) => {
            value.operation_id.len().max(value.route_ref.len())
        }
    }
}
