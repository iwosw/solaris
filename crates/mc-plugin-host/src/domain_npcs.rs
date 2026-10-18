//! The npc and npc-order families of the contract, converted to and
//! from the server's own DTOs.
//!
//! The generated `npcs` interface is a rename of the server's own
//! vocabulary and nothing more: `decode`/`decode_order` build exactly the
//! `mc_script::ScriptNpcOperation`/`ScriptNpcOrderOperation` the guest
//! described, `encode_result`/`encode_order_result` rename the server's typed
//! results back, and none of them invents a value, clamps one to a bound, or
//! substitutes a different npc, endpoint or target for the one a plugin
//! named. Bounds stay the DTO's own: `ScriptOperationRequest::try_new` is what
//! validates and canonicalizes a decoded operation before any owner sees it -
//! including the parallel handle/revision lists, which it sorts and dedups so one
//! operation id always fingerprints identically - so this conversion only calls
//! the DTO's own constructors.
//!
//! A native variant this contract cannot express - the server's enums are
//! `non_exhaustive` on purpose - is never mapped onto a member that means
//! something else: the `encode_*` functions answer `None` and the answer is
//! dropped rather than delivered with an invented shape. A refusal is not such a
//! variant: `npc_order_batch_refusal` answers a `blocked` refusal whose
//! payload carries one per-member state per handle, and that payload is encoded
//! here like any other, so a plugin reads why every member was refused instead of
//! losing the batch detail behind a bare failure.

use crate::bindings::solaris::plugin::inventories as wire_inventories;
use crate::bindings::solaris::plugin::npcs as wire;
use mc_script::{
    ScriptBlockPosition, ScriptCombatEvent, ScriptDtoError, ScriptEngagementPolicy,
    ScriptFormation, ScriptFormationKind, ScriptHostileCategory, ScriptInventoryEndpoint,
    ScriptInventoryMaterial, ScriptItemChange, ScriptNpcControlSnapshot, ScriptNpcItemSummary,
    ScriptNpcLifecycle, ScriptNpcLoadedState, ScriptNpcOperation, ScriptNpcOrder,
    ScriptNpcOrderOperation, ScriptNpcOrderResult, ScriptNpcResult, ScriptNpcSnapshot,
    ScriptNpcWorkOrder, ScriptOrderMemberOutcome, ScriptOrderMemberState, ScriptOrderTarget,
    ScriptOrderTargetRef, ScriptPosition, ScriptWorkArea, ScriptWorkAssignment,
    ScriptWorkPauseReason, ScriptWorkState,
};

/// One npc call, as the server's own DTO.
///
/// The fields are carried unchanged; DTO validation owns admission bounds.
pub(crate) fn decode(value: wire::NpcOperation) -> Result<ScriptNpcOperation, ScriptDtoError> {
    Ok(match value {
        wire::NpcOperation::Adopt(claim) => ScriptNpcOperation::Adopt {
            operation_id: claim.operation_id,
            actor_id: claim.actor_id,
            entity_uuid: claim.entity_uuid,
            expected_entity_revision: claim.expected_entity_revision,
        },
        wire::NpcOperation::AdoptNativeChild(claim) => ScriptNpcOperation::AdoptNativeChild {
            operation_id: claim.operation_id,
            entity_uuid: claim.entity_uuid,
            parent_handles: [claim.parent_handles.0, claim.parent_handles.1],
            expected_entity_revision: claim.expected_entity_revision,
        },
        wire::NpcOperation::Spawn(spawn) => ScriptNpcOperation::Spawn {
            operation_id: spawn.operation_id,
            generation_id: spawn.generation_id,
            position: ScriptPosition::try_new(spawn.position.x, spawn.position.y, spawn.position.z)
                .ok_or(ScriptDtoError::InvalidBounds)?,
            entity_type: spawn.entity_type,
        },
        wire::NpcOperation::Query(query) => ScriptNpcOperation::Query {
            handles: query.handles,
            cursor: query.cursor,
        },
        wire::NpcOperation::QueryBirths(query) => ScriptNpcOperation::QueryBirths {
            parent_handles: query.parent_handles,
            cursor: query.cursor,
            limit: query.limit,
        },
        wire::NpcOperation::Release(release) => ScriptNpcOperation::Release {
            operation_id: release.operation_id,
            handle: release.handle,
            expected_revision: release.expected_revision,
        },
        wire::NpcOperation::ApplyHealthDelta(effect) => ScriptNpcOperation::ApplyHealthDelta {
            operation_id: effect.operation_id,
            handle: effect.handle,
            expected_revision: effect.expected_revision,
            source: endpoint(effect.source),
            material: ScriptInventoryMaterial::new(
                effect.material.resource_id,
                effect.material.quantity,
            ),
            health_delta_milli: effect.health_delta_milli,
        },
    })
}

