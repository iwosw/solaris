//! Native group orders use UUID-authenticated targets and collision navigation.
use super::*;
use crate::entities::EntityKind;
use mc_physics::mth;

#[derive(Clone, Serialize, Deserialize)]
struct Target {
    entity_uuid: uuid::Uuid,
    policy_revision: u64,
    expires_revision: u64,
    category: ScriptHostileCategory,
}

pub(super) fn execute(
    state: &mut TickState,
    id: &str,
    operation: &ScriptNpcOrderOperation,
) -> Result<ScriptOperationOutcome, ScriptOperationFailure> {
    match operation {
        ScriptNpcOrderOperation::AssignWork { .. }
        | ScriptNpcOrderOperation::CancelWork { .. }
        | ScriptNpcOrderOperation::SuspendWork { .. }
        | ScriptNpcOrderOperation::ResumeWork { .. }
        | ScriptNpcOrderOperation::AckWorkReceipts { .. } => {
            return work::execute(state, id, operation);
        }
        ScriptNpcOrderOperation::DeployItem { .. }
        | ScriptNpcOrderOperation::RetractItem { .. }
        | ScriptNpcOrderOperation::CaptureItem { .. }
        | ScriptNpcOrderOperation::UseItem { .. } => {
            return deployment::execute(state, id, operation);
        }
        _ => {}
    }
    let mut all = records(state, id)?;
    let revision = next_revision(state, id);
    match operation {
        ScriptNpcOrderOperation::Query { handles } => {
            let mut npcs = Vec::new();
            for handle in handles {
                let record = all.get(handle).ok_or(ScriptOperationFailure::Forbidden)?;
                let mut control = record.controls.clone();
                control.item_use = None;
                if let Ok(slot) = owned_slot(state, id, handle) {
                    control.item_use = item_use(state, slot);
                    if let Some(mut phase) =
                        crate::physical_attacks::npc_combat_control(state, slot)
                    {
                        phase.last_event = control
                            .combat
                            .as_ref()
                            .and_then(|control| control.last_event.clone());
                        control.combat = Some(Box::new(phase));
                    }
                }
                for observation in &mut control.targets {
                    if let Some(target) = target(state, id, &observation.target_ref)? {
                        let slot = state.entities.slot_of_uuid(target.entity_uuid);
                        observation.position = slot.map(|slot| {
                            let p = state.entities.pos(slot);
                            ScriptPosition::try_new(p[0], p[1], p[2]).expect("native pose")
                        });
                        observation.alive = slot.map(|slot| state.entities.body(slot).alive);
                    }
                }
                npcs.push(control);
            }
            npcs.sort_unstable_by(|a, b| a.handle.cmp(&b.handle));
            Ok(order_result(
                revision,
                ScriptNpcOrderResult::Controls { npcs },
            ))
        }
        ScriptNpcOrderOperation::IssueOrder {
            handles,
            expected_order_revisions,
            order,
            operation_id,
        } => {
            let mut members = Vec::with_capacity(handles.len());
            let mut slots = Vec::with_capacity(handles.len());
            for (index, (handle, expected)) in
                handles.iter().zip(expected_order_revisions).enumerate()
            {
                let record = all.get(handle);
                let status = match record {
                    None => ScriptOrderMemberState::Forbidden,
                    Some(record) if record.controls.order_revision != *expected => {
                        ScriptOrderMemberState::StaleRevision
                    }
                    Some(record) if record.disposition == ScriptNpcLifecycle::Dead => {
                        ScriptOrderMemberState::Dead
                    }
                    Some(record) if record.disposition == ScriptNpcLifecycle::Released => {
                        ScriptOrderMemberState::Forbidden
                    }
                    Some(_) if validate_permit(state, id, handle, None).is_err() => {
                        ScriptOrderMemberState::Forbidden
                    }
                    Some(record) => match owned_slot(state, id, handle) {
                        Ok(slot) => {
                            let goal =
                                order_target(state, order, index as u16, handles.len() as u16, 0)?;
                            let dimension_ok = !matches!(order,ScriptNpcOrder::Move{dimension,..} if dimension != state.dimension.name());
                            let route_ok =
                                goal.is_none_or(|point| path_possible(state, slot, point));
                            if !dimension_ok {
                                ScriptOrderMemberState::Migrating
                            } else if !route_ok {
                                ScriptOrderMemberState::BlockedRoute
                            } else {
                                slots.push((handle.clone(), slot));
                                ScriptOrderMemberState::Applied
                            }
                        }
                        Err(_) => {
                            let _ = record;
                            ScriptOrderMemberState::Unloaded
                        }
                    },
                };
                members.push(ScriptOrderMemberOutcome::new(
                    handle.clone(),
                    status,
                    (status == ScriptOrderMemberState::Applied).then_some(index as u16),
                    Vec::new(),
                ));
            }
            if let ScriptNpcOrder::Attack { targets, policy } = order {
                for reference in targets {
                    let target = target(state, id, &reference.target_ref)?
                        .ok_or(ScriptOperationFailure::Forbidden)?;
                    if reference.policy_revision != policy.revision
                        || reference.policy_revision != target.policy_revision
                        || reference.expires_revision != target.expires_revision
                        || target.expires_revision < state.tick
                        || !policy.permitted.contains(&target.category)
                    {
                        return Err(ScriptOperationFailure::StaleRevision);
                    }
                    if allied(state, policy, target.entity_uuid) {
                        return Err(ScriptOperationFailure::Forbidden);
                    }
                }
            }
            // Group admission is atomic: a failed member never moves a subset.
            if members
                .iter()
                .any(|member| member.state != ScriptOrderMemberState::Applied)
            {
                let failure = members
                    .iter()
                    .find(|member| member.state != ScriptOrderMemberState::Applied)
                    .map_or(ScriptOperationFailure::Blocked, |member| {
                        match member.state {
                            ScriptOrderMemberState::Forbidden => ScriptOperationFailure::Forbidden,
                            ScriptOrderMemberState::StaleRevision => {
                                ScriptOperationFailure::StaleRevision
                            }
                            ScriptOrderMemberState::Dead => ScriptOperationFailure::NotFound,
                            ScriptOrderMemberState::Unloaded
                            | ScriptOrderMemberState::Migrating => ScriptOperationFailure::Unloaded,
                            _ => ScriptOperationFailure::Blocked,
                        }
                    });
                for member in &mut members {
                    if member.state == ScriptOrderMemberState::Applied {
                        member.state = ScriptOrderMemberState::BlockedRoute;
                        member.formation_slot = None;
                    }
                }
                return Ok(ScriptOperationOutcome::rejected_with_payload(
                    failure,
                    ScriptOperationPayload::NpcOrder {
                        result: Box::new(ScriptNpcOrderResult::Order {
                            order_revision: revision,
                            members,
                            combat: Vec::new(),
                        }),
                    },
                ));
            }
            for ((handle, slot), member) in slots.into_iter().zip(&mut members) {
                let record = all.get_mut(&handle).expect("preflight owned member");
                record.controls.order = Some(order.clone());
                record.controls.order_revision = revision;
                record.controls.revision = revision;
                record.controls.combat =
                    matches!(order, ScriptNpcOrder::Attack { .. }).then(|| {
                        Box::new(ScriptNpcCombatControl::new(
                            operation_id.clone(),
                            state.tick,
                            state.tick,
                            None,
                            None,
                        ))
                    });
                record.formation_slot = member.formation_slot.expect("admitted formation slot");
                record.formation_size = handles.len() as u16;
                record.patrol_cursor = 0;
                let policy = if let ScriptNpcOrder::Attack { policy, .. } = order {
                    policy.clone()
                } else {
                    ScriptEngagementPolicy::new(0, Vec::new(), vec![ScriptHostileCategory::Hostile])
                };
                let radius = match order {
                    ScriptNpcOrder::Hold {
                        engagement_radius, ..
                    }
                    | ScriptNpcOrder::Patrol {
                        engagement_radius, ..
                    } => f64::from(*engagement_radius),
                    _ => f64::from(state.entities.row(slot).sight_range),
                };
                member.targets = observe_targets(state, id, slot, &policy, radius)?;
                record.controls.targets = member
                    .targets
                    .iter()
                    .map(|target| {
                        ScriptNpcTargetObservation::new(target.target_ref.clone(), None, None)
                    })
                    .collect();
                if let Some(point) = order_target(
                    state,
                    order,
                    record.formation_slot,
                    record.formation_size,
                    record.patrol_cursor,
                )? {
                    navigate(state, slot, point);
                } else {
                    clear_navigation(state, slot);
                }
            }
            save_records(state, id, &all, revision);
            Ok(order_result(
                revision,
                ScriptNpcOrderResult::Order {
                    order_revision: revision,
                    members,
                    combat: Vec::new(),
                },
            ))
        }
        ScriptNpcOrderOperation::CancelOrder {
            handles,
            expected_order_revisions,
            ..
        } => {
            for (handle, expected) in handles.iter().zip(expected_order_revisions) {
                validate_permit(state, id, handle, None)?;
                let record = all.get(handle).ok_or(ScriptOperationFailure::Forbidden)?;
                if record.controls.order_revision != *expected {
                    return Err(ScriptOperationFailure::StaleRevision);
                }
            }
            let mut members = Vec::new();
            for (index, handle) in handles.iter().enumerate() {
                let record = all.get_mut(handle).expect("preflight NPC");
                record.controls.order = None;
                record.controls.order_revision = 0;
                record.controls.revision = revision;
                record.controls.combat = None;
                record.controls.targets.clear();
                if let Some(slot) = state.entities.slot_of_uuid(record.entity_uuid) {
                    clear_navigation(state, slot);
                }
                members.push(ScriptOrderMemberOutcome::new(
                    handle.clone(),
                    ScriptOrderMemberState::Applied,
                    Some(index as u16),
                    Vec::new(),
                ));
            }
            save_records(state, id, &all, revision);
            Ok(order_result(
                revision,
                ScriptNpcOrderResult::OrderCancelled {
                    order_revision: revision,
                    members,
                },
            ))
        }
        _ => Err(ScriptOperationFailure::InvalidRequest),
    }
}

