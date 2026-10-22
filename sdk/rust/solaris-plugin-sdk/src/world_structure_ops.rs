//! Generic physical world operations. Answers arrive asynchronously through operation events.
use crate::{Command, domain_operations, world_structures as wire};

/// Read bounded exact physical facts without registering a worksite.
#[must_use]
pub fn query_blocks(
    request: &str,
    dimension: &str,
    positions: Vec<wire::BlockPosition>,
) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::QueryBlocks(wire::QueryBlocks {
            dimension: dimension.to_owned(),
            positions,
        }),
    )
}

#[must_use]
pub fn list_features(request: &str, cursor: Option<&str>, limit: u8) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::ListFeatures(wire::ListFeatures {
            cursor: cursor.map(str::to_owned),
            limit,
        }),
    )
}
#[must_use]
pub fn query_feature(request: &str, feature_id: &str) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::QueryFeature(wire::QueryFeature {
            feature_id: feature_id.to_owned(),
        }),
    )
}
#[must_use]
pub fn survey(request: &str, dimension: &str, bounds: wire::WorldBounds) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::Survey(wire::Survey {
            dimension: dimension.to_owned(),
            bounds,
        }),
    )
}
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn prepare_structure(
    request: &str,
    operation_id: &str,
    dimension: &str,
    blueprint_id: &str,
    anchor: wire::BlockPosition,
    rotation: u16,
    survey_token: &str,
    expected_world_revision: u64,
    expected_blueprint_hash: Option<&str>,
) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::PrepareStructure(wire::PrepareStructure {
            operation_id: operation_id.to_owned(),
            dimension: dimension.to_owned(),
            blueprint_id: blueprint_id.to_owned(),
            anchor,
            rotation,
            survey_token: survey_token.to_owned(),
            expected_world_revision,
            expected_blueprint_hash: expected_blueprint_hash.map(str::to_owned),
        }),
    )
}
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn advance_structure(
    request: &str,
    operation_id: &str,
    structure_id: &str,
    stage: &str,
    reservation_ref: &str,
    expected_revision: u64,
    work_units: u64,
) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::AdvanceStructure(wire::AdvanceStructure {
            operation_id: operation_id.to_owned(),
            structure_id: structure_id.to_owned(),
            stage: stage.to_owned(),
            reservation_ref: reservation_ref.to_owned(),
            expected_revision,
            work_units,
        }),
    )
}
#[must_use]
pub fn pause_structure(
    request: &str,
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::PauseStructure(state_change(
            operation_id,
            structure_id,
            expected_revision,
        )),
    )
}
#[must_use]
pub fn resume_structure(
    request: &str,
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::ResumeStructure(state_change(
            operation_id,
            structure_id,
            expected_revision,
        )),
    )
}
#[must_use]
pub fn cancel_structure(
    request: &str,
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::CancelStructure(state_change(
            operation_id,
            structure_id,
            expected_revision,
        )),
    )
}
#[must_use]
pub fn structure_status(request: &str, structure_id: &str) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::Status(wire::StructureStatus {
            structure_id: structure_id.to_owned(),
        }),
    )
}
/// Check authored points, preserved entrances and live routes; not product acceptance.
#[must_use]
pub fn check_structure(request: &str, structure_id: &str) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::CheckStructure(wire::StructureStatus {
            structure_id: structure_id.to_owned(),
        }),
    )
}
#[must_use]
pub fn register_worksite(
    request: &str,
    operation_id: &str,
    worksite: wire::WorksiteRequest,
) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::RegisterWorksite(wire::RegisterWorksite {
            operation_id: operation_id.to_owned(),
            request: worksite,
        }),
    )
}
#[must_use]
pub fn worksite_status(request: &str, worksite_id: &str) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::WorksiteStatus(wire::WorksiteStatus {
            worksite_id: worksite_id.to_owned(),
        }),
    )
}
#[must_use]
pub fn check_worksite(request: &str, worksite_id: &str) -> Command {
    operation(
        request,
        wire::WorldStructureOperation::CheckWorksite(wire::WorksiteStatus {
            worksite_id: worksite_id.to_owned(),
        }),
    )
}
fn state_change(
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> wire::ChangeStructureState {
    wire::ChangeStructureState {
        operation_id: operation_id.to_owned(),
        structure_id: structure_id.to_owned(),
        expected_revision,
    }
}
fn operation(request: &str, value: wire::WorldStructureOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest {
        expected_plugin_storage_versions: Vec::new(),
        request: request.to_owned(),
        operation: domain_operations::DomainOperation::WorldStructure(value),
    })
}
