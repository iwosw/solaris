//! Native settlement query/action commands (D006 S1/S2/S3/S4).
//!
//! Thin guest helpers over the `settlements` WIT interface: a query reads
//! owned-or-visible records, an action names one closed mutation fenced by the
//! monotonic record id and the revision the caller read. Like the npc helpers,
//! these only build the operation envelope; the server's own DTOs validate and
//! the native settlement owner decides.

use crate::{domain_operations, settlements, types, Command};

/// Read the server's current settlement snapshots. An empty list requests one
/// bounded page; explicit names must each be valid registry names. This read
/// carries no durable operation id: the answer observes the server's current
/// revisions, and every snapshot carries the stable id a later action fences
/// on (names are reusable, ids never are).
#[must_use]
pub fn query_settlements(request: &str, names: &[String], cursor: Option<&str>) -> Command {
    operation_query(
        request,
        settlements::QuerySettlement {
            names: names.to_vec(),
            cursor: cursor.map(str::to_owned),
        },
    )
}

/// Bind one resident villager (by native entity UUID) to the settlement id the
/// query observed, fenced by that record's revision.
#[must_use]
pub fn bind_settlement(
    request: &str,
    operation_id: &str,
    settlement: u64,
    entity_uuid: &str,
    expected_revision: u64,
) -> Command {
    operation_action(
        request,
        settlements::SettlementOperation::Bind(settlements::BindSettlement {
            operation_id: operation_id.to_owned(),
            settlement,
            entity_uuid: entity_uuid.to_owned(),
            expected_revision,
        }),
    )
}

/// Release one resident villager (by native entity UUID) from the settlement
/// id the query observed. A recreated same-name record never matches.
#[must_use]
pub fn unbind_settlement(
    request: &str,
    operation_id: &str,
    settlement: u64,
    entity_uuid: &str,
    expected_revision: u64,
) -> Command {
    operation_action(
        request,
        settlements::SettlementOperation::Unbind(settlements::UnbindSettlement {
            operation_id: operation_id.to_owned(),
            settlement,
            entity_uuid: entity_uuid.to_owned(),
            expected_revision,
        }),
    )
}

/// Rename the settlement id the query observed, fenced by its revision. The
/// id survives the rename; the answer carries both.
#[must_use]
pub fn rename_settlement(
    request: &str,
    operation_id: &str,
    settlement: u64,
    new_name: &str,
    expected_revision: u64,
) -> Command {
    operation_action(
        request,
        settlements::SettlementOperation::Rename(settlements::RenameSettlement {
            operation_id: operation_id.to_owned(),
            settlement,
            new_name: new_name.to_owned(),
            expected_revision,
        }),
    )
}

/// Form a household from the already-bound native people the query observed.
#[must_use]
pub fn form_settlement_household(
    request: &str,
    operation_id: &str,
    settlement: u64,
    entity_uuids: &[String],
    expected_revision: u64,
) -> Command {
    operation_action(
        request,
        settlements::SettlementOperation::FormHousehold(settlements::FormHousehold {
            operation_id: operation_id.to_owned(),
            settlement,
            entity_uuids: entity_uuids.to_vec(),
            expected_revision,
        }),
    )
}

/// Relocate the same household, fenced by both settlement observations.
#[must_use]
pub fn relocate_settlement_household(
    request: &str,
    operation_id: &str,
    settlement: u64,
    household: &str,
    destination: u64,
    expected_revision: u64,
    expected_destination_revision: u64,
) -> Command {
    operation_action(
        request,
        settlements::SettlementOperation::RelocateHousehold(settlements::RelocateHousehold {
            operation_id: operation_id.to_owned(),
            settlement,
            household: household.to_owned(),
            destination,
            expected_revision,
            expected_destination_revision,
        }),
    )
}

/// Plan one native district on the settlement id the query observed, fenced by
/// that record's revision. The plan binds requested branch/level and certified
/// building refs; the query later derives the live observation.
#[must_use]
pub fn plan_settlement_district(
    request: &str,
    operation_id: &str,
    settlement: u64,
    expected_revision: u64,
    branch: &str,
    level: u8,
    buildings: &[settlements::DistrictBuildingRef],
) -> Command {
    operation_action(
        request,
        settlements::SettlementOperation::PlanDistrict(settlements::PlanDistrict {
            operation_id: operation_id.to_owned(),
            settlement,
            expected_revision,
            branch: branch.to_owned(),
            level,
            buildings: buildings.to_vec(),
        }),
    )
}

/// Run one typed economic mutation on the settlement id the query observed,
/// fenced by that record's revision. This is the only economic customization
/// surface: production, employment, quotes, escrow and caravan arms arrive as
/// the contract's closed `economy-action`, never as command strings or JSON
/// blobs. Like every sibling helper this only builds the operation envelope;
/// the server's own DTOs validate and the native settlement owner decides.
#[must_use]
pub fn request_settlement_economy(
    request: &str,
    operation_id: &str,
    settlement: u64,
    expected_revision: u64,
    action: settlements::EconomyAction,
) -> Command {
    operation_action(
        request,
        settlements::SettlementOperation::Economy(settlements::EconomySettlement {
            operation_id: operation_id.to_owned(),
            settlement,
            expected_revision,
            action,
        }),
    )
}

/// One settlement query wrapped in the single operation envelope every family
/// shares.
fn operation_query(request: &str, query: settlements::QuerySettlement) -> Command {
    Command::Operation(domain_operations::OperationRequest {
        expected_plugin_storage_versions: Vec::new(),
        request: request.to_owned(),
        operation: domain_operations::DomainOperation::QuerySettlement(query),
    })
}

/// One closed settlement mutation wrapped in the same envelope, under its own
/// family: the two are separate names because the server's own unions differ,
/// and a settlement action sent as a query would be a different operation.
fn operation_action(request: &str, action: settlements::SettlementOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest {
        expected_plugin_storage_versions: Vec::new(),
        request: request.to_owned(),
        operation: domain_operations::DomainOperation::SettlementAction(action),
    })
}

/// The posing anchor the native owner reports for one settlement record.
#[must_use]
pub fn settlement_anchor(snapshot: &settlements::SettlementSnapshot) -> types::Position {
    types::Position {
        x: f64::from(snapshot.anchor_x),
        y: f64::from(snapshot.anchor_y),
        z: f64::from(snapshot.anchor_z),
    }
}
