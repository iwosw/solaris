//! Native work progress counts only committed physical deltas.
use super::*;
use crate::plugin_inventories as inventories;
use inventories::InventoryImage;
use mc_data::{Identifier, ItemStack};
mod craft;

pub(super) fn execute(
    state: &mut TickState,
    id: &str,
    operation: &ScriptNpcOrderOperation,
) -> Result<ScriptOperationOutcome, ScriptOperationFailure> {
    let mut all = records(state, id)?;
    let revision = next_revision(state, id);
    let (handle, expected) = match operation {
        ScriptNpcOrderOperation::AssignWork {
            handle,
            expected_revision,
            ..
        }
        | ScriptNpcOrderOperation::CancelWork {
            handle,
            expected_revision,
            ..
        }
        | ScriptNpcOrderOperation::SuspendWork {
            handle,
            expected_revision,
            ..
        }
        | ScriptNpcOrderOperation::ResumeWork {
            handle,
            expected_revision,
            ..
        }
        | ScriptNpcOrderOperation::AckWorkReceipts {
            handle,
            expected_revision,
            ..
        } => (handle, *expected_revision),
        _ => return Err(ScriptOperationFailure::InvalidRequest),
    };
    validate_permit(state, id, handle, None)?;
    let record = all
        .get_mut(handle)
        .ok_or(ScriptOperationFailure::Forbidden)?;
    if record.controls.revision != expected {
        return Err(ScriptOperationFailure::StaleRevision);
    }
    if record.disposition != ScriptNpcLifecycle::AliveUnloaded {
        return Err(ScriptOperationFailure::NotFound);
    }
    let result = match operation {
        ScriptNpcOrderOperation::AssignWork {
            operation_id,
            work,
            work_units,
            ..
        } => {
            if record.controls.pending_work.is_some() {
                return Err(ScriptOperationFailure::Busy);
            }
            let reason = match owned_slot(state, id, handle) {
                Ok(slot) => prerequisites(state, id, record, slot, work).err(),
                Err(_) => Some(ScriptWorkPauseReason::Unloaded),
            };
            let mut assignment = ScriptWorkAssignment::new(
                handle.clone(),
                if reason.is_some() {
                    ScriptWorkState::Paused
                } else {
                    ScriptWorkState::Accepted
                },
                reason,
                0,
                match work {
                    ScriptNpcWorkOrder::Craft { count, .. } => (*work_units).min(u64::from(*count)),
                    _ => *work_units,
                },
                Vec::new(),
                revision,
            );
            assignment.operation_id = Some(operation_id.clone());
            assignment.due_tick = reason
                .is_none()
                .then_some(state.tick + duration(state, work));
            record.controls.work = Some(assignment.clone());
            record.controls.pending_work = Some(ScriptNpcPendingWork::new(
                work.clone(),
                assignment.work_units_planned,
            ));
            record.controls.revision = revision;
            record.work_cursor = 0;
            ScriptNpcOrderResult::Work {
                assignment: Box::new(assignment),
            }
        }
        ScriptNpcOrderOperation::CancelWork { .. } => {
            record.controls.pending_work = None;
            if let Some(work) = &mut record.controls.work {
                work.state = ScriptWorkState::Cancelled;
                work.reason = None;
                work.suspension_reason = None;
                work.due_tick = None;
                work.revision = revision;
            }
            record.controls.revision = revision;
            if let Ok(slot) = owned_slot(state, id, handle) {
                orders::clear_navigation(state, slot);
            }
            ScriptNpcOrderResult::WorkCancelled {
                handle: handle.clone(),
                revision,
            }
        }
        ScriptNpcOrderOperation::SuspendWork {
            admitted_operation_id,
            reason,
            ..
        } => {
            let work = record
                .controls
                .work
                .as_mut()
                .ok_or(ScriptOperationFailure::NotFound)?;
            if work.operation_id.as_ref() != Some(admitted_operation_id) {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            if record.controls.pending_work.is_none() {
                return Err(ScriptOperationFailure::Busy);
            }
            work.state = ScriptWorkState::Suspended;
            work.reason = None;
            work.suspension_reason = Some(reason.clone());
            work.due_tick = None;
            work.revision = revision;
            record.controls.revision = revision;
            if let Ok(slot) = owned_slot(state, id, handle) {
                orders::clear_navigation(state, slot);
            }
            ScriptNpcOrderResult::Work {
                assignment: Box::new(work.clone()),
            }
        }
        ScriptNpcOrderOperation::ResumeWork {
            admitted_operation_id,
            ..
        } => {
            let work = record
                .controls
                .work
                .as_mut()
                .ok_or(ScriptOperationFailure::NotFound)?;
            if work.operation_id.as_ref() != Some(admitted_operation_id) {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            if work.state != ScriptWorkState::Suspended {
                return Err(ScriptOperationFailure::Busy);
            }
            work.state = ScriptWorkState::Accepted;
            work.reason = None;
            work.suspension_reason = None;
            work.due_tick = Some(
                state.tick
                    + duration(
                        state,
                        &record
                            .controls
                            .pending_work
                            .as_ref()
                            .ok_or(ScriptOperationFailure::Busy)?
                            .work,
                    ),
            );
            work.revision = revision;
            record.controls.revision = revision;
            ScriptNpcOrderResult::Work {
                assignment: Box::new(work.clone()),
            }
        }
        ScriptNpcOrderOperation::AckWorkReceipts {
            admitted_operation_id,
            through_sequence,
            ..
        } => {
            let work = record
                .controls
                .work
                .as_mut()
                .ok_or(ScriptOperationFailure::NotFound)?;
            if work.operation_id.as_ref() != Some(admitted_operation_id)
                || *through_sequence < work.acknowledged_sequence
                || *through_sequence > work.work_units_done
            {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            work.receipts
                .retain(|receipt| receipt.sequence > *through_sequence);
            work.acknowledged_sequence = *through_sequence;
            work.revision = revision;
            record.controls.revision = revision;
            ScriptNpcOrderResult::Work {
                assignment: Box::new(work.clone()),
            }
        }
        _ => return Err(ScriptOperationFailure::InvalidRequest),
    };
    save_records(state, id, &all, revision);
    Ok(order_result(revision, result))
}

fn duration(state: &TickState, work: &ScriptNpcWorkOrder) -> u64 {
    if let ScriptNpcWorkOrder::Craft { recipe, .. } = work {
        craft::recipe(state, recipe).map_or(20, |recipe| recipe.duration_ticks)
    } else {
        20
    }
}
fn carry(handle: &str) -> ScriptInventoryEndpoint {
    ScriptInventoryEndpoint::NpcCarry {
        handle: handle.to_owned(),
    }
}
fn equipment(handle: &str) -> ScriptInventoryEndpoint {
    ScriptInventoryEndpoint::NpcEquipment {
        handle: handle.to_owned(),
    }
}
fn resource(state: &TickState, stack: &ItemStack) -> Option<String> {
    inventories::to_script_item(state, stack)
        .ok()
        .map(|item| item.resource_id)
}
fn tools(
    state: &TickState,
    id: &str,
    handle: &str,
    name: &str,
) -> Result<(ScriptInventoryEndpoint, InventoryImage, u8), ScriptWorkPauseReason> {
    for endpoint in [equipment(handle), carry(handle)] {
        let image = inventories::load(state, id, &endpoint).map_err(reason)?;
        if let Some(slot) = image
            .slots
            .iter()
            .find(|(_, stack)| !stack.is_empty() && resource(state, stack).as_deref() == Some(name))
            .map(|(&slot, _)| slot)
        {
            return Ok((endpoint, image, slot));
        }
    }
    Err(ScriptWorkPauseReason::MissingTool)
}
fn prerequisites(
    state: &TickState,
    id: &str,
    record: &NpcRecord,
    _slot: EntitySlot,
    work: &ScriptNpcWorkOrder,
) -> Result<(), ScriptWorkPauseReason> {
    if record.controls.order.is_some() || routes::has_lease(state, id, &record.handle) {
        return Err(ScriptWorkPauseReason::Interrupted);
    }
    match work {
        ScriptNpcWorkOrder::Harvest { area, tool }
        | ScriptNpcWorkOrder::CutTree { area, tool }
        | ScriptNpcWorkOrder::Mine { area, tool }
        | ScriptNpcWorkOrder::Replant { area, tool, .. } => {
            if area.dimension != state.dimension.name() {
                return Err(ScriptWorkPauseReason::Unloaded);
            }
            tools(state, id, &record.handle, tool)?;
            if let ScriptNpcWorkOrder::Replant { seed, .. } = work {
                let image = inventories::load(state, id, &carry(&record.handle)).map_err(reason)?;
                if inventories::stock(state, &image)
                    .map_err(reason)?
                    .get(seed)
                    .copied()
                    .unwrap_or(0)
                    == 0
                {
                    return Err(ScriptWorkPauseReason::MissingInput);
                }
            }
        }
        ScriptNpcWorkOrder::TendLivestock { area, feed } => {
            if area.dimension != state.dimension.name() {
                return Err(ScriptWorkPauseReason::Unloaded);
            }
            let image = inventories::load(state, id, &carry(&record.handle)).map_err(reason)?;
            if inventories::stock(state, &image)
                .map_err(reason)?
                .get(feed)
                .copied()
                .unwrap_or(0)
                == 0
            {
                return Err(ScriptWorkPauseReason::MissingInput);
            }
        }
        ScriptNpcWorkOrder::Haul {
            source,
            destination,
            ..
        } => {
            inventories::load(state, id, source).map_err(reason)?;
            inventories::load(state, id, destination).map_err(reason)?;
        }
        ScriptNpcWorkOrder::Craft {
            recipe, station, ..
        } => {
            let recipe = craft::recipe(state, recipe)?;
            craft::station(state, station, &recipe)?;
            craft::plan(state, id, &record.handle, &recipe)?;
        }
        ScriptNpcWorkOrder::Construct { .. } => {}
        _ => return Err(ScriptWorkPauseReason::Unsupported),
    }
    Ok(())
}
pub(super) fn reason(failure: ScriptOperationFailure) -> ScriptWorkPauseReason {
    match failure {
        ScriptOperationFailure::Unloaded => ScriptWorkPauseReason::Unloaded,
        ScriptOperationFailure::InsufficientItems => ScriptWorkPauseReason::MissingInput,
        ScriptOperationFailure::Capacity => ScriptWorkPauseReason::NoStorage,
        ScriptOperationFailure::Forbidden => ScriptWorkPauseReason::Protected,
        ScriptOperationFailure::Busy => ScriptWorkPauseReason::Interrupted,
        _ => ScriptWorkPauseReason::BlockedRoute,
    }
}
fn pause(work: &mut ScriptWorkAssignment, reason: ScriptWorkPauseReason, revision: u64) -> bool {
    let changed = work.state != ScriptWorkState::Paused || work.reason != Some(reason);
    work.state = ScriptWorkState::Paused;
    work.reason = Some(reason);
    work.due_tick = None;
    if changed {
        work.revision = revision;
    }
    changed
}

pub(super) fn advance(
    state: &mut TickState,
    id: &str,
    record: &mut NpcRecord,
    slot: EntitySlot,
    revision: u64,
) -> bool {
    let Some(pending) = record.controls.pending_work.clone() else {
        return false;
    };
    let Some(mut assignment) = record.controls.work.take() else {
        return false;
    };
    if matches!(
        assignment.state,
        ScriptWorkState::Suspended | ScriptWorkState::Cancelled | ScriptWorkState::Committed
    ) {
        record.controls.work = Some(assignment);
        return false;
    }
    let result = prerequisites(state, id, record, slot, &pending.work);
    let mut changed = false;
    if let Err(reason) = result {
        changed = pause(&mut assignment, reason, revision);
    } else if assignment.receipts.len() == MAX_WORK_RECEIPTS {
        changed = pause(
            &mut assignment,
            ScriptWorkPauseReason::ReceiptBackpressure,
            revision,
        );
    } else if assignment.due_tick.is_none() {
        assignment.state = ScriptWorkState::Running;
        assignment.reason = None;
        assignment.due_tick = Some(state.tick + duration(state, &pending.work));
        assignment.revision = revision;
        changed = true;
    } else if assignment.due_tick.is_some_and(|due| state.tick >= due) {
        let result = match &pending.work {
            ScriptNpcWorkOrder::Craft {
                recipe, station, ..
            } => craft::perform(
                state,
                id,
                (record, slot),
                &mut assignment,
                recipe,
                station,
                revision,
            ),
            ScriptNpcWorkOrder::Haul {
                source,
                destination,
                item,
            } => haul(
                state,
                id,
                record,
                slot,
                source,
                destination,
                item.as_deref(),
            ),
            ScriptNpcWorkOrder::Harvest { .. }
            | ScriptNpcWorkOrder::CutTree { .. }
            | ScriptNpcWorkOrder::Mine { .. }
            | ScriptNpcWorkOrder::Replant { .. } => match physical_block_work(
                state,
                id,
                record,
                slot,
                &pending.work,
                assignment.revision,
            ) {
                Ok(Some(changes)) => Ok(changes),
                Ok(None) => {
                    record.controls.work = Some(assignment);
                    return false;
                }
                Err(reason) => Err(reason),
            },
            ScriptNpcWorkOrder::TendLivestock { area, feed } => {
                tend(state, id, record, slot, area, feed)
            }
            ScriptNpcWorkOrder::Construct {
                structure_id,
                stage,
                expected_revision,
            } => match crate::plugin_structures::advance_npc_work(
                state,
                id,
                &record.handle,
                structure_id,
                stage,
                *expected_revision,
                assignment.revision,
            ) {
                Ok(Some((_, structure_revision, changes))) => {
                    if let Some(ScriptNpcPendingWork {
                        work:
                            ScriptNpcWorkOrder::Construct {
                                expected_revision, ..
                            },
                        ..
                    }) = &mut record.controls.pending_work
                    {
                        *expected_revision = structure_revision;
                    }
                    Ok(changes)
                }
                Ok(None) => {
                    record.controls.work = Some(assignment);
                    return false;
                }
                Err(failure) => Err(reason(failure)),
            },
            _ => Err(ScriptWorkPauseReason::Unsupported),
        };
        match result {
            Ok(changes) => {
                if !matches!(pending.work, ScriptNpcWorkOrder::Haul { .. })
                    || record.work_cursor == 0
                {
                    assignment.work_units_done += 1;
                }
                for change in changes {
                    if let Some(existing) = assignment
                        .changes
                        .iter_mut()
                        .find(|existing| existing.item_id == change.item_id)
                    {
                        existing.delta += change.delta;
                    } else {
                        assignment.changes.push(change);
                    }
                }
                assignment.changes.retain(|change| change.delta != 0);
                assignment
                    .changes
                    .sort_unstable_by(|a, b| a.item_id.cmp(&b.item_id));
                assignment.reason = None;
                assignment.revision = revision;
                if assignment.work_units_done >= assignment.work_units_planned {
                    assignment.state = ScriptWorkState::Committed;
                    assignment.due_tick = None;
                    record.controls.pending_work = None;
                    orders::clear_navigation(state, slot);
                } else {
                    assignment.state = ScriptWorkState::Running;
                    assignment.due_tick = Some(state.tick + duration(state, &pending.work));
                    if let Some(pending) = &mut record.controls.pending_work {
                        pending.work_units =
                            assignment.work_units_planned - assignment.work_units_done;
                    }
                }
                changed = true;
            }
            Err(reason) => changed = pause(&mut assignment, reason, revision),
        }
    }
    if changed {
        record.controls.revision = revision;
    }
    record.controls.work = Some(assignment);
    changed
}

fn near(
    state: &mut TickState,
    slot: EntitySlot,
    point: ScriptBlockPosition,
) -> Result<(), ScriptWorkPauseReason> {
    let pos = state.entities.pos(slot);
    let eye = [
        pos[0],
        pos[1] + state.entities.body(slot).height * 0.85,
        pos[2],
    ];
    let target = [
        f64::from(point.x) + 0.5,
        f64::from(point.y) + 0.5,
        f64::from(point.z) + 0.5,
    ];
    if distance(pos, centre(point)) <= 16.0 && clear_segment(state, eye, target, Some(block(point)))
    {
        return Ok(());
    }
    let candidates = [
        ScriptBlockPosition::new(point.x + 1, point.y, point.z),
        ScriptBlockPosition::new(point.x - 1, point.y, point.z),
        ScriptBlockPosition::new(point.x, point.y, point.z + 1),
        ScriptBlockPosition::new(point.x, point.y, point.z - 1),
    ];
    for candidate in candidates {
        if navigate(state, slot, candidate) {
            return Err(ScriptWorkPauseReason::BlockedRoute);
        }
    }
    Err(ScriptWorkPauseReason::BlockedRoute)
}
fn endpoint_position(
    state: &TickState,
    id: &str,
    endpoint: &ScriptInventoryEndpoint,
) -> Result<ScriptBlockPosition, ScriptWorkPauseReason> {
    match endpoint {
        ScriptInventoryEndpoint::Container {
            dimension,
            position,
        } => {
            if dimension != state.dimension.name() {
                return Err(ScriptWorkPauseReason::Unloaded);
            }
            Ok(*position)
        }
        ScriptInventoryEndpoint::NpcCarry { handle }
        | ScriptInventoryEndpoint::NpcEquipment { handle } => {
            let slot = owned_slot(state, id, handle).map_err(reason)?;
            let p = state.entities.pos(slot);
            Ok(ScriptBlockPosition::new(
                p[0].floor() as i32,
                p[1].floor() as i32,
                p[2].floor() as i32,
            ))
        }
        ScriptInventoryEndpoint::PlayerInventory { player_id } => {
            let entity = state
                .connections
                .entity_of(crate::boundary::ConnectionId(*player_id))
                .ok_or(ScriptWorkPauseReason::Unloaded)?;
            let slot = state
                .entities
                .slot_of(entity)
                .ok_or(ScriptWorkPauseReason::Unloaded)?;
            let p = state.entities.pos(slot);
            Ok(ScriptBlockPosition::new(
                p[0].floor() as i32,
                p[1].floor() as i32,
                p[2].floor() as i32,
            ))
        }
        _ => Err(ScriptWorkPauseReason::Unsupported),
    }
}
fn haul(
    state: &mut TickState,
    id: &str,
    record: &mut NpcRecord,
    slot: EntitySlot,
    source: &ScriptInventoryEndpoint,
    destination: &ScriptInventoryEndpoint,
    filter: Option<&str>,
) -> Result<Vec<ScriptItemChange>, ScriptWorkPauseReason> {
    let carry_endpoint = carry(&record.handle);
    let two_legs = source != &carry_endpoint && destination != &carry_endpoint;
    let loaded = two_legs && record.work_cursor == 1;
    let (from, to) = if loaded {
        (&carry_endpoint, destination)
    } else if two_legs {
        (source, &carry_endpoint)
    } else {
        (source, destination)
    };
    near(state, slot, endpoint_position(state, id, from)?)?;
    near(state, slot, endpoint_position(state, id, to)?)?;
    let mut from_image = inventories::load(state, id, from).map_err(reason)?;
    let mut to_image = inventories::load(state, id, to).map_err(reason)?;
    let source_slot = if loaded {
        record
            .haul_slot
            .ok_or(ScriptWorkPauseReason::MissingInput)?
    } else {
        from_image
            .slots
            .iter()
            .find(|(_, stack)| {
                !stack.is_empty()
                    && filter.is_none_or(|name| resource(state, stack).as_deref() == Some(name))
            })
            .map(|(&slot, _)| slot)
            .ok_or(ScriptWorkPauseReason::MissingInput)?
    };
    let original = from_image
        .slots
        .get(&source_slot)
        .filter(|stack| !stack.is_empty())
        .ok_or(ScriptWorkPauseReason::MissingInput)?;
    let mut exact = original.clone();
    exact.count = 1;
    if loaded && record.haul_hash.as_ref() != Some(&inventories::hash(&exact)) {
        return Err(ScriptWorkPauseReason::MissingInput);
    }
    let name = resource(state, &exact).ok_or(ScriptWorkPauseReason::Unsupported)?;
    let before = to_image.slots.clone();
    inventories::insert(state, &mut to_image, exact.clone()).map_err(reason)?;
    let target_slot = to_image
        .slots
        .iter()
        .find(|(slot, stack)| stack.count > before.get(slot).map_or(0, |old| old.count))
        .map(|(&slot, _)| slot)
        .ok_or(ScriptWorkPauseReason::NoStorage)?;
    let source_stack = from_image
        .slots
        .get_mut(&source_slot)
        .expect("native source slot");
    source_stack.count -= 1;
    if source_stack.is_empty() {
        *source_stack = ItemStack::EMPTY;
    }
    inventories::commit(
        state,
        id,
        BTreeMap::from([(from.clone(), from_image), (to.clone(), to_image)]),
    )
    .map_err(reason)?;
    if two_legs && !loaded {
        record.work_cursor = 1;
        record.haul_slot = Some(target_slot);
        record.haul_hash = Some(inventories::hash(&exact));
    } else {
        record.work_cursor = 0;
        record.haul_slot = None;
        record.haul_hash = None;
    }
    Ok(vec![ScriptItemChange::new(
        name,
        if to == &carry_endpoint { 1 } else { -1 },
    )])
}
fn point_at(area: &ScriptWorkArea, index: u32) -> ScriptBlockPosition {
    let width = (area.max.x - area.min.x + 1) as u32;
    let depth = (area.max.z - area.min.z + 1) as u32;
    ScriptBlockPosition::new(
        area.min.x + (index % width) as i32,
        area.min.y + (index / (width * depth)) as i32,
        area.min.z + ((index / width) % depth) as i32,
    )
}
fn wear(
    state: &TickState,
    image: &mut InventoryImage,
    slot: u8,
    amount: u32,
) -> Option<ScriptItemChange> {
    let stack = image.slots.get_mut(&slot)?;
    let name = resource(state, stack)?;
    let maximum = maximum_damage(state, stack, &name)?;
    let damage = (stack.damage.unwrap_or(0).max(0) as u32).saturating_add(amount);
    if damage >= maximum {
        *stack = ItemStack::EMPTY;
        Some(ScriptItemChange::new(name, -1))
    } else {
        stack.damage = Some(damage as i32);
        None
    }
}

fn maximum_damage(state: &TickState, stack: &ItemStack, name: &str) -> Option<u32> {
    stack
        .extended()
        .max_damage
        .or_else(|| {
            state
                .registry
                .item_facts
                .get(&Identifier::parse(name).ok()?)
                .and_then(|facts| facts.max_damage)
        })
        .or_else(|| {
            mc_data::item_semantics_26_1_2::max_tool_damage_for_path(
                name.strip_prefix("minecraft:").unwrap_or(name),
            )
            .map(|value| value as u32)
        })
        .filter(|maximum| *maximum > 0 && *maximum <= i32::MAX as u32)
}
fn write_block(state: &mut TickState, point: ScriptBlockPosition, value: u32) {
    state.world.set_block(point.x, point.y, point.z, value);
    state.plugin_block_writes.push((block(point), value));
}

fn physical_block_work(
    state: &mut TickState,
    id: &str,
    record: &mut NpcRecord,
    slot: EntitySlot,
    work: &ScriptNpcWorkOrder,
    work_revision: u64,
) -> Result<Option<Vec<ScriptItemChange>>, ScriptWorkPauseReason> {
    use crate::plugin::NpcBlockBuild;
    let entity_uuid = state.entities.row(slot).uuid;
    let polled = crate::plugin::poll_npc_block_build(
        state,
        id,
        &record.handle,
        work_revision,
        entity_uuid,
        work,
    )
    .map_err(reason)?;
    match polled {
        NpcBlockBuild::Pending => Ok(None),
        NpcBlockBuild::Ready { plan, approval } => {
            commit_block_work(state, id, record, slot, work, plan, Some(approval)).map(Some)
        }
        NpcBlockBuild::Direct | NpcBlockBuild::Prepare => {
            let plan = match work {
                ScriptNpcWorkOrder::Replant { .. } => {
                    prepare_replant(state, id, record, slot, work)
                }
                _ => prepare_break_block(state, id, record, slot, work),
            }?;
            if matches!(polled, NpcBlockBuild::Direct) {
                commit_block_work(state, id, record, slot, work, plan, None).map(Some)
            } else {
                crate::plugin::begin_npc_block_build(
                    state,
                    id,
                    &record.handle,
                    work_revision,
                    entity_uuid,
                    work,
                    plan,
                )
                .map_err(reason)?;
                Ok(None)
            }
        }
    }
}

fn commit_block_work(
    state: &mut TickState,
    id: &str,
    record: &mut NpcRecord,
    slot: EntitySlot,
    work: &ScriptNpcWorkOrder,
    plan: crate::plugin::PreparedNpcBlockWork,
    approval: Option<mc_script::precommit::Approval>,
) -> Result<Vec<ScriptItemChange>, ScriptWorkPauseReason> {
    let point = ScriptBlockPosition::new(plan.edit.x, plan.edit.y, plan.edit.z);
    if let Some(mut approval) = approval {
        if record.work_cursor != plan.cursor
            || state.world.block_at(point.x, point.y, point.z) != Some(plan.edit.previous_state)
            || state
                .block_entities
                .contains_key(&(point.x, point.y, point.z))
        {
            return Err(ScriptWorkPauseReason::BlockedRoute);
        }
        near(state, slot, point)?;
        if matches!(work, ScriptNpcWorkOrder::Replant { .. })
            && !crate::farming::can_stand(state, plan.edit.proposed_state, block(point))
        {
            return Err(ScriptWorkPauseReason::MissingInput);
        }
        for (endpoint, image) in &plan.images {
            if inventories::load(state, id, endpoint)
                .map_err(reason)?
                .fence
                != image.fence
            {
                return Err(ScriptWorkPauseReason::BlockedRoute);
            }
        }
        crate::build_precommit::consume_approval(state, &mut approval)
            .map_err(|failure| reason(crate::plugin::hook_operation_failure(failure)))?;
    }
    inventories::commit(state, id, plan.images).map_err(reason)?;
    write_block(state, point, plan.edit.proposed_state);
    record.work_cursor = plan.next_cursor;
    Ok(plan.changes)
}
fn prepare_break_block(
    state: &mut TickState,
    id: &str,
    record: &NpcRecord,
    slot: EntitySlot,
    work: &ScriptNpcWorkOrder,
) -> Result<crate::plugin::PreparedNpcBlockWork, ScriptWorkPauseReason> {
    let (area, tool) = match work {
        ScriptNpcWorkOrder::Harvest { area, tool }
        | ScriptNpcWorkOrder::CutTree { area, tool }
        | ScriptNpcWorkOrder::Mine { area, tool } => (area, tool.as_str()),
        _ => return Err(ScriptWorkPauseReason::Unsupported),
    };
    let (tool_endpoint, mut tool_image, tool_slot) = tools(state, id, &record.handle, tool)?;
    let mut selected = None;
    for index in record.work_cursor..area.cell_count() {
        let point = point_at(area, index);
        let native = state
            .world
            .block_at(point.x, point.y, point.z)
            .ok_or(ScriptWorkPauseReason::Unloaded)?;
        let value = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(native))
            .ok_or(ScriptWorkPauseReason::Unsupported)?;
        let name = value.block.id.path();
        if native == state.registry.states.air
            || mc_data::block_mining::block_break_is_denied(name)
            || state
                .registry
                .mining
                .destroy_speed(native)
                .is_some_and(|speed| speed < 0.0)
        {
            continue;
        }
        let eligible = match work {
            ScriptNpcWorkOrder::Harvest { .. } => {
                value
                    .properties
                    .iter()
                    .find(|(name, _)| name == "age")
                    .is_some_and(|(_, age)| {
                        value
                            .block
                            .properties
                            .iter()
                            .find(|(name, _)| name == "age")
                            .and_then(|(_, values)| {
                                values
                                    .iter()
                                    .filter_map(|value| value.parse::<u32>().ok())
                                    .max()
                            })
                            .is_some_and(|max| age.parse::<u32>() == Ok(max))
                    })
                    && matches!(
                        name,
                        "wheat"
                            | "carrots"
                            | "potatoes"
                            | "beetroots"
                            | "nether_wart"
                            | "cocoa"
                            | "sweet_berry_bush"
                    )
            }
            ScriptNpcWorkOrder::CutTree { .. } => {
                state.registry.tags.contains_raw_id(
                    "minecraft:block",
                    "minecraft:logs",
                    value.block.raw_id,
                ) || name.ends_with("_log")
                    || name.ends_with("_stem")
            }
            ScriptNpcWorkOrder::Mine { .. } => true,
            _ => false,
        };
        if eligible {
            selected = Some((index, point, native));
            break;
        }
    }
    let (index, point, native) = selected.ok_or(ScriptWorkPauseReason::MissingInput)?;
    if state
        .block_entities
        .contains_key(&(point.x, point.y, point.z))
    {
        return Err(ScriptWorkPauseReason::Protected);
    }
    near(state, slot, point)?;
    let tool_stack = tool_image.slots.get(&tool_slot).expect("physical tool");
    let drops = crate::block_drop_context::roll(
        state,
        block(point),
        native,
        tool_stack,
        state.tick ^ u64::from(index),
    )
    .map_err(|_| ScriptWorkPauseReason::Unsupported)?;
    let carry_endpoint = carry(&record.handle);
    let mut carry_image = if tool_endpoint == carry_endpoint {
        tool_image.clone()
    } else {
        inventories::load(state, id, &carry_endpoint).map_err(reason)?
    };
    let mut changes = Vec::new();
    for drop in drops {
        if let Some(name) = resource(state, &drop) {
            changes.push(ScriptItemChange::new(name, i64::from(drop.count)));
        }
        inventories::insert(state, &mut carry_image, drop).map_err(reason)?;
    }
    if tool_endpoint == carry_endpoint {
        tool_image = carry_image.clone();
    }
    if let Some(change) = wear(state, &mut tool_image, tool_slot, 1) {
        changes.push(change);
    }
    let mut images = BTreeMap::from([(carry_endpoint, carry_image)]);
    images.insert(tool_endpoint, tool_image);
    Ok(crate::plugin::PreparedNpcBlockWork {
        edit: mc_script::precommit::BuildEdit::new(
            point.x,
            point.y,
            point.z,
            native,
            state.registry.states.air,
        ),
        images,
        changes,
        cursor: record.work_cursor,
        next_cursor: index + 1,
    })
}
fn prepare_replant(
    state: &mut TickState,
    id: &str,
    record: &NpcRecord,
    slot: EntitySlot,
    work: &ScriptNpcWorkOrder,
) -> Result<crate::plugin::PreparedNpcBlockWork, ScriptWorkPauseReason> {
    let ScriptNpcWorkOrder::Replant { area, seed, tool } = work else {
        return Err(ScriptWorkPauseReason::Unsupported);
    };
    let seed_id = Identifier::parse(seed).map_err(|_| ScriptWorkPauseReason::Unsupported)?;
    let placed = crate::farming::placed_block(&state.registry.blocks, &seed_id)
        .map(|block| block.default.0)
        .ok_or(ScriptWorkPauseReason::Unsupported)?;
    let mut selected = None;
    for index in record.work_cursor..area.cell_count() {
        let point = point_at(area, index);
        if state
            .world
            .block_at(point.x, point.y, point.z)
            .ok_or(ScriptWorkPauseReason::Unloaded)?
            == state.registry.states.air
            && crate::farming::can_stand(state, placed, block(point))
        {
            selected = Some((index, point));
            break;
        }
    }
    let (index, point) = selected.ok_or(ScriptWorkPauseReason::MissingInput)?;
    near(state, slot, point)?;
    let carry_endpoint = carry(&record.handle);
    let mut carry_image = inventories::plan_debit(
        state,
        id,
        &carry_endpoint,
        &BTreeMap::from([(seed.to_owned(), 1)]),
    )
    .map_err(reason)?;
    let (tool_endpoint, mut tool_image, tool_slot) = tools(state, id, &record.handle, tool)?;
    if tool_endpoint == carry_endpoint {
        tool_image = carry_image.clone();
    }
    let mut changes = vec![ScriptItemChange::new(seed.to_owned(), -1)];
    if let Some(change) = wear(state, &mut tool_image, tool_slot, 1) {
        changes.push(change);
    }
    if tool_endpoint == carry_endpoint {
        carry_image = tool_image.clone();
    }
    let mut images = BTreeMap::from([(carry_endpoint, carry_image)]);
    images.insert(tool_endpoint, tool_image);
    Ok(crate::plugin::PreparedNpcBlockWork {
        edit: mc_script::precommit::BuildEdit::new(
            point.x,
            point.y,
            point.z,
            state.registry.states.air,
            placed,
        ),
        images,
        changes,
        cursor: record.work_cursor,
        next_cursor: index + 1,
    })
}
fn tend(
    state: &mut TickState,
    id: &str,
    record: &mut NpcRecord,
    slot: EntitySlot,
    area: &ScriptWorkArea,
    feed: &str,
) -> Result<Vec<ScriptItemChange>, ScriptWorkPauseReason> {
    let feed_id = state
        .registry
        .items
        .id_of(&Identifier::parse(feed).map_err(|_| ScriptWorkPauseReason::Unsupported)?)
        .ok_or(ScriptWorkPauseReason::MissingInput)?;
    let target = state.entities.slots().find(|target| {
        let body = state.entities.body(*target);
        let p = state.entities.pos(*target);
        if !crate::entity_life::can_feed(body, state.entities.row(*target))
            || p[0] < f64::from(area.min.x) || p[0] > f64::from(area.max.x + 1)
            || p[1] < f64::from(area.min.y) || p[1] > f64::from(area.max.y + 1)
            || p[2] < f64::from(area.min.z) || p[2] > f64::from(area.max.z + 1) { return false; }
        matches!(body.kind, crate::entities::EntityKind::Mob { type_id }
            if state.registry.blueprint(type_id).is_some_and(|blueprint| blueprint.breed_food.contains(&feed_id)))
    }).ok_or(ScriptWorkPauseReason::MissingInput)?;
    let p = state.entities.pos(target);
    near(
        state,
        slot,
        ScriptBlockPosition::new(
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        ),
    )?;
    let endpoint = carry(&record.handle);
    let image = inventories::plan_debit(
        state,
        id,
        &endpoint,
        &BTreeMap::from([(feed.to_owned(), 1)]),
    )
    .map_err(reason)?;
    inventories::commit(state, id, BTreeMap::from([(endpoint, image)])).map_err(reason)?;
    crate::entity_life::feed(state, state.entities.body(target).id);
    state
        .appearance_changed
        .push(state.entities.body(target).id);
    Ok(vec![ScriptItemChange::new(feed.to_owned(), -1)])
}