/// One npc work or squad call, as the server's own DTO.
///
/// The two parallel lists an order carries - the durable handles and the order
/// revision each member is fenced on - are handed over as they arrived, in the
/// same order and the same length: the DTO's own canonicalization is what pairs
/// them, and a conversion that paired them itself would be a second, silent
/// interpretation of the guest's answer.
pub(crate) fn decode_order(
    value: wire::NpcOrderOperation,
) -> Result<ScriptNpcOrderOperation, ScriptDtoError> {
    Ok(match value {
        wire::NpcOrderOperation::Query(query) => ScriptNpcOrderOperation::Query {
            handles: query.handles,
        },
        wire::NpcOrderOperation::UseItem(input) => ScriptNpcOrderOperation::UseItem {
            operation_id: input.operation_id,
            handle: input.handle,
            equipment_slot: input.equipment_slot,
            active: input.active,
            expected_revision: input.expected_revision,
            execution_permit: input.execution_permit.map(|permit| {
                mc_script::ScriptNpcExecutionPermit::new(permit.route_ref, permit.executor_epoch)
            }),
        },
        wire::NpcOrderOperation::DeployItem(input) => ScriptNpcOrderOperation::DeployItem {
            operation_id: input.operation_id,
            handle: input.handle,
            equipment_slot: input.equipment_slot,
            expected_revision: input.expected_revision,
            execution_permit: input.execution_permit.map(|permit| {
                mc_script::ScriptNpcExecutionPermit::new(permit.route_ref, permit.executor_epoch)
            }),
            entity_uuid: input.entity_uuid,
            support: position(input.support),
            position: ScriptPosition::try_new(input.position.x, input.position.y, input.position.z)
                .ok_or(ScriptDtoError::InvalidBounds)?,
            heading_degrees: input.heading_degrees,
        },
        wire::NpcOrderOperation::RetractItem(input) => ScriptNpcOrderOperation::RetractItem {
            operation_id: input.operation_id,
            handle: input.handle,
            expected_revision: input.expected_revision,
            entity_uuid: input.entity_uuid,
            equipment_slot: input.equipment_slot,
            execution_permit: input.execution_permit.map(|permit| {
                mc_script::ScriptNpcExecutionPermit::new(permit.route_ref, permit.executor_epoch)
            }),
        },
        wire::NpcOrderOperation::CaptureItem(input) => ScriptNpcOrderOperation::CaptureItem {
            operation_id: input.operation_id,
            handle: input.handle,
            expected_revision: input.expected_revision,
            entity_uuid: input.entity_uuid,
            execution_permit: input.execution_permit.map(|permit| {
                mc_script::ScriptNpcExecutionPermit::new(permit.route_ref, permit.executor_epoch)
            }),
        },
        wire::NpcOrderOperation::AssignWork(assign) => ScriptNpcOrderOperation::AssignWork {
            operation_id: assign.operation_id,
            handle: assign.handle,
            work: work_order(assign.work),
            work_units: assign.work_units,
            expected_revision: assign.expected_revision,
        },
        wire::NpcOrderOperation::SuspendWork(suspend) => ScriptNpcOrderOperation::SuspendWork {
            operation_id: suspend.operation_id,
            handle: suspend.handle,
            admitted_operation_id: suspend.admitted_operation_id,
            reason: suspend.reason,
            expected_revision: suspend.expected_revision,
        },
        wire::NpcOrderOperation::ResumeWork(resume) => ScriptNpcOrderOperation::ResumeWork {
            operation_id: resume.operation_id,
            handle: resume.handle,
            admitted_operation_id: resume.admitted_operation_id,
            expected_revision: resume.expected_revision,
        },
        wire::NpcOrderOperation::AckWorkReceipts(ack) => ScriptNpcOrderOperation::AckWorkReceipts {
            operation_id: ack.operation_id,
            handle: ack.handle,
            admitted_operation_id: ack.admitted_operation_id,
            through_sequence: ack.through_sequence,
            expected_revision: ack.expected_revision,
        },
        wire::NpcOrderOperation::CancelWork(cancel) => ScriptNpcOrderOperation::CancelWork {
            operation_id: cancel.operation_id,
            handle: cancel.handle,
            expected_revision: cancel.expected_revision,
        },
        wire::NpcOrderOperation::IssueOrder(issue) => ScriptNpcOrderOperation::IssueOrder {
            operation_id: issue.operation_id,
            handles: issue.handles,
            expected_order_revisions: issue.expected_order_revisions,
            order: order(issue.order),
        },
        wire::NpcOrderOperation::CancelOrder(cancel) => ScriptNpcOrderOperation::CancelOrder {
            operation_id: cancel.operation_id,
            handles: cancel.handles,
            expected_order_revisions: cancel.expected_order_revisions,
        },
    })
}

/// One npc result, as the contract names it. The page is the core's
/// owner-scoped, bounded query response; it must not be renamed into a single
/// npc snapshot or silently dropped when the caller reads lifecycle.
pub(crate) fn encode_result(value: &ScriptNpcResult) -> Option<wire::NpcResult> {
    Some(match value {
        ScriptNpcResult::Snapshot { npc } => wire::NpcResult::Snapshot(snapshot(npc)?),
        ScriptNpcResult::Page { npcs, cursor } => wire::NpcResult::Page(wire::NpcPage {
            npcs: npcs.iter().map(snapshot).collect::<Option<Vec<_>>>()?,
            cursor: cursor.clone(),
        }),
        ScriptNpcResult::BirthPage {
            births,
            cursor,
            history_start,
            resume_cursor,
        } => wire::NpcResult::BirthPage(wire::NativeBirthPage {
            births: births
                .iter()
                .map(|birth| {
                    Some(wire::NativeBirthObservation {
                        entity_uuid: birth.entity_uuid.clone(),
                        origin: birth_origin(&birth.origin),
                        lifecycle: lifecycle(birth.lifecycle)?,
                        dimension: birth.dimension.clone(),
                        loaded: birth.loaded.as_ref().map(loaded_state),
                    })
                })
                .collect::<Option<Vec<_>>>()?,
            cursor: *cursor,
            history_start: *history_start,
            resume_cursor: *resume_cursor,
        }),
        ScriptNpcResult::HealthChanged { handle } => wire::NpcResult::HealthChanged(handle.clone()),
        _ => return None,
    })
}