fn path_possible(state: &mut TickState, slot: EntitySlot, target: ScriptBlockPosition) -> bool {
    let row = state.entities.body(slot);
    let (width, height) = (row.half_width, row.height);
    let start = state.entities.pos(slot);
    if crate::settlements::allocator::standing(
        &state.world,
        &state.registry,
        block(target),
        width,
        height,
    )
    .is_none()
    {
        return false;
    }
    let mut cache = crate::settlements::allocator::PoiCache::default();
    let mut path = Vec::new();
    crate::settlements::allocator::find_path_exact(
        state,
        &mut cache,
        start,
        block(target),
        width,
        height,
        &mut path,
    )
}

pub(super) fn clear_navigation(state: &mut TickState, slot: EntitySlot) {
    if let Some(v) = &mut state.entities.row_mut(slot).villager {
        v.navigation_path.clear();
        v.navigation_target = None;
        v.navigation_chunks.clear();
    }
}

fn offset(
    anchor: ScriptBlockPosition,
    heading: u16,
    formation: ScriptFormation,
    index: u16,
    size: u16,
) -> ScriptBlockPosition {
    let spacing = f64::from(formation.spacing) * 0.5;
    let (x, z) = match formation.kind {
        ScriptFormationKind::Line => (
            f64::from(index) - f64::from(size.saturating_sub(1)) * 0.5,
            0.0,
        ),
        ScriptFormationKind::Column => (0.0, -f64::from(index)),
        ScriptFormationKind::Wedge => (
            if index == 0 {
                0.0
            } else if index.is_multiple_of(2) {
                f64::from(index.div_ceil(2))
            } else {
                -f64::from(index.div_ceil(2))
            },
            -f64::from(index.div_ceil(2)),
        ),
        ScriptFormationKind::Square => {
            let width = f64::from(size).sqrt().ceil() as u16;
            (
                f64::from(index % width) - f64::from(width.saturating_sub(1)) * 0.5,
                -f64::from(index / width),
            )
        }
        _ => (0.0, 0.0),
    };
    let angle = f64::from(heading).to_radians();
    ScriptBlockPosition::new(
        anchor.x
            + ((x * f64::from(mth::cos(angle)) - z * f64::from(mth::sin(angle))) * spacing).round()
                as i32,
        anchor.y,
        anchor.z
            + ((x * f64::from(mth::sin(angle)) + z * f64::from(mth::cos(angle))) * spacing).round()
                as i32,
    )
}

