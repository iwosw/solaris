//! Physical owned-inventory queries, atomic transfers and durable reservations.
//!
//! Each helper builds exactly the record `inventories.wit` declares and nothing
//! more: the endpoint, the revision and the items are the plugin's own, and the
//! server's own DTO validator is what refuses a value outside the contract's
//! bounds when the host converts the batch. No helper waits for an answer - a
//! query, a transfer and a reservation each answer later as
//! [`Event::OperationAnswered`](crate::events::Event::OperationAnswered) with the
//! same `request`.

use crate::{domain_operations, inventories, Command};

/// Read one owned endpoint, optionally fenced to a revision.
///
/// The answer is the endpoint's whole canonical snapshot: every occupied and
/// empty slot with the items and their components exactly as the owner holds
/// them, plus the `inventory-fence` a later transfer or reservation fences
/// against. `expected-revision` is `None` for an unfenced read; naming a revision
/// the endpoint has moved past is answered `stale-revision` instead of the
/// snapshot.
///
/// A query carries no durable operation id: it changes nothing and creates no
/// receipt, so the answer's `operation-id` is absent rather than invented and a
/// plugin must not read it as one. The endpoint may be any of the four - a
/// player's inventory, a physical container, or one durable npc's equipment
/// or carry. A missing container or handle is answered `not-found`, never an
/// invented empty inventory.
#[must_use]
pub fn query_owned_inventory(
    request: &str,
    endpoint: inventories::InventoryEndpoint,
    expected_revision: Option<u64>,
) -> Command {
    operation(
        request,
        inventories::InventoryOperation::Query(inventories::InventoryQuery {
            endpoint,
            expected_revision,
        }),
    )
}

/// Read exact per-slot native room for an actual fenced component stack.
///
/// Returns `InventoryResult::PlacementRoom` with both actual full fences and
/// every destination slot, including zero-room slots. Room is capacity, not a
/// promise to move the entire source: callers bound portions by source count.
/// Stale fences, foreign ownership and an absent/empty source refuse. This read
/// grants no reservation or route execution authority; later transfers still
/// fence every endpoint and require their current native execution permit.
#[must_use]
pub fn query_inventory_placement_room(
    request: &str,
    source: inventories::InventoryExpectedRevision,
    source_slot: u8,
    destination: inventories::InventoryExpectedRevision,
) -> Command {
    operation(
        request,
        inventories::InventoryOperation::PlacementRoom(inventories::InventoryPlacementQuery {
            source,
            source_slot,
            destination,
        }),
    )
}

/// Move items between owned endpoints under one durable operation id.
///
/// A player endpoint requires the exact participating runtime player id:
/// another player's inventory is `forbidden`, and a disconnected session is
/// `not-found`. `actor_id` is zero only for a native container and this
/// plugin's npc equipment/carry: stock may be issued or returned without
/// a player participant. `expected_revisions` is one fence per distinct
/// endpoint the transfers name - the fences the plugin read with
/// [`query_owned_inventory`] - and a fence that has moved is refused
/// `stale-revision`, never applied against the image the plugin did not see.
///
/// `operation_id` is the durable name the server records the commit under: a
/// transfer the plugin repeats with byte-identical content replays the recorded
/// outcome and applies nothing, which is what makes retrying after a lost answer
/// safe, while reusing the id for different content is refused
/// `operation-conflict`. The answer reports the committed server-owned endpoint
/// fences; a participating player's fence is read again with a query after its
/// world-journal decision id has been assigned.
///
/// The items move as the stacks they are, components included: a transfer never
/// substitutes a resource-only count for an item the owner holds.
#[must_use]
pub fn transfer_owned_items(
    request: &str,
    operation_id: &str,
    actor_id: u64,
    execution_permit: Option<crate::types::NpcExecutionPermit>,
    transfers: Vec<inventories::OwnedItemTransfer>,
    expected_revisions: Vec<inventories::InventoryExpectedRevision>,
) -> Command {
    operation(
        request,
        inventories::InventoryOperation::Transfer(inventories::InventoryTransfer {
            operation_id: operation_id.to_owned(),
            actor_id,
            execution_permit,
            transfers,
            expected_revisions,
        }),
    )
}