/// One npc work or squad result, as the contract names it, or nothing when
/// the contract cannot express the server's own variant.
///
/// Every variant the current operations can commit is named here, refusals
/// included: a refused order still carries its per-member outcomes, so a plugin
/// reads which member was blocked, unloaded or stale instead of a bare refusal.
pub(crate) fn encode_order_result(value: &ScriptNpcOrderResult) -> Option<wire::NpcOrderResult> {
    Some(match value {
        ScriptNpcOrderResult::ItemUsed { handle, revision } => {
            wire::NpcOrderResult::ItemUsed(wire::ItemUsed {
                handle: handle.clone(),
                revision: *revision,
            })
        }
        ScriptNpcOrderResult::ItemDeployed {
            handle,
            entity_uuid,
            revision,
        } => wire::NpcOrderResult::ItemDeployed(wire::ItemCustodyChanged {
            handle: handle.clone(),
            entity_uuid: entity_uuid.clone(),
            revision: *revision,
        }),
        ScriptNpcOrderResult::ItemRetracted {
            handle,
            entity_uuid,
            revision,
        } => wire::NpcOrderResult::ItemRetracted(wire::ItemCustodyChanged {
            handle: handle.clone(),
            entity_uuid: entity_uuid.clone(),
            revision: *revision,
        }),
        ScriptNpcOrderResult::ItemCaptured {
            handle,
            entity_uuid,
            revision,
        } => wire::NpcOrderResult::ItemCaptured(wire::ItemCustodyChanged {
            handle: handle.clone(),
            entity_uuid: entity_uuid.clone(),
            revision: *revision,
        }),
        ScriptNpcOrderResult::Controls { npcs } => {
            wire::NpcOrderResult::Controls(wire::NpcControlPage {
                npcs: npcs
                    .iter()
                    .map(control_snapshot)
                    .collect::<Option<Vec<_>>>()?,
            })
        }
        ScriptNpcOrderResult::Work { assignment } => {
            wire::NpcOrderResult::Work(work_assignment(assignment)?)
        }
        ScriptNpcOrderResult::WorkCancelled { handle, revision } => {
            wire::NpcOrderResult::WorkCancelled(wire::WorkCancelled {
                handle: handle.clone(),
                revision: *revision,
            })
        }
        ScriptNpcOrderResult::Order {
            order_revision,
            members,
            combat,
        } => wire::NpcOrderResult::Order(wire::OrderResult {
            order_revision: *order_revision,
            members: members
                .iter()
                .map(member_outcome)
                .collect::<Option<Vec<_>>>()?,
            combat: combat.iter().map(combat_event).collect(),
        }),
        ScriptNpcOrderResult::OrderCancelled {
            order_revision,
            members,
        } => wire::NpcOrderResult::OrderCancelled(wire::OrderCancelled {
            order_revision: *order_revision,
            members: members
                .iter()
                .map(member_outcome)
                .collect::<Option<Vec<_>>>()?,
        }),
        _ => return None,
    })
}

/// The longest text one npc record carries, for the staging bound the host
/// checks a callback's answer against.
///
/// A record carries several strings rather than one, and the server's own DTO
/// already bounds every one of them, so the record is bounded by construction
/// and charging every string here would refuse a record the DTO admits.
/// What this answers is the longest single string the
/// guest put in the record, exactly as the rest of a staged batch is charged its
/// longest string rather than its whole content.
pub(crate) fn max_text_bytes(value: &wire::NpcOperation) -> usize {
    match value {
        wire::NpcOperation::Adopt(claim) => claim.entity_uuid.len().max(claim.operation_id.len()),
        wire::NpcOperation::AdoptNativeChild(claim) => claim
            .entity_uuid
            .len()
            .max(claim.operation_id.len())
            .max(claim.parent_handles.0.len())
            .max(claim.parent_handles.1.len()),
        wire::NpcOperation::Spawn(spawn) => spawn
            .operation_id
            .len()
            .max(spawn.generation_id.len())
            .max(spawn.entity_type.len()),
        wire::NpcOperation::Query(query) => query
            .handles
            .iter()
            .map(String::len)
            .chain(query.cursor.as_ref().map(String::len))
            .max()
            .unwrap_or(0),
        wire::NpcOperation::QueryBirths(query) => query
            .parent_handles
            .iter()
            .map(String::len)
            .max()
            .unwrap_or(0),
        wire::NpcOperation::Release(release) => {
            release.operation_id.len().max(release.handle.len())
        }
        wire::NpcOperation::ApplyHealthDelta(effect) => effect
            .operation_id
            .len()
            .max(effect.handle.len())
            .max(endpoint_text_bytes(&effect.source))
            .max(effect.material.resource_id.len()),
    }
}