fn order_target(
    state: &TickState,
    order: &ScriptNpcOrder,
    index: u16,
    size: u16,
    cursor: usize,
) -> Result<Option<ScriptBlockPosition>, ScriptOperationFailure> {
    Ok(match order {
        ScriptNpcOrder::Move {
            anchor,
            heading_degrees,
            formation,
            ..
        }
        | ScriptNpcOrder::Hold {
            anchor,
            heading_degrees,
            formation,
            ..
        } => Some(offset(*anchor, *heading_degrees, *formation, index, size)),
        ScriptNpcOrder::Retreat { anchor, formation } => {
            Some(offset(*anchor, 0, *formation, index, size))
        }
        ScriptNpcOrder::Patrol {
            waypoints,
            formation,
            ..
        } => Some(offset(
            waypoints[cursor % waypoints.len()],
            0,
            *formation,
            index,
            size,
        )),
        ScriptNpcOrder::Follow {
            target_player,
            formation,
        } => {
            let id = state
                .connections
                .entity_of(crate::boundary::ConnectionId(*target_player))
                .ok_or(ScriptOperationFailure::NotFound)?;
            let slot = state
                .entities
                .slot_of(id)
                .filter(|_| state.players.contains_key(&id))
                .ok_or(ScriptOperationFailure::NotFound)?;
            let p = state.entities.pos(slot);
            Some(offset(
                ScriptBlockPosition::new(
                    p[0].floor() as i32,
                    p[1].ceil() as i32,
                    p[2].floor() as i32,
                ),
                0,
                *formation,
                index,
                size,
            ))
        }
        ScriptNpcOrder::Attack { .. } => None,
        _ => return Err(ScriptOperationFailure::InvalidRequest),
    })
}

