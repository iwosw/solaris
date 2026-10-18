//! Generic durable NPC identity, physical work and group-control commands.
//!
//! Repeating a durable operation ID with identical content replays its receipt.
//! Plugins own social membership, population, housing and occupations.

use crate::{domain_operations, inventories, npcs, types, Command};

/// Adopt a loaded living entity near the authenticated player.
/// `expected_entity_revision` is zero for a first adoption.
#[must_use]
pub fn adopt_npc(
    request: &str,
    operation_id: &str,
    actor_id: u64,
    entity_uuid: &str,
    expected_entity_revision: u64,
) -> Command {
    operation(
        request,
        npcs::NpcOperation::Adopt(npcs::NpcAdopt {
            operation_id: operation_id.to_owned(),
            actor_id,
            entity_uuid: entity_uuid.to_owned(),
            expected_entity_revision,
        }),
    )
}

/// Adopt the original loaded/alive native child through ownership of both
/// actual native parents. Zero is the existing unbound-ledger revision fence;
/// native UUID/provenance/lifecycle are independently checked by the owner.
#[must_use]
pub fn adopt_native_child(
    request: &str,
    operation_id: &str,
    entity_uuid: &str,
    parent_handles: [String; 2],
    expected_entity_revision: u64,
) -> Command {
    let [first, second] = parent_handles;
    operation(
        request,
        npcs::NpcOperation::AdoptNativeChild(npcs::NpcAdoptNativeChild {
            operation_id: operation_id.to_owned(),
            entity_uuid: entity_uuid.to_owned(),
            parent_handles: (first, second),
            expected_entity_revision,
        }),
    )
}

/// Materialize one registered living entity at a loaded, standable coordinate.
/// The stable plugin generation key resolves to the same UUID on retries.
#[must_use]
pub fn spawn_npc(
    request: &str,
    operation_id: &str,
    generation_id: &str,
    position: types::Position,
    entity_type: &str,
) -> Command {
    operation(
        request,
        npcs::NpcOperation::Spawn(npcs::NpcSpawn {
            operation_id: operation_id.to_owned(),
            generation_id: generation_id.to_owned(),
            position,
            entity_type: entity_type.to_owned(),
        }),
    )
}

/// Release this plugin's identity binding without deleting the physical entity.
#[must_use]
pub fn release_npc(
    request: &str,
    operation_id: &str,
    handle: &str,
    expected_revision: u64,
) -> Command {
    operation(
        request,
        npcs::NpcOperation::Release(npcs::NpcRelease {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            expected_revision,
        }),
    )
}

/// Read the server's current owner-scoped npc snapshots. An explicit
/// handle that is absent or foreign is refused; this read observes death
/// before answering, so role policy cannot equip a tombstoned npc.
/// An empty list requests one bounded page instead.
#[must_use]
pub fn query_npcs(request: &str, handles: &[String], cursor: Option<&str>) -> Command {
    operation(
        request,
        npcs::NpcOperation::Query(npcs::NpcQuery {
            handles: handles.to_vec(),
            cursor: cursor.map(str::to_owned),
        }),
    )
}

/// Discover actual native children of owned parent handles; observation grants
/// no child ownership. Limit and parent list are each bounded to 64.
#[must_use]
pub fn query_native_births(
    request: &str,
    parent_handles: &[String],
    cursor: Option<u64>,
    limit: u16,
) -> Command {
    operation(
        request,
        npcs::NpcOperation::QueryBirths(npcs::NativeBirthQuery {
            parent_handles: parent_handles.to_vec(),
            cursor,
            limit,
        }),
    )
}