/// The longest text one npc work or squad record carries, for the same
/// staging bound.
pub(crate) fn max_order_text_bytes(value: &wire::NpcOrderOperation) -> usize {
    match value {
        wire::NpcOrderOperation::UseItem(input) => {
            input.operation_id.len().max(input.handle.len()).max(
                input
                    .execution_permit
                    .as_ref()
                    .map_or(0, |permit| permit.route_ref.len()),
            )
        }
        wire::NpcOrderOperation::DeployItem(input) => input
            .operation_id
            .len()
            .max(input.handle.len())
            .max(input.entity_uuid.len())
            .max(
                input
                    .execution_permit
                    .as_ref()
                    .map_or(0, |permit| permit.route_ref.len()),
            ),
        wire::NpcOrderOperation::RetractItem(input) => input
            .operation_id
            .len()
            .max(input.handle.len())
            .max(input.entity_uuid.len())
            .max(
                input
                    .execution_permit
                    .as_ref()
                    .map_or(0, |permit| permit.route_ref.len()),
            ),
        wire::NpcOrderOperation::CaptureItem(input) => input
            .operation_id
            .len()
            .max(input.handle.len())
            .max(input.entity_uuid.len())
            .max(
                input
                    .execution_permit
                    .as_ref()
                    .map_or(0, |permit| permit.route_ref.len()),
            ),
        wire::NpcOrderOperation::Query(query) => {
            query.handles.iter().map(String::len).max().unwrap_or(0)
        }
        wire::NpcOrderOperation::SuspendWork(suspend) => suspend
            .operation_id
            .len()
            .max(suspend.handle.len())
            .max(suspend.admitted_operation_id.len())
            .max(suspend.reason.len()),
        wire::NpcOrderOperation::ResumeWork(resume) => resume
            .operation_id
            .len()
            .max(resume.handle.len())
            .max(resume.admitted_operation_id.len()),
        wire::NpcOrderOperation::AckWorkReceipts(ack) => ack
            .operation_id
            .len()
            .max(ack.handle.len())
            .max(ack.admitted_operation_id.len()),
        wire::NpcOrderOperation::AssignWork(assign) => assign
            .operation_id
            .len()
            .max(assign.handle.len())
            .max(work_text_bytes(&assign.work)),
        wire::NpcOrderOperation::CancelWork(cancel) => {
            cancel.operation_id.len().max(cancel.handle.len())
        }
        wire::NpcOrderOperation::IssueOrder(issue) => {
            let mut longest = handle_text_bytes(&issue.operation_id, &issue.handles);
            longest = longest.max(order_text_bytes(&issue.order));
            longest
        }
        wire::NpcOrderOperation::CancelOrder(cancel) => {
            handle_text_bytes(&cancel.operation_id, &cancel.handles)
        }
    }
}

/// The longest of an operation id and the durable handles of one batch.
fn handle_text_bytes(operation_id: &str, handles: &[String]) -> usize {
    handles.iter().fold(operation_id.len(), |longest, handle| {
        longest.max(handle.len())
    })
}

/// The longest string one work order carries.
fn work_text_bytes(value: &wire::WorkOrder) -> usize {
    match value {
        wire::WorkOrder::Harvest(work)
        | wire::WorkOrder::CutTree(work)
        | wire::WorkOrder::Mine(work) => work.tool.len().max(work.area.dimension.len()),
        wire::WorkOrder::Replant(work) => work
            .tool
            .len()
            .max(work.seed.len())
            .max(work.area.dimension.len()),
        wire::WorkOrder::TendLivestock(work) => work.feed.len().max(work.area.dimension.len()),
        wire::WorkOrder::Haul(haul) => endpoint_text_bytes(&haul.source)
            .max(endpoint_text_bytes(&haul.destination))
            .max(haul.item.as_ref().map_or(0, String::len)),
        wire::WorkOrder::Craft(craft) => craft.recipe.len().max(craft.station.dimension.len()),
        wire::WorkOrder::Construct(construct) => {
            construct.structure_id.len().max(construct.stage.len())
        }
    }
}

/// The longest string one squad order carries.
///
/// A formation, a heading, a spacing and an anchor are numbers and closed enums,
/// so the orders built only from them carry no text at all and answer zero. The
/// rest name resources, posts or target references, which the server's own DTO
/// bounds.
fn order_text_bytes(value: &wire::Order) -> usize {
    match value {
        wire::Order::Follow(_)
        | wire::Order::Hold(_)
        | wire::Order::Patrol(_)
        | wire::Order::Retreat(_) => 0,
        wire::Order::Move(move_order) => move_order.dimension.len(),
        wire::Order::Attack(attack) => {
            let mut longest = attack
                .targets
                .iter()
                .fold(0, |longest, target| longest.max(target.target_ref.len()));
            for ally in &attack.policy.allies {
                longest = longest.max(ally.len());
            }
            longest
        }
    }
}

/// The strings one inventory endpoint carries. A player endpoint is a number and
/// carries none.
fn endpoint_text_bytes(value: &wire_inventories::InventoryEndpoint) -> usize {
    match value {
        wire_inventories::InventoryEndpoint::PlayerInventory(_) => 0,
        wire_inventories::InventoryEndpoint::Container(container) => container.dimension.len(),
        wire_inventories::InventoryEndpoint::NpcEquipment(handle)
        | wire_inventories::InventoryEndpoint::NpcCarry(handle) => handle.len(),
    }
}

/// One contract work order as the server's own.
fn work_order(value: wire::WorkOrder) -> ScriptNpcWorkOrder {
    let area = |work: wire::AreaToolWork| (work.area, work.tool);
    match value {
        wire::WorkOrder::Harvest(work) => {
            let (area, tool) = area(work);
            ScriptNpcWorkOrder::Harvest {
                area: work_area(area),
                tool,
            }
        }
        wire::WorkOrder::CutTree(work) => {
            let (area, tool) = area(work);
            ScriptNpcWorkOrder::CutTree {
                area: work_area(area),
                tool,
            }
        }
        wire::WorkOrder::Mine(work) => {
            let (area, tool) = area(work);
            ScriptNpcWorkOrder::Mine {
                area: work_area(area),
                tool,
            }
        }
        wire::WorkOrder::Replant(work) => ScriptNpcWorkOrder::Replant {
            area: work_area(work.area),
            seed: work.seed,
            tool: work.tool,
        },
        wire::WorkOrder::TendLivestock(work) => ScriptNpcWorkOrder::TendLivestock {
            area: work_area(work.area),
            feed: work.feed,
        },
        wire::WorkOrder::Haul(haul) => ScriptNpcWorkOrder::Haul {
            source: endpoint(haul.source),
            destination: endpoint(haul.destination),
            item: haul.item,
        },
        wire::WorkOrder::Craft(craft) => ScriptNpcWorkOrder::Craft {
            recipe: craft.recipe,
            count: craft.count,
            station: work_area(craft.station),
        },
        wire::WorkOrder::Construct(construct) => ScriptNpcWorkOrder::Construct {
            structure_id: construct.structure_id,
            stage: construct.stage,
            expected_revision: construct.expected_revision,
        },
    }
}