fn target(
    state: &TickState,
    id: &str,
    reference: &str,
) -> Result<Option<Target>, ScriptOperationFailure> {
    state
        .plugins
        .get_domain(id, &format!("npc/target/{reference}"))
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)
        })
        .transpose()
}

fn category(state: &TickState, slot: EntitySlot) -> ScriptHostileCategory {
    let row = state.entities.row(slot);
    if matches!(state.entities.body(slot).kind, EntityKind::Player { .. }) {
        ScriptHostileCategory::Player
    } else if custodian(state, row.uuid).is_some() {
        ScriptHostileCategory::OwnedNpc
    } else if matches!(state.entities.body(slot).kind,EntityKind::Mob{type_id} if state.registry.blueprint(type_id).is_some_and(|b| b.has_goal(crate::blueprint::GoalKind::MeleeChase) || b.has_goal(crate::blueprint::GoalKind::RangedAttack) || b.has_goal(crate::blueprint::GoalKind::Swell)))
    {
        ScriptHostileCategory::Hostile
    } else {
        ScriptHostileCategory::NeutralAnimal
    }
}

fn observe_targets(
    state: &mut TickState,
    id: &str,
    actor: EntitySlot,
    policy: &ScriptEngagementPolicy,
    radius: f64,
) -> Result<Vec<ScriptOrderTarget>, ScriptOperationFailure> {
    let position = state.entities.pos(actor);
    let own = state.entities.row(actor).uuid;
    let mut observed = Vec::new();
    for slot in state.entities.slots() {
        let row = state.entities.row(slot);
        if !state.entities.body(slot).alive
            || row.uuid == own
            || !matches!(
                state.entities.body(slot).kind,
                EntityKind::Mob { .. } | EntityKind::Player { .. }
            )
            || allied(state, policy, row.uuid)
        {
            continue;
        }
        let category = category(state, slot);
        if !policy.permitted.contains(&category)
            || distance(position, state.entities.pos(slot)) > radius.powi(2)
        {
            continue;
        }
        let eye = [
            position[0],
            position[1] + state.entities.body(actor).height * 0.85,
            position[2],
        ];
        let target_pos = state.entities.pos(slot);
        let target_eye = [
            target_pos[0],
            target_pos[1] + state.entities.body(slot).height * 0.5,
            target_pos[2],
        ];
        if !clear_segment(state, eye, target_eye, None) {
            continue;
        }
        let reference = format!("{}:{}", row.uuid, policy.revision);
        let expires_revision = target(state, id, &reference)?
            .filter(|old| {
                old.entity_uuid == row.uuid
                    && old.policy_revision == policy.revision
                    && old.category == category
                    && old.expires_revision >= state.tick
            })
            .map_or(
                state.tick.saturating_add(200).min(MAX_SCRIPT_WORLD_TIME),
                |old| old.expires_revision,
            );
        let pos = state.entities.pos(slot);
        observed.push((
            reference,
            Target {
                entity_uuid: row.uuid,
                policy_revision: policy.revision,
                expires_revision,
                category,
            },
            ScriptBlockPosition::new(
                pos[0].floor() as i32,
                pos[1].floor() as i32,
                pos[2].floor() as i32,
            ),
        ));
        if observed.len() == MAX_ORDER_TARGETS {
            break;
        }
    }
    let mut results = Vec::new();
    for (reference, target, position) in observed {
        save(state, id, &format!("npc/target/{reference}"), &target);
        results.push(ScriptOrderTarget::new(
            reference,
            target.policy_revision,
            target.expires_revision,
            target.category,
            position,
        ));
    }
    Ok(results)
}