/// Spend exactly the next reserved plan portion as a physical transfer.
///
/// The original plan must match the admitted hash, `portion` must name its next
/// material-bearing portion (one-based), and every transfer must leave the
/// reservation's source. Resource-free stages are skipped, not executed.
/// The result is `InventoryResult::ReservedTransfer`: full native after-images
/// and consumed/remaining accounting from one durable decision. Failure consumes
/// nothing; replaying identical `operation_id` content never transfers twice.
/// A container reservation belongs to the admitted native principal (`actor_id`
/// zero); a player reservation requires that actual player's principal.
/// Route-owned NPC endpoints require their current execution permit.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn consume_inventory_reservation(
    request: &str,
    operation_id: &str,
    actor_id: u64,
    execution_permit: Option<crate::types::NpcExecutionPermit>,
    reservation_ref: &str,
    expected_reservation_revision: u64,
    portion: u64,
    resource_plan: inventories::InventoryResourcePlan,
    transfers: Vec<inventories::OwnedItemTransfer>,
    expected_revisions: Vec<inventories::InventoryExpectedRevision>,
) -> Command {
    operation(
        request,
        inventories::InventoryOperation::ConsumeReservation(
            inventories::InventoryConsumeReservation {
                operation_id: operation_id.to_owned(),
                actor_id,
                execution_permit,
                reservation_ref: reservation_ref.to_owned(),
                expected_reservation_revision,
                portion,
                resource_plan,
                transfers,
                expected_revisions,
            },
        ),
    )
}

/// Reserve the materials of one resource plan against one owned endpoint.
///
/// The reservation is durable: the server records it under `operation_id`, holds
/// the plan's quantities against the endpoint so a later transfer cannot draw
/// below them, and answers the whole `inventory-reservation-snapshot` - the
/// opaque `reservation-ref`, the plan hash and the reserved, consumed, returned
/// and remaining units of every resource. A reservation repeats under the same
/// operation id with byte-identical content as a replay, exactly as a transfer
/// does.
///
/// `expected_revision` is the endpoint's fence, read with
/// [`query_owned_inventory`]: a reservation against a fence that has moved is
/// refused `stale-revision` rather than held against the wrong image, and a plan
/// the endpoint cannot cover is refused `insufficient-items`. Native container
/// access is checked freshly and core fences the real container image while
/// installing the durable reservation and physical floor.
#[must_use]
pub fn reserve_inventory_items(
    request: &str,
    operation_id: &str,
    endpoint: inventories::InventoryEndpoint,
    resource_plan: inventories::InventoryResourcePlan,
    expected_revision: inventories::InventoryFence,
) -> Command {
    operation(
        request,
        inventories::InventoryOperation::Reserve(inventories::InventoryReserve {
            operation_id: operation_id.to_owned(),
            endpoint,
            resource_plan,
            expected_revision,
        }),
    )
}

/// Read the complete durable reservation state for this plugin.
#[must_use]
pub fn query_inventory_reservation(request: &str, reservation_ref: &str) -> Command {
    operation(
        request,
        inventories::InventoryOperation::ReservationStatus(
            inventories::InventoryReservationStatus {
                reservation_ref: reservation_ref.to_owned(),
            },
        ),
    )
}

/// Release a reservation at its observed revision under one durable operation.
/// Only the unused native hold is removed; items that were already consumed are
/// never returned or issued, and unused stock has stayed at its physical source.
#[must_use]
pub fn release_inventory_reservation(
    request: &str,
    operation_id: &str,
    reservation_ref: &str,
    expected_revision: u64,
) -> Command {
    operation(
        request,
        inventories::InventoryOperation::Release(inventories::InventoryRelease {
            operation_id: operation_id.to_owned(),
            reservation_ref: reservation_ref.to_owned(),
            expected_revision,
        }),
    )
}

/// One owned-inventory record wrapped in the single operation envelope every
/// family shares.
fn operation(request: &str, record: inventories::InventoryOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest { expected_plugin_storage_versions: Vec::new(), request: request.to_owned(),
    operation: domain_operations::DomainOperation::Inventory(record), })
}