/// One contract squad order as the server's own.
fn order(value: wire::Order) -> ScriptNpcOrder {
    match value {
        wire::Order::Follow(follow) => ScriptNpcOrder::Follow {
            target_player: follow.target_player,
            formation: formation(follow.formation),
        },
        wire::Order::Move(move_order) => ScriptNpcOrder::Move {
            dimension: move_order.dimension,
            anchor: position(move_order.anchor),
            heading_degrees: move_order.heading_degrees,
            formation: formation(move_order.formation),
        },
        wire::Order::Hold(hold) => ScriptNpcOrder::Hold {
            anchor: position(hold.anchor),
            heading_degrees: hold.heading_degrees,
            formation: formation(hold.formation),
            engagement_radius: hold.engagement_radius,
        },
        wire::Order::Patrol(patrol) => ScriptNpcOrder::Patrol {
            waypoints: patrol.waypoints.into_iter().map(position).collect(),
            formation: formation(patrol.formation),
            engagement_radius: patrol.engagement_radius,
        },
        wire::Order::Attack(attack) => ScriptNpcOrder::Attack {
            targets: attack
                .targets
                .into_iter()
                .map(|target| {
                    ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )
                })
                .collect(),
            policy: {
                ScriptEngagementPolicy::new(
                    attack.policy.revision,
                    attack.policy.allies,
                    attack
                        .policy
                        .permitted
                        .into_iter()
                        .map(hostile_category)
                        .collect(),
                )
            },
        },
        wire::Order::Retreat(retreat) => ScriptNpcOrder::Retreat {
            anchor: position(retreat.anchor),
            formation: formation(retreat.formation),
        },
    }
}

/// One contract block cell as the server's own.
fn position(value: wire::BlockPosition) -> ScriptBlockPosition {
    ScriptBlockPosition::new(value.x, value.y, value.z)
}

/// One contract work area as the server's own.
fn work_area(value: wire::WorkArea) -> ScriptWorkArea {
    ScriptWorkArea::new(value.dimension, position(value.min), position(value.max))
}

/// One inventory endpoint of a haul as the server's own.
fn endpoint(value: wire_inventories::InventoryEndpoint) -> ScriptInventoryEndpoint {
    match value {
        wire_inventories::InventoryEndpoint::PlayerInventory(player_id) => {
            ScriptInventoryEndpoint::PlayerInventory { player_id }
        }
        wire_inventories::InventoryEndpoint::Container(container) => {
            ScriptInventoryEndpoint::Container {
                dimension: container.dimension,
                position: ScriptBlockPosition::new(
                    container.position.x,
                    container.position.y,
                    container.position.z,
                ),
            }
        }
        wire_inventories::InventoryEndpoint::NpcEquipment(handle) => {
            ScriptInventoryEndpoint::NpcEquipment { handle }
        }
        wire_inventories::InventoryEndpoint::NpcCarry(handle) => {
            ScriptInventoryEndpoint::NpcCarry { handle }
        }
    }
}

/// One contract formation as the server's own.
fn formation(value: wire::Formation) -> ScriptFormation {
    ScriptFormation::new(formation_kind(value.kind), value.spacing)
}

/// One contract formation kind as the server's own.
fn formation_kind(value: wire::FormationKind) -> ScriptFormationKind {
    match value {
        wire::FormationKind::Line => ScriptFormationKind::Line,
        wire::FormationKind::Column => ScriptFormationKind::Column,
        wire::FormationKind::Wedge => ScriptFormationKind::Wedge,
        wire::FormationKind::Square => ScriptFormationKind::Square,
    }
}

/// One contract hostile category as the server's own.
fn hostile_category(value: wire::HostileCategory) -> ScriptHostileCategory {
    match value {
        wire::HostileCategory::Hostile => ScriptHostileCategory::Hostile,
        wire::HostileCategory::Player => ScriptHostileCategory::Player,
        wire::HostileCategory::OwnedNpc => ScriptHostileCategory::OwnedNpc,
        wire::HostileCategory::NeutralAnimal => ScriptHostileCategory::NeutralAnimal,
    }
}

/// One server npc snapshot as the contract names it, or nothing when a
/// member the contract does not name appears.
fn snapshot(value: &ScriptNpcSnapshot) -> Option<wire::NpcSnapshot> {
    Some(wire::NpcSnapshot {
        handle: value.handle.clone(),
        entity_uuid: value.entity_uuid.clone(),
        lifecycle: lifecycle(value.lifecycle)?,
        revision: value.revision,
        generation_id: value.generation_id.clone(),
        loaded: value.loaded.as_ref().map(loaded_state),
    })
}

/// One server lifecycle as the contract names it.
fn lifecycle(value: ScriptNpcLifecycle) -> Option<wire::NpcLifecycle> {
    Some(match value {
        ScriptNpcLifecycle::AliveLoaded => wire::NpcLifecycle::AliveLoaded,
        ScriptNpcLifecycle::AliveUnloaded => wire::NpcLifecycle::AliveUnloaded,
        ScriptNpcLifecycle::Dead => wire::NpcLifecycle::Dead,
        ScriptNpcLifecycle::Released => wire::NpcLifecycle::Released,
        _ => return None,
    })
}

