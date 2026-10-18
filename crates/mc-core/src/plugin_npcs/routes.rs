//! Exclusive route leases advance only from native collision-resolved poses.
use super::*;
use crate::plugin_inventories as inventories;

const ROUTES: &str = "npc/routes";
#[derive(Clone, Serialize, Deserialize)]
struct Route {
    admission: ScriptNpcRouteAdmission,
    status: ScriptNpcRouteStatus,
    geometry: BTreeMap<String, u32>,
    cargo_operation_id: Option<String>,
}
fn all(state: &TickState, id: &str) -> Result<BTreeMap<String, Route>, ScriptOperationFailure> {
    read_domain(state, id, ROUTES)
}
fn active(route: &Route) -> bool {
    !matches!(
        route.status.state,
        ScriptNpcRouteState::Cancelled | ScriptNpcRouteState::Dead | ScriptNpcRouteState::Exhausted
    )
}
pub(super) fn has_lease(state: &TickState, id: &str, handle: &str) -> bool {
    all(state, id).is_ok_and(|routes| {
        routes
            .values()
            .any(|route| route.status.handle == handle && active(route))
    })
}

pub(super) fn validate_permit(
    state: &TickState,
    id: &str,
    handle: &str,
    permit: Option<&ScriptNpcExecutionPermit>,
) -> Result<(), ScriptOperationFailure> {
    let routes = all(state, id)?;
    let lease = routes
        .values()
        .find(|route| route.status.handle == handle && active(route));
    match (lease, permit) {
        (None, None) => Ok(()),
        (Some(route), Some(permit))
            if permit.route_ref == route.status.route_ref
                && permit.executor_epoch == route.status.executor_epoch =>
        {
            if matches!(
                route.status.state,
                ScriptNpcRouteState::Prepared
                    | ScriptNpcRouteState::Running
                    | ScriptNpcRouteState::Arrived
            ) {
                Ok(())
            } else {
                Err(ScriptOperationFailure::Busy)
            }
        }
        (Some(_), None) => Err(ScriptOperationFailure::Busy),
        _ => Err(ScriptOperationFailure::StaleRevision),
    }
}
fn outcome(status: ScriptNpcRouteStatus) -> ScriptOperationOutcome {
    committed(
        status.revision,
        ScriptOperationPayload::NpcRoute {
            result: Box::new(ScriptNpcRouteResult::Status {
                route: Box::new(status),
            }),
        },
    )
}
fn inventory(
    state: &TickState,
    id: &str,
    handle: &str,
) -> Result<(u64, String), ScriptOperationFailure> {
    let carry = inventories::load(
        state,
        id,
        &ScriptInventoryEndpoint::NpcCarry {
            handle: handle.to_owned(),
        },
    )?;
    let equipment = inventories::load(
        state,
        id,
        &ScriptInventoryEndpoint::NpcEquipment {
            handle: handle.to_owned(),
        },
    )?;
    Ok((
        carry.fence.revision.max(equipment.fence.revision),
        inventories::hash(&(carry.slots, equipment.slots)),
    ))
}
fn ticket(state: &mut TickState, id: &str, route: &Route) {
    let key = format!("npc/{id}/{}", route.status.route_ref);
    if route.status.mode == ScriptNpcExecutorMode::RemotePhysical && active(route) {
        if let Some(pose) = &route.status.pose {
            state.tickets.insert(
                key,
                (
                    crate::boundary::ChunkPos {
                        x: (pose.x.floor() as i32).div_euclid(16),
                        z: (pose.z.floor() as i32).div_euclid(16),
                    },
                    1,
                ),
            );
        }
    } else {
        state.tickets.remove(&key);
    }
}