pub(super) fn advance(
    state: &mut TickState,
    id: &str,
    record: &mut NpcRecord,
    slot: EntitySlot,
    revision: u64,
) -> bool {
    if routes::has_lease(state, id, &record.handle) {
        return false;
    }
    let Some(order) = record.controls.order.clone() else {
        return false;
    };
    match &order {
        ScriptNpcOrder::Attack { targets, policy } => {
            let chosen = targets
                .iter()
                .filter_map(|reference| {
                    target(state, id, &reference.target_ref)
                        .ok()
                        .flatten()
                        .filter(|target| {
                            reference.policy_revision == target.policy_revision
                                && reference.expires_revision == target.expires_revision
                        })
                })
                .filter(|t| {
                    t.policy_revision == policy.revision
                        && t.expires_revision >= state.tick
                        && policy.permitted.contains(&t.category)
                        && !allied(state, policy, t.entity_uuid)
                })
                .filter_map(|target| {
                    state
                        .entities
                        .slot_of_uuid(target.entity_uuid)
                        .filter(|slot| category(state, *slot) == target.category)
                })
                .min_by(|a, b| {
                    distance(state.entities.pos(slot), state.entities.pos(*a))
                        .total_cmp(&distance(state.entities.pos(slot), state.entities.pos(*b)))
                });
            let Some(victim) = chosen else {
                clear_navigation(state, slot);
                return false;
            };
            let p = state.entities.pos(victim);
            let operation_id = record
                .controls
                .combat
                .as_ref()
                .map(|control| control.operation_id.clone())
                .unwrap_or_else(|| format!("native-order-{}", record.controls.order_revision));
            if let Some(accepted) = crate::physical_attacks::attack(
                state,
                slot,
                victim,
                &operation_id,
                record.controls.order_revision,
            ) {
                if accepted {
                    clear_navigation(state, slot);
                    record.controls.revision = revision;
                    return true;
                }
                navigate(
                    state,
                    slot,
                    ScriptBlockPosition::new(
                        p[0].floor() as i32,
                        p[1].ceil() as i32,
                        p[2].floor() as i32,
                    ),
                );
                return false;
            }
            if distance(state.entities.pos(slot), p)
                <= f64::from(state.entities.row(slot).attack_reach.max(2.0)).powi(2)
            {
                clear_navigation(state, slot);
                let from = state.entities.pos(slot);
                let from = [
                    from[0],
                    from[1] + state.entities.body(slot).height * 0.85,
                    from[2],
                ];
                let to = [p[0], p[1] + state.entities.body(victim).height * 0.5, p[2]];
                if !clear_segment(state, from, to, None) {
                    return false;
                }
                if !physical_attack_permitted(
                    state,
                    slot,
                    victim,
                    &operation_id,
                    record.controls.order_revision,
                ) {
                    return false;
                }
                let row = state.entities.row(slot);
                if state.entities.body(slot).attack_cooldown != 0 {
                    return false;
                }
                let damage = crate::entity_loot::effective_attribute(
                    &state.registry.item_rules(),
                    row,
                    "attack_damage",
                    f64::from(row.attack_damage.max(1.0)),
                ) as f32;
                let source = crate::intent::DamageSource::Melee {
                    attacker: state.entities.body(slot).id,
                    position: state.entities.pos(slot),
                    yaw: state.entities.rot(slot)[0],
                    bonus_knockback: 0.0,
                    sweep: false,
                    fire_ticks: 0,
                    bane_ticks: 0,
                };
                state.plugin_damage.push((
                    victim,
                    crate::tick::DamagePiece {
                        amount: damage,
                        kind: crate::survival::DamageKind::Melee,
                        source: Some(source),
                        physical: None,
                        custom_armor: false,
                    },
                ));
                state.entities.body_mut(slot).attack_cooldown = 20;
                state.appearance_changed.push(state.entities.body(slot).id);
                record.controls.revision = revision;
                return true;
            }
            navigate(
                state,
                slot,
                ScriptBlockPosition::new(
                    p[0].floor() as i32,
                    p[1].ceil() as i32,
                    p[2].floor() as i32,
                ),
            );
        }
        _ => {
            let Ok(Some(target)) = order_target(
                state,
                &order,
                record.formation_slot,
                record.formation_size,
                record.patrol_cursor,
            ) else {
                clear_navigation(state, slot);
                return false;
            };
            if matches!(order, ScriptNpcOrder::Patrol { .. })
                && distance(state.entities.pos(slot), centre(target)) < 0.04
            {
                record.patrol_cursor += 1;
                record.controls.revision = revision;
                return true;
            }
            navigate(state, slot, target);
        }
    }
    false
}