fn birth_origin(value: &mc_script::ScriptNativeBirthOrigin) -> wire::NativeBirthOrigin {
    wire::NativeBirthOrigin {
        parents: (value.parents[0].clone(), value.parents[1].clone()),
        started_tick: value.started_tick,
        birth_tick: value.birth_tick,
    }
}

/// One server loaded state as the contract names it. The carried summaries are a
/// bounded read of the live entity and carry no variant of their own.
fn loaded_state(value: &ScriptNpcLoadedState) -> wire::NpcLoadedState {
    wire::NpcLoadedState {
        dimension: value.dimension.clone(),
        x: value.x,
        y: value.y,
        z: value.z,
        health: value.health,
        max_health: value.max_health,
        yaw: value.yaw,
        native_age_ticks: value.native_age_ticks,
        birth_origin: value.birth_origin.as_ref().map(birth_origin),
        carried: value
            .carried
            .iter()
            .map(|item: &ScriptNpcItemSummary| wire::NpcItemSummary {
                item_id: item.item_id.clone(),
                count: item.count,
            })
            .collect(),
    }
}

/// One server work assignment as the contract names it, or nothing when a state
/// or pause reason the contract does not name appears.
fn work_assignment(value: &ScriptWorkAssignment) -> Option<wire::WorkAssignment> {
    Some(wire::WorkAssignment {
        handle: value.handle.clone(),
        operation_id: value.operation_id.clone(),
        receipts: value
            .receipts
            .iter()
            .map(craft_receipt)
            .collect::<Option<Vec<_>>>()?,
        acknowledged_sequence: value.acknowledged_sequence,
        due_tick: value.due_tick,
        state: work_state(value.state)?,
        reason: match value.reason {
            Some(reason) => Some(pause_reason(reason)?),
            None => None,
        },
        suspension_reason: value.suspension_reason.clone(),
        work_units_done: value.work_units_done,
        work_units_planned: value.work_units_planned,
        changes: value.changes.iter().map(item_change).collect(),
        revision: value.revision,
    })
}

fn craft_stack(value: &mc_script::ScriptCraftStack) -> Option<wire::CraftStack> {
    Some(wire::CraftStack {
        endpoint: crate::domain_inventories::contract_endpoint(&value.endpoint)?,
        slot: value.slot,
        item: crate::domain_inventories::item(&value.item)?,
    })
}

fn craft_receipt(value: &mc_script::ScriptCraftReceipt) -> Option<wire::CraftReceipt> {
    let fences = |values: &[mc_script::ScriptInventoryExpectedRevision]| {
        values
            .iter()
            .map(|value| {
                Some(wire_inventories::InventoryExpectedRevision {
                    endpoint: crate::domain_inventories::contract_endpoint(&value.endpoint)?,
                    fence: wire_inventories::InventoryFence {
                        revision: value.fence.revision,
                        snapshot_hash: value.fence.snapshot_hash.clone(),
                    },
                })
            })
            .collect::<Option<Vec<_>>>()
    };
    Some(wire::CraftReceipt {
        operation_id: value.operation_id.clone(),
        handle: value.handle.clone(),
        portion_id: value.portion_id.clone(),
        sequence: value.sequence,
        recipe: value.recipe.clone(),
        recipe_revision: value.recipe_revision.clone(),
        seed: value.seed,
        simulation_tick: value.simulation_tick,
        duration_ticks: value.duration_ticks,
        station: wire::WorkArea {
            dimension: value.station.dimension.clone(),
            min: wire_position(value.station.min),
            max: wire_position(value.station.max),
        },
        inputs: value
            .inputs
            .iter()
            .map(craft_stack)
            .collect::<Option<Vec<_>>>()?,
        outputs: value
            .outputs
            .iter()
            .map(craft_stack)
            .collect::<Option<Vec<_>>>()?,
        wear: value
            .wear
            .iter()
            .map(|wear| {
                Some(wire::CraftToolWear {
                    before: craft_stack(&wear.before)?,
                    after: match &wear.after {
                        Some(item) => Some(crate::domain_inventories::item(item)?),
                        None => None,
                    },
                    damage: wear.damage,
                })
            })
            .collect::<Option<Vec<_>>>()?,
        before: fences(&value.before)?,
        after: fences(&value.after)?,
        revision: value.revision,
    })
}

/// One server work state as the contract names it.
fn work_state(value: ScriptWorkState) -> Option<wire::WorkState> {
    Some(match value {
        ScriptWorkState::Accepted => wire::WorkState::Accepted,
        ScriptWorkState::Running => wire::WorkState::Running,
        ScriptWorkState::Paused => wire::WorkState::Paused,
        ScriptWorkState::Suspended => wire::WorkState::Suspended,
        ScriptWorkState::Committed => wire::WorkState::Committed,
        ScriptWorkState::Cancelled => wire::WorkState::Cancelled,
        _ => return None,
    })
}

/// One server pause reason as the contract names it.
fn pause_reason(value: ScriptWorkPauseReason) -> Option<wire::WorkPauseReason> {
    Some(match value {
        ScriptWorkPauseReason::Unloaded => wire::WorkPauseReason::Unloaded,
        ScriptWorkPauseReason::NoWorkers => wire::WorkPauseReason::NoWorkers,
        ScriptWorkPauseReason::MissingInput => wire::WorkPauseReason::MissingInput,
        ScriptWorkPauseReason::MissingTool => wire::WorkPauseReason::MissingTool,
        ScriptWorkPauseReason::MissingStation => wire::WorkPauseReason::MissingStation,
        ScriptWorkPauseReason::BlockedRoute => wire::WorkPauseReason::BlockedRoute,
        ScriptWorkPauseReason::Interrupted => wire::WorkPauseReason::Interrupted,
        ScriptWorkPauseReason::Protected => wire::WorkPauseReason::Protected,
        ScriptWorkPauseReason::Unsupported => wire::WorkPauseReason::Unsupported,
        ScriptWorkPauseReason::NoStorage => wire::WorkPauseReason::NoStorage,
        ScriptWorkPauseReason::ReceiptBackpressure => wire::WorkPauseReason::ReceiptBackpressure,
        _ => return None,
    })
}