/// Give one npc a bounded work order.
///
/// `work` names a concrete physical target and `work_units` bounds the work
/// this call may commit, at least one and at
/// most `mc_script::MAX_WORK_UNITS` (4096). The engine reads the live world and
/// decides what the worker can actually do, so the answer is not a promise: a
/// worker without its tool pauses `missing_tool`, one whose container is empty
/// pauses `missing_input`, one whose chunk is not loaded pauses `unloaded`, and
/// the committed item changes the engine reports are the real ones it made.
///
/// `expected_revision` is the revision of the npc's own work record, which
/// is `0` for a npc this plugin has never given work. A stale one is refused
/// `stale_revision` with nothing applied, which is what makes an assignment safe
/// to retry against the revision the last answer named.
#[must_use]
pub fn assign_npc_work(
    request: &str,
    operation_id: &str,
    handle: &str,
    work: npcs::WorkOrder,
    work_units: u64,
    expected_revision: u64,
) -> Command {
    order(
        request,
        npcs::NpcOrderOperation::AssignWork(npcs::AssignWork {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            work,
            work_units,
            expected_revision,
        }),
    )
}

/// Explicitly stop unfinished work without discarding its original lineage.
///
/// `reason` is an opaque owner label using the native script-id grammar (1..64
/// lowercase ASCII letters, digits, `_` or `-`), not a native product category.
/// Success returns `NpcOrderResult::Work` with `WorkState::Suspended`, the same
/// admitted operation id, progress and exact retained receipts, and no deadline.
/// Resume it only with [`resume_npc_work`] and its fresh native record fence.
#[must_use]
pub fn suspend_npc_work(
    request: &str,
    operation_id: &str,
    handle: &str,
    admitted_operation_id: &str,
    reason: &str,
    expected_revision: u64,
) -> Command {
    order(
        request,
        npcs::NpcOrderOperation::SuspendWork(npcs::SuspendWork {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            admitted_operation_id: admitted_operation_id.to_owned(),
            reason: reason.to_owned(),
            expected_revision,
        }),
    )
}

/// Resume only an explicitly suspended original assignment, never a replacement
/// job or a terminal cancellation. Native physical pauses continue through the
/// authorized native producer; paused elapsed time never grants production.
#[must_use]
pub fn resume_npc_work(
    request: &str,
    operation_id: &str,
    handle: &str,
    admitted_operation_id: &str,
    expected_revision: u64,
) -> Command {
    order(
        request,
        npcs::NpcOrderOperation::ResumeWork(npcs::ResumeWork {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            admitted_operation_id: admitted_operation_id.to_owned(),
            expected_revision,
        }),
    )
}

/// Acknowledge only portions already committed to the consumer's durable state.
/// Native execution backpressures rather than evicting an unacknowledged portion.
#[must_use]
pub fn ack_npc_work_receipts(
    request: &str,
    operation_id: &str,
    handle: &str,
    admitted_operation_id: &str,
    through_sequence: u64,
    expected_revision: u64,
) -> Command {
    order(
        request,
        npcs::NpcOrderOperation::AckWorkReceipts(npcs::AckWorkReceipts {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            admitted_operation_id: admitted_operation_id.to_owned(),
            through_sequence,
            expected_revision,
        }),
    )
}

/// Stop what one npc was told to do.
///
/// The answer is the cancelled assignment's durable revision, which is the
/// revision this npc's work record now holds: a plugin sends that back for
/// the next call about the same npc. A stale `expected_revision` is refused
/// rather than cancelling work another decision already moved.
#[must_use]
pub fn cancel_npc_work(
    request: &str,
    operation_id: &str,
    handle: &str,
    expected_revision: u64,
) -> Command {
    order(
        request,
        npcs::NpcOrderOperation::CancelWork(npcs::CancelWork {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            expected_revision,
        }),
    )
}

/// Start or stop actual held-item use without replacing an NPC's movement,
/// work or attack order. Active routes require their current executor permit.
#[must_use]
pub fn use_npc_item(request: &str, input: npcs::UseItem) -> Command {
    order(request, npcs::NpcOrderOperation::UseItem(input))
}

/// Move the exact equipment instance into native world entity custody.
#[must_use]
pub fn deploy_npc_item(request: &str, input: npcs::DeployItem) -> Command {
    order(request, npcs::NpcOrderOperation::DeployItem(input))
}