pub(super) mod deployment;

fn allied(state: &TickState, policy: &ScriptEngagementPolicy, uuid: uuid::Uuid) -> bool {
    policy.allies.contains(&uuid.to_string())
        || custodian(state, uuid)
            .is_some_and(|(_, handle)| policy.allies.iter().any(|ally| ally == handle))
}

pub(super) fn physical_attack_permitted(
    state: &TickState,
    actor: EntitySlot,
    victim: EntitySlot,
    operation_id: &str,
    order_revision: u64,
) -> bool {
    if actor == victim {
        return false;
    }
    let Some((owner, _)) = custodian(state, state.entities.row(actor).uuid) else {
        return false;
    };
    let Some(control) = npc_controls(state, actor) else {
        return false;
    };
    if control.order_revision != order_revision
        || control
            .combat
            .as_ref()
            .is_none_or(|combat| combat.operation_id != operation_id)
    {
        return false;
    }
    let Some(ScriptNpcOrder::Attack { targets, policy }) = control.order else {
        return false;
    };
    let uuid = state.entities.row(victim).uuid;
    if !state.entities.body(victim).alive || allied(state, &policy, uuid) {
        return false;
    }
    targets.iter().any(|reference| {
        target(state, owner, &reference.target_ref)
            .ok()
            .flatten()
            .is_some_and(|target| {
                target.entity_uuid == uuid
                    && target.policy_revision == policy.revision
                    && reference.policy_revision == target.policy_revision
                    && reference.expires_revision == target.expires_revision
                    && target.expires_revision >= state.tick
                    && policy.permitted.contains(&target.category)
                    && category(state, victim) == target.category
            })
    })
}