/// One server item change as the contract names it.
fn item_change(value: &ScriptItemChange) -> wire::ItemChange {
    wire::ItemChange {
        item_id: value.item_id.clone(),
        delta: value.delta,
    }
}

/// One server member outcome as the contract names it, or nothing when a state
/// the contract does not name appears.
fn member_outcome(value: &ScriptOrderMemberOutcome) -> Option<wire::OrderMemberOutcome> {
    Some(wire::OrderMemberOutcome {
        handle: value.handle.clone(),
        state: member_state(value.state)?,
        formation_slot: value.formation_slot,
        targets: value
            .targets
            .iter()
            .map(order_target)
            .collect::<Option<Vec<_>>>()?,
    })
}

/// One server member state as the contract names it.
fn member_state(value: ScriptOrderMemberState) -> Option<wire::OrderMemberState> {
    Some(match value {
        ScriptOrderMemberState::Applied => wire::OrderMemberState::Applied,
        ScriptOrderMemberState::BlockedRoute => wire::OrderMemberState::BlockedRoute,
        ScriptOrderMemberState::Unloaded => wire::OrderMemberState::Unloaded,
        ScriptOrderMemberState::Dead => wire::OrderMemberState::Dead,
        ScriptOrderMemberState::Migrating => wire::OrderMemberState::Migrating,
        ScriptOrderMemberState::Forbidden => wire::OrderMemberState::Forbidden,
        ScriptOrderMemberState::StaleRevision => wire::OrderMemberState::StaleRevision,
        _ => return None,
    })
}

/// One server order target as the contract names it.
fn order_target(value: &ScriptOrderTarget) -> Option<wire::OrderTarget> {
    Some(wire::OrderTarget {
        target_ref: value.target_ref.clone(),
        policy_revision: value.policy_revision,
        expires_revision: value.expires_revision,
        category: contract_hostile_category(value.category)?,
        position: wire::BlockPosition {
            x: value.position.x,
            y: value.position.y,
            z: value.position.z,
        },
    })
}

/// One server hostile category as the contract names it.
fn contract_hostile_category(value: ScriptHostileCategory) -> Option<wire::HostileCategory> {
    Some(match value {
        ScriptHostileCategory::Hostile => wire::HostileCategory::Hostile,
        ScriptHostileCategory::Player => wire::HostileCategory::Player,
        ScriptHostileCategory::OwnedNpc => wire::HostileCategory::OwnedNpc,
        ScriptHostileCategory::NeutralAnimal => wire::HostileCategory::NeutralAnimal,
        _ => return None,
    })
}

/// One server combat event as the contract names it. A committed combat outcome
/// carries no closed variant of its own beyond the category above.
fn combat_event(value: &ScriptCombatEvent) -> wire::CombatEvent {
    wire::CombatEvent {
        event_id: value.event_id,
        revision: value.revision,
        attacker_handle: value.attacker_handle.clone(),
        victim_target_ref: value.victim_target_ref.clone(),
        order_revision: value.order_revision,
        damage_milli: value.damage_milli,
        killed: value.killed,
    }
}

fn control_snapshot(value: &ScriptNpcControlSnapshot) -> Option<wire::NpcControlSnapshot> {
    Some(wire::NpcControlSnapshot {
        handle: value.handle.clone(),
        revision: value.revision,
        order_revision: value.order_revision,
        order: match &value.order {
            Some(order) => Some(wire_order(order)?),
            None => None,
        },
        work: match &value.work {
            Some(work) => Some(work_assignment(work)?),
            None => None,
        },
        death_cargo: match &value.death_cargo {
            Some(receipt) => Some(super::domain_inventories::encode_npc_death_cargo_receipt(
                receipt,
            )?),
            None => None,
        },
        pending_work: match &value.pending_work {
            Some(pending) => Some(wire::NpcPendingWork {
                work: wire_work_order(&pending.work)?,
                work_units: pending.work_units,
            }),
            None => None,
        },
        item_use: value
            .item_use
            .as_ref()
            .map(|input| wire::NpcItemUseControl {
                equipment_slot: input.equipment_slot,
                started_tick: input.started_tick,
                disabled_until: input.disabled_until,
            }),
        targets: value
            .targets
            .iter()
            .map(|target| wire::NpcTargetObservation {
                target_ref: target.target_ref.clone(),
                position: target.position.map(|at| {
                    crate::bindings::solaris::plugin::types::Position {
                        x: at.x(),
                        y: at.y(),
                        z: at.z(),
                    }
                }),
                alive: target.alive,
            })
            .collect(),
    })
}

fn wire_position(value: ScriptBlockPosition) -> wire::BlockPosition {
    wire::BlockPosition {
        x: value.x,
        y: value.y,
        z: value.z,
    }
}

fn wire_formation(value: ScriptFormation) -> Option<wire::Formation> {
    let kind = match value.kind {
        ScriptFormationKind::Line => wire::FormationKind::Line,
        ScriptFormationKind::Column => wire::FormationKind::Column,
        ScriptFormationKind::Wedge => wire::FormationKind::Wedge,
        ScriptFormationKind::Square => wire::FormationKind::Square,
        _ => return None,
    };
    Some(wire::Formation {
        kind,
        spacing: value.spacing,
    })
}