pub(super) fn execute(
    state: &mut TickState,
    id: &str,
    operation: &ScriptNpcRouteOperation,
) -> Result<ScriptOperationOutcome, ScriptOperationFailure> {
    let mut routes = all(state, id)?;
    let revision = next_revision(state, id);
    match operation {
        ScriptNpcRouteOperation::Admit {
            operation_id,
            admission,
        } => {
            let records = records(state, id)?;
            let record = records
                .get(&admission.handle)
                .ok_or(ScriptOperationFailure::Forbidden)?;
            if record.revision != admission.identity_revision
                || record.controls.order_revision != admission.order_revision
            {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            if record.dimension != admission.dimension
                || admission.dimension != state.dimension.name()
            {
                return Err(ScriptOperationFailure::Unloaded);
            }
            if routes
                .values()
                .any(|route| route.status.handle == admission.handle && active(route))
            {
                return Err(ScriptOperationFailure::Busy);
            }
            inventories::validate_route_reservations(state, id, &admission.reservation_refs)?;
            let slot = owned_slot(state, id, &admission.handle)?;
            let geometry = geometry(state, slot, &admission.waypoints)?;
            let route_ref = inventories::hash(&(id, operation_id, &admission));
            let status = ScriptNpcRouteStatus::new(
                route_ref.clone(),
                admission.handle.clone(),
                record.entity_uuid.to_string(),
                Some(state.entities.body(slot).id.0),
                record.revision,
                record.controls.order_revision,
                revision,
                1,
                ScriptNpcExecutorMode::ObservedPhysical,
                ScriptNpcRouteState::Prepared,
                ScriptNpcLifecycle::AliveLoaded,
                Some(loaded(state, slot)),
                0,
                0,
                None,
                None,
                0,
                geometry.len() as u32,
                inventories::hash(&geometry),
                admission.max_ticks,
                None,
                None,
                None,
                None,
            );
            routes.insert(
                route_ref,
                Route {
                    admission: admission.clone(),
                    status: status.clone(),
                    geometry,
                    cargo_operation_id: None,
                },
            );
            save(state, id, ROUTES, &routes);
            Ok(outcome(status))
        }
        ScriptNpcRouteOperation::Query { route_ref } => {
            let route = routes
                .get_mut(route_ref)
                .ok_or(ScriptOperationFailure::NotFound)?;
            refresh_pose(state, route);
            Ok(outcome(route.status.clone()))
        }
        ScriptNpcRouteOperation::Start {
            route_ref,
            expected_epoch,
            cargo_operation_id,
            ..
        } => {
            let route = routes
                .get_mut(route_ref)
                .ok_or(ScriptOperationFailure::NotFound)?;
            if route.status.executor_epoch != *expected_epoch {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            if route.status.state != ScriptNpcRouteState::Prepared {
                return Err(ScriptOperationFailure::Busy);
            }
            if !route.admission.reservation_refs.is_empty() && cargo_operation_id.is_none() {
                return Err(ScriptOperationFailure::InvalidRequest);
            }
            if let Some(cargo_id) = cargo_operation_id {
                let receipt = state
                    .plugins
                    .operation_receipt(id, cargo_id)
                    .ok_or(ScriptOperationFailure::NotFound)?;
                if receipt.failure().is_some() {
                    return Err(ScriptOperationFailure::Forbidden);
                }
                let ScriptOperationPayload::OwnedInventory { result } = receipt.payload() else {
                    return Err(ScriptOperationFailure::Forbidden);
                };
                let endpoint = ScriptInventoryEndpoint::NpcCarry {
                    handle: route.status.handle.clone(),
                };
                let fence = match &**result {
                    ScriptOwnedInventoryResult::Transfer { inventories }
                        if route.admission.reservation_refs.is_empty() =>
                    {
                        inventories
                            .iter()
                            .find(|image| image.endpoint == endpoint)
                            .map(|image| &image.fence)
                    }
                    ScriptOwnedInventoryResult::ReservedTransfer {
                        reservation,
                        inventories,
                    } if route.admission.reservation_refs.is_empty()
                        || route
                            .admission
                            .reservation_refs
                            .contains(&reservation.reservation_ref) =>
                    {
                        inventories
                            .iter()
                            .find(|image| image.endpoint == endpoint)
                            .map(|image| &image.fence)
                    }
                    _ => None,
                }
                .ok_or(ScriptOperationFailure::Forbidden)?;
                if inventories::load(state, id, &endpoint)?.fence != *fence {
                    return Err(ScriptOperationFailure::StaleRevision);
                }
            }
            inventories::validate_route_reservations(state, id, &route.admission.reservation_refs)?;
            let slot = owned_slot(state, id, &route.status.handle)?;
            if !geometry_current(state, &route.geometry) {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            route.cargo_operation_id = cargo_operation_id.clone();
            route.status.state = ScriptNpcRouteState::Running;
            route.status.revision = revision;
            route.status.due_tick = Some(state.tick + 1);
            navigate(state, slot, route.admission.waypoints[0]);
            let status = route.status.clone();
            save(state, id, ROUTES, &routes);
            Ok(outcome(status))
        }
        ScriptNpcRouteOperation::Handoff {
            route_ref,
            expected_epoch,
            mode,
            ..
        } => {
            let route = routes
                .get_mut(route_ref)
                .ok_or(ScriptOperationFailure::NotFound)?;
            if route.status.executor_epoch != *expected_epoch {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            if !active(route)
                || matches!(
                    route.status.state,
                    ScriptNpcRouteState::Prepared | ScriptNpcRouteState::Arrived
                )
            {
                return Err(ScriptOperationFailure::Busy);
            }
            let slot = owned_slot(state, id, &route.status.handle)?;
            let pose = loaded(state, slot);
            let (inventory_revision, inventory_hash) = inventory(state, id, &route.status.handle)?;
            let source_epoch = route.status.executor_epoch;
            route.status.executor_epoch += 1;
            route.status.handoff = Some(ScriptNpcRouteHandoff::new(
                inventories::hash(&(route_ref, source_epoch, revision)),
                source_epoch,
                route.status.executor_epoch,
                state.tick,
                state.dimension.name().to_owned(),
                pose.clone(),
                inventory_revision,
                inventory_hash,
                route.status.geometry_hash.clone(),
                route.admission.reservation_refs.clone(),
            ));
            route.status.pose = Some(pose);
            route.status.mode = *mode;
            route.status.state = if *mode == ScriptNpcExecutorMode::Parked {
                ScriptNpcRouteState::Parked
            } else {
                ScriptNpcRouteState::Running
            };
            route.status.blocked_reason = None;
            route.status.revision = revision;
            route.status.due_tick =
                (*mode != ScriptNpcExecutorMode::Parked).then_some(state.tick + 1);
            if *mode == ScriptNpcExecutorMode::Parked {
                orders::clear_navigation(state, slot);
            }
            ticket(state, id, route);
            let status = route.status.clone();
            save(state, id, ROUTES, &routes);
            Ok(outcome(status))
        }
        ScriptNpcRouteOperation::Cancel {
            route_ref,
            expected_epoch,
            ..
        } => {
            let route = routes
                .get_mut(route_ref)
                .ok_or(ScriptOperationFailure::NotFound)?;
            if route.status.executor_epoch != *expected_epoch {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            if !active(route) {
                return Err(ScriptOperationFailure::Busy);
            }
            if let Ok(slot) = owned_slot(state, id, &route.status.handle) {
                orders::clear_navigation(state, slot);
            }
            route.status.executor_epoch += 1;
            route.status.state = ScriptNpcRouteState::Cancelled;
            route.status.blocked_reason = None;
            route.status.revision = revision;
            route.status.due_tick = None;
            route.status.outcome_tick = Some(state.tick);
            ticket(state, id, route);
            let status = route.status.clone();
            save(state, id, ROUTES, &routes);
            Ok(outcome(status))
        }
        _ => Err(ScriptOperationFailure::InvalidRequest),
    }
}
fn refresh_pose(state: &TickState, route: &mut Route) {
    let uuid = uuid::Uuid::parse_str(&route.status.entity_uuid).expect("native route UUID");
    let slot = state.entities.slot_of_uuid(uuid).filter(|slot| {
        route.admission.dimension == state.dimension.name()
            && route.status.state != ScriptNpcRouteState::Dead
            && physical_slot_loaded(state, *slot)
    });
    route.status.entity_id = slot.map(|slot| state.entities.body(slot).id.0);
    route.status.pose = slot.map(|slot| loaded(state, slot));
    route.status.lifecycle = if route.status.state == ScriptNpcRouteState::Dead {
        ScriptNpcLifecycle::Dead
    } else if slot.is_some() {
        ScriptNpcLifecycle::AliveLoaded
    } else {
        ScriptNpcLifecycle::AliveUnloaded
    };
}
fn geometry_current(state: &TickState, image: &BTreeMap<String, u32>) -> bool {
    image.iter().all(|(key, expected)| {
        let mut parts = key.split(',').filter_map(|part| part.parse::<i32>().ok());
        let Some(x) = parts.next() else {
            return false;
        };
        let Some(y) = parts.next() else {
            return false;
        };
        let Some(z) = parts.next() else {
            return false;
        };
        state.world.block_at(x, y, z) == Some(*expected)
    })
}
fn geometry(
    state: &mut TickState,
    slot: EntitySlot,
    waypoints: &[ScriptBlockPosition],
) -> Result<BTreeMap<String, u32>, ScriptOperationFailure> {
    let row = state.entities.body(slot);
    let (width, height) = (row.half_width, row.height);
    let mut start = state.entities.pos(slot);
    let mut image = BTreeMap::new();
    for point in waypoints {
        let mut cache = crate::settlements::allocator::PoiCache::default();
        let mut path = Vec::new();
        if !crate::settlements::allocator::find_path_exact(
            state,
            &mut cache,
            start,
            block(*point),
            width,
            height,
            &mut path,
        ) {
            return Err(ScriptOperationFailure::Blocked);
        }
        if crate::settlements::allocator::standing(
            &state.world,
            &state.registry,
            block(*point),
            width,
            height,
        )
        .is_none()
        {
            return Err(ScriptOperationFailure::Blocked);
        }
        if path.is_empty() {
            path.push(block(*point));
        }
        for point in path {
            for x in point.x - 1..=point.x + 1 {
                for y in point.y - 1..=point.y + 3 {
                    for z in point.z - 1..=point.z + 1 {
                        let value = state
                            .world
                            .block_at(x, y, z)
                            .ok_or(ScriptOperationFailure::Unloaded)?;
                        image.insert(format!("{x},{y},{z}"), value);
                    }
                }
            }
        }
        start = centre(*point);
    }
    Ok(image)
}
fn block_route(route: &mut Route, reason: ScriptNpcRouteBlockReason) {
    route.status.state = ScriptNpcRouteState::Blocked;
    route.status.blocked_reason = Some(reason);
    route.status.due_tick = None;
}

fn finish_advance(state: &mut TickState, id: &str, route: &mut Route, revision: u64) {
    route.status.revision = revision;
    if route.status.state != ScriptNpcRouteState::Running
        && let Ok(uuid) = uuid::Uuid::parse_str(&route.status.entity_uuid)
        && let Some(slot) = state.entities.slot_of_uuid(uuid)
    {
        orders::clear_navigation(state, slot);
    }
    ticket(state, id, route);
}

pub(super) fn advance(state: &mut TickState, id: &str, records: &BTreeMap<String, NpcRecord>) {
    let Ok(mut routes) = all(state, id) else {
        crate::plugin::fail_native(state);
        return;
    };
    let revision = next_revision(state, id);
    let mut changed = false;
    for route in routes.values_mut() {
        if route.admission.dimension != state.dimension.name()
            || !matches!(
                route.status.state,
                ScriptNpcRouteState::Running | ScriptNpcRouteState::Blocked
            )
        {
            continue;
        }
        refresh_pose(state, route);
        let Some(record) = records.get(&route.status.handle) else {
            block_route(route, ScriptNpcRouteBlockReason::StaleIdentity);
            finish_advance(state, id, route, revision);
            changed = true;
            continue;
        };
        if record.disposition == ScriptNpcLifecycle::Dead {
            route.status.state = ScriptNpcRouteState::Dead;
            route.status.blocked_reason = None;
            route.status.due_tick = None;
            route.status.outcome_tick = Some(state.tick);
            route.status.lifecycle = ScriptNpcLifecycle::Dead;
            route.status.pose = None;
            finish_advance(state, id, route, revision);
            changed = true;
            continue;
        }
        if record.revision != route.status.identity_revision {
            block_route(route, ScriptNpcRouteBlockReason::StaleIdentity);
            finish_advance(state, id, route, revision);
            changed = true;
            continue;
        }
        if record.controls.order_revision != route.status.order_revision {
            block_route(route, ScriptNpcRouteBlockReason::StaleOrder);
            finish_advance(state, id, route, revision);
            changed = true;
            continue;
        }
        let Ok(slot) = owned_slot(state, id, &route.status.handle) else {
            block_route(route, ScriptNpcRouteBlockReason::Unloaded);
            finish_advance(state, id, route, revision);
            changed = true;
            continue;
        };
        if !geometry_current(state, &route.geometry) {
            block_route(route, ScriptNpcRouteBlockReason::StaleGeometry);
            orders::clear_navigation(state, slot);
            finish_advance(state, id, route, revision);
            changed = true;
            continue;
        }
        if route.status.native_ticks == route.status.max_ticks {
            route.status.state = ScriptNpcRouteState::Exhausted;
            route.status.blocked_reason = None;
            route.status.due_tick = None;
            route.status.outcome_tick = Some(state.tick);
            orders::clear_navigation(state, slot);
            finish_advance(state, id, route, revision);
            changed = true;
            continue;
        }
        let point = route.admission.waypoints[usize::from(route.status.cursor)];
        let row = state.entities.body(slot);
        let actual_stance = crate::settlements::allocator::standing(
            &state.world,
            &state.registry,
            block(point),
            row.half_width,
            row.height,
        );
        if actual_stance.is_some_and(|stance| {
            distance(state.entities.pos(slot), stance) < 0.0001 && state.entities.ground(slot)
        }) {
            route.status.cursor += 1;
            route.status.segment = route.status.cursor;
            if usize::from(route.status.cursor) == route.admission.waypoints.len() {
                match inventory(state, id, &route.status.handle) {
                    Ok((inventory_revision, hash)) => {
                        let pose = loaded(state, slot);
                        route.status.arrival = Some(ScriptNpcRouteArrival::new(
                            inventories::hash(&(
                                &route.status.route_ref,
                                route.status.executor_epoch,
                                state.tick,
                            )),
                            route.status.executor_epoch,
                            state.tick,
                            state.dimension.name().to_owned(),
                            pose,
                            inventory_revision,
                            hash,
                        ));
                        route.status.state = ScriptNpcRouteState::Arrived;
                        route.status.blocked_reason = None;
                        route.status.due_tick = None;
                        route.status.outcome_tick = Some(state.tick);
                        orders::clear_navigation(state, slot);
                    }
                    Err(_) => {
                        route.status.cursor -= 1;
                        route.status.segment = route.status.cursor;
                        block_route(route, ScriptNpcRouteBlockReason::ExecutorUnavailable);
                    }
                }
            }
        } else if !navigate(state, slot, point) {
            block_route(route, ScriptNpcRouteBlockReason::BlockedRoute);
        } else {
            route.status.state = ScriptNpcRouteState::Running;
            route.status.blocked_reason = None;
            route.status.due_tick = Some(state.tick + 1);
            route.status.native_ticks += 1;
        }
        finish_advance(state, id, route, revision);
        changed = true;
    }
    if changed {
        save(state, id, ROUTES, &routes);
        state
            .plugins
            .set_domain(id, "npc/revision", serde_json::Value::from(revision));
    }
}

pub(super) fn death_epoch(state: &TickState, id: &str, handle: &str) -> Option<u64> {
    all(state, id)
        .ok()?
        .values()
        .find(|route| route.status.handle == handle && active(route))
        .map(|route| route.status.executor_epoch)
}

pub(super) fn record_death(
    state: &mut TickState,
    id: &str,
    handle: &str,
    receipt: &ScriptNpcDeathCargoReceipt,
    revision: u64,
) {
    let Ok(mut routes) = all(state, id) else {
        crate::plugin::fail_native(state);
        return;
    };
    for route in routes
        .values_mut()
        .filter(|route| route.status.handle == handle && active(route))
    {
        route.status.state = ScriptNpcRouteState::Dead;
        route.status.lifecycle = ScriptNpcLifecycle::Dead;
        route.status.pose = None;
        route.status.entity_id = None;
        route.status.blocked_reason = None;
        route.status.due_tick = None;
        route.status.outcome_tick = Some(state.tick);
        route.status.revision = revision;
        route.status.death_cargo = Some(Box::new(receipt.clone()));
        ticket(state, id, route);
    }
    save(state, id, ROUTES, &routes);
}

pub(super) fn validate(state: &TickState, id: &str) -> Result<(), ScriptOperationFailure> {
    let routes = all(state, id)?;
    let records = records(state, id)?;
    let mut leases = std::collections::BTreeSet::new();
    for (reference, route) in routes {
        route
            .admission
            .validate()
            .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
        route
            .status
            .validate()
            .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
        let record = records
            .get(&route.status.handle)
            .ok_or(ScriptOperationFailure::InvalidRequest)?;
        if reference != route.status.route_ref
            || route.admission.handle != record.handle
            || route.status.entity_uuid != record.entity_uuid.to_string()
            || route.admission.dimension != record.dimension
            || route.status.max_ticks != route.admission.max_ticks
            || usize::from(route.status.cursor) > route.admission.waypoints.len()
            || route.status.geometry_hash != inventories::hash(&route.geometry)
            || route.status.geometry_cells as usize != route.geometry.len()
            || active(&route) && !leases.insert(record.entity_uuid)
        {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        for key in route.geometry.keys() {
            let coordinates = key
                .split(',')
                .map(str::parse::<i32>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            if coordinates.len() != 3 {
                return Err(ScriptOperationFailure::InvalidRequest);
            }
        }
    }
    Ok(())
}
