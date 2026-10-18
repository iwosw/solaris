//! Finite routes run by the native physical actor owner. Admission and command
//! acceptance are not arrival; query the actual native status and receipt.

use crate::{domain_operations, npc_routes, Command};

#[must_use]
pub fn admit_npc_route(
    request: &str,
    operation_id: &str,
    admission: npc_routes::NpcRouteAdmission,
) -> Command {
    operation(
        request,
        npc_routes::NpcRouteOperation::Admit(npc_routes::NpcRouteAdmit {
            operation_id: operation_id.to_owned(),
            admission,
        }),
    )
}

#[must_use]
pub fn start_npc_route(
    request: &str,
    operation_id: &str,
    route_ref: &str,
    expected_epoch: u64,
    cargo_operation_id: Option<&str>,
) -> Command {
    operation(
        request,
        npc_routes::NpcRouteOperation::Start(npc_routes::NpcRouteStart {
            operation_id: operation_id.to_owned(),
            route_ref: route_ref.to_owned(),
            expected_epoch,
            cargo_operation_id: cargo_operation_id.map(str::to_owned),
        }),
    )
}

#[must_use]
pub fn query_npc_route(request: &str, route_ref: &str) -> Command {
    operation(
        request,
        npc_routes::NpcRouteOperation::Query(npc_routes::NpcRouteQuery {
            route_ref: route_ref.to_owned(),
        }),
    )
}

#[must_use]
pub fn handoff_npc_route(
    request: &str,
    operation_id: &str,
    route_ref: &str,
    expected_epoch: u64,
    mode: npc_routes::NpcExecutorMode,
) -> Command {
    operation(
        request,
        npc_routes::NpcRouteOperation::Handoff(npc_routes::NpcRouteHandoffRequest {
            operation_id: operation_id.to_owned(),
            route_ref: route_ref.to_owned(),
            expected_epoch,
            mode,
        }),
    )
}

#[must_use]
pub fn cancel_npc_route(
    request: &str,
    operation_id: &str,
    route_ref: &str,
    expected_epoch: u64,
) -> Command {
    operation(
        request,
        npc_routes::NpcRouteOperation::Cancel(npc_routes::NpcRouteCancel {
            operation_id: operation_id.to_owned(),
            route_ref: route_ref.to_owned(),
            expected_epoch,
        }),
    )
}

fn operation(request: &str, operation: npc_routes::NpcRouteOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest { expected_plugin_storage_versions: Vec::new(), request: request.to_owned(),
    operation: domain_operations::DomainOperation::NpcRoute(operation), })
}