fn wire_order(value: &ScriptNpcOrder) -> Option<wire::Order> {
    Some(match value {
        ScriptNpcOrder::Follow {
            target_player,
            formation,
        } => wire::Order::Follow(wire::FollowOrder {
            target_player: *target_player,
            formation: wire_formation(*formation)?,
        }),
        ScriptNpcOrder::Move {
            dimension,
            anchor,
            heading_degrees,
            formation,
        } => wire::Order::Move(wire::MoveOrder {
            dimension: dimension.clone(),
            anchor: wire_position(*anchor),
            heading_degrees: *heading_degrees,
            formation: wire_formation(*formation)?,
        }),
        ScriptNpcOrder::Hold {
            anchor,
            heading_degrees,
            formation,
            engagement_radius,
        } => wire::Order::Hold(wire::HoldOrder {
            anchor: wire_position(*anchor),
            heading_degrees: *heading_degrees,
            formation: wire_formation(*formation)?,
            engagement_radius: *engagement_radius,
        }),
        ScriptNpcOrder::Patrol {
            waypoints,
            formation,
            engagement_radius,
        } => wire::Order::Patrol(wire::PatrolOrder {
            waypoints: waypoints.iter().copied().map(wire_position).collect(),
            formation: wire_formation(*formation)?,
            engagement_radius: *engagement_radius,
        }),
        ScriptNpcOrder::Attack { targets, policy } => wire::Order::Attack(wire::AttackOrder {
            targets: targets
                .iter()
                .map(|target| wire::OrderTargetRef {
                    target_ref: target.target_ref.clone(),
                    policy_revision: target.policy_revision,
                    expires_revision: target.expires_revision,
                })
                .collect(),
            policy: wire::EngagementPolicy {
                revision: policy.revision,
                allies: policy.allies.clone(),
                permitted: policy
                    .permitted
                    .iter()
                    .copied()
                    .map(contract_hostile_category)
                    .collect::<Option<Vec<_>>>()?,
            },
        }),
        ScriptNpcOrder::Retreat { anchor, formation } => wire::Order::Retreat(wire::RetreatOrder {
            anchor: wire_position(*anchor),
            formation: wire_formation(*formation)?,
        }),
        _ => return None,
    })
}

fn wire_work_area(value: &ScriptWorkArea) -> wire::WorkArea {
    wire::WorkArea {
        dimension: value.dimension.clone(),
        min: wire_position(value.min),
        max: wire_position(value.max),
    }
}

fn wire_endpoint(value: &ScriptInventoryEndpoint) -> Option<wire_inventories::InventoryEndpoint> {
    Some(match value {
        ScriptInventoryEndpoint::PlayerInventory { player_id } => {
            wire_inventories::InventoryEndpoint::PlayerInventory(*player_id)
        }
        ScriptInventoryEndpoint::NpcEquipment { handle } => {
            wire_inventories::InventoryEndpoint::NpcEquipment(handle.clone())
        }
        ScriptInventoryEndpoint::NpcCarry { handle } => {
            wire_inventories::InventoryEndpoint::NpcCarry(handle.clone())
        }
        ScriptInventoryEndpoint::Container {
            dimension,
            position,
        } => wire_inventories::InventoryEndpoint::Container(wire_inventories::ContainerEndpoint {
            dimension: dimension.clone(),
            position: wire_position(*position),
        }),
        _ => return None,
    })
}

fn wire_work_order(value: &ScriptNpcWorkOrder) -> Option<wire::WorkOrder> {
    Some(match value {
        ScriptNpcWorkOrder::Harvest { area, tool } => {
            wire::WorkOrder::Harvest(wire::AreaToolWork {
                area: wire_work_area(area),
                tool: tool.clone(),
            })
        }
        ScriptNpcWorkOrder::CutTree { area, tool } => {
            wire::WorkOrder::CutTree(wire::AreaToolWork {
                area: wire_work_area(area),
                tool: tool.clone(),
            })
        }
        ScriptNpcWorkOrder::Mine { area, tool } => wire::WorkOrder::Mine(wire::AreaToolWork {
            area: wire_work_area(area),
            tool: tool.clone(),
        }),
        ScriptNpcWorkOrder::Replant { area, seed, tool } => {
            wire::WorkOrder::Replant(wire::ReplantWork {
                area: wire_work_area(area),
                seed: seed.clone(),
                tool: tool.clone(),
            })
        }
        ScriptNpcWorkOrder::TendLivestock { area, feed } => {
            wire::WorkOrder::TendLivestock(wire::TendLivestockWork {
                area: wire_work_area(area),
                feed: feed.clone(),
            })
        }
        ScriptNpcWorkOrder::Haul {
            source,
            destination,
            item,
        } => wire::WorkOrder::Haul(wire::HaulWork {
            source: wire_endpoint(source)?,
            destination: wire_endpoint(destination)?,
            item: item.clone(),
        }),
        ScriptNpcWorkOrder::Craft {
            recipe,
            count,
            station,
        } => wire::WorkOrder::Craft(wire::CraftWork {
            recipe: recipe.clone(),
            count: *count,
            station: wire_work_area(station),
        }),
        ScriptNpcWorkOrder::Construct {
            structure_id,
            stage,
            expected_revision,
        } => wire::WorkOrder::Construct(wire::ConstructWork {
            structure_id: structure_id.clone(),
            stage: stage.clone(),
            expected_revision: *expected_revision,
        }),
        _ => return None,
    })
}