/// Return one actual world-custodied item to the specified native equipment slot.
#[must_use]
pub fn retract_npc_item(request: &str, input: npcs::RetractItem) -> Command {
    order(request, npcs::NpcOrderOperation::RetractItem(input))
}

/// Claim native world-item control under the caller's actual executor authority.
#[must_use]
pub fn capture_npc_item(request: &str, input: npcs::CaptureItem) -> Command {
    order(request, npcs::NpcOrderOperation::CaptureItem(input))
}

/// Give every named member the same squad order.
///
/// `handles` and `expected_order_revisions` are two parallel lists: the durable
/// handles this plugin holds and the order revision each member is fenced on,
/// one entry per handle in the same order. The server pairs, sorts and dedups
/// them, so one operation id always fingerprints identically. A member whose
/// revision has moved, that is not this plugin's, or whose engine admission does
/// not commit refuses the batch with `blocked` and answers one outcome per
/// member - every one of them `stale_revision` - which is why the answer carries
/// member detail even when nothing was applied.
///
/// `order` names follow, move, hold, patrol, attack or retreat controls.
/// Per-member outcomes and combat events report native accepted effects.
#[must_use]
pub fn issue_npc_order(
    request: &str,
    operation_id: &str,
    handles: Vec<String>,
    expected_order_revisions: Vec<u64>,
    order: npcs::Order,
) -> Command {
    self::order(
        request,
        npcs::NpcOrderOperation::IssueOrder(npcs::IssueOrder {
            operation_id: operation_id.to_owned(),
            handles,
            expected_order_revisions,
            order,
        }),
    )
}

/// Cancel the squad order every named member holds.
///
/// The member list is the same two parallel lists [`issue_npc_order`] takes,
/// with each member's current order revision. An accepted batch answers one
/// `applied` outcome per member - a cancellation carries the member's batch
/// ordinal as its slot, never a formation slot - and clears the order; a batch
/// the server refuses answers `blocked` with the same per-member detail an
/// issuance does.
#[must_use]
pub fn cancel_npc_order(
    request: &str,
    operation_id: &str,
    handles: Vec<String>,
    expected_order_revisions: Vec<u64>,
) -> Command {
    order(
        request,
        npcs::NpcOrderOperation::CancelOrder(npcs::CancelOrder {
            operation_id: operation_id.to_owned(),
            handles,
            expected_order_revisions,
        }),
    )
}

/// Read current native order/work fences and canonical remaining work.
#[must_use]
pub fn query_npc_controls(request: &str, handles: &[String]) -> Command {
    order(
        request,
        npcs::NpcOrderOperation::Query(npcs::NpcControlQuery {
            handles: handles.to_vec(),
        }),
    )
}

/// Commit an exact item cost and nonlethal signed native health change.
/// Zero/no-op changes and lethal damage refuse without consuming items.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn apply_npc_health_delta(
    request: &str,
    operation_id: &str,
    handle: &str,
    expected_revision: u64,
    source: inventories::InventoryEndpoint,
    material: inventories::InventoryMaterial,
    health_delta_milli: i32,
) -> Command {
    operation(
        request,
        npcs::NpcOperation::ApplyHealthDelta(npcs::ApplyHealthDelta {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            expected_revision,
            source,
            material,
            health_delta_milli,
        }),
    )
}

/// One npc record wrapped in the single operation envelope every family
/// shares.
fn operation(request: &str, record: npcs::NpcOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest {
        expected_plugin_storage_versions: Vec::new(),
        request: request.to_owned(),
        operation: domain_operations::DomainOperation::Npc(record),
    })
}

/// One npc work or squad record wrapped in the same envelope, under its own
/// family: the two are separate names because the server's own unions differ, and
/// a work order sent as a npc call would be a different operation.
fn order(request: &str, record: npcs::NpcOrderOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest {
        expected_plugin_storage_versions: Vec::new(),
        request: request.to_owned(),
        operation: domain_operations::DomainOperation::NpcOrder(record),
    })
}
