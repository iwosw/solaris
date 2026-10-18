//! World deployment transfers the sole exact equipment stack to a native body.
use super::*;
use crate::plugin_inventories as inventories;
use mc_data::{Identifier, ItemStack};
use mc_physics::mth;

const DEPLOYMENTS: &str = "npc/deployments";
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Deployment {
    pub(crate) entity_uuid: uuid::Uuid,
    pub(crate) owner_uuid: uuid::Uuid,
    pub(crate) principal: String,
    pub(crate) dimension: String,
    pub(crate) support: ScriptBlockPosition,
    pub(crate) support_state: u32,
    pub(crate) collider: mc_data::item_profiles::ShieldProfile,
    pub(crate) started_tick: u64,
    pub(crate) disabled_until: u64,
}
fn all(state: &TickState) -> Result<BTreeMap<String, Deployment>, ScriptOperationFailure> {
    read_domain(state, NATIVE, DEPLOYMENTS)
}
fn profile(state: &TickState, stack: &ItemStack) -> Option<mc_data::item_profiles::ShieldProfile> {
    state
        .registry
        .item_facts
        .custom_for_stack(stack, &state.registry.items)?
        .facts
        .physical_profile
        .as_ref()?
        .shield
        .clone()
}
fn equipment(handle: &str) -> ScriptInventoryEndpoint {
    ScriptInventoryEndpoint::NpcEquipment {
        handle: handle.to_owned(),
    }
}

pub(super) fn execute(
    state: &mut TickState,
    id: &str,
    operation: &ScriptNpcOrderOperation,
) -> Result<ScriptOperationOutcome, ScriptOperationFailure> {
    let (handle, expected, permit) = match operation {
        ScriptNpcOrderOperation::UseItem {
            handle,
            expected_revision,
            execution_permit,
            ..
        }
        | ScriptNpcOrderOperation::DeployItem {
            handle,
            expected_revision,
            execution_permit,
            ..
        }
        | ScriptNpcOrderOperation::RetractItem {
            handle,
            expected_revision,
            execution_permit,
            ..
        }
        | ScriptNpcOrderOperation::CaptureItem {
            handle,
            expected_revision,
            execution_permit,
            ..
        } => (handle, *expected_revision, execution_permit.as_ref()),
        _ => return Err(ScriptOperationFailure::InvalidRequest),
    };
    validate_permit(state, id, handle, permit)?;
    let slot = owned_slot(state, id, handle)?;
    let actor_uuid = state.entities.row(slot).uuid;
    let mut records = records(state, id)?;
    let record = records
        .get_mut(handle)
        .ok_or(ScriptOperationFailure::Forbidden)?;
    if record.controls.revision != expected {
        return Err(ScriptOperationFailure::StaleRevision);
    }
    let revision = next_revision(state, id);
    let mut deployments = all(state)?;
    let result = match operation {
        ScriptNpcOrderOperation::UseItem {
            equipment_slot,
            active,
            ..
        } => {
            let hand = if *equipment_slot == 0 {
                crate::boundary::Hand::Main
            } else {
                crate::boundary::Hand::Off
            };
            if !crate::physical_defense::set_item_use(state, slot, hand, *active) {
                return Err(ScriptOperationFailure::Forbidden);
            }
            record.controls.item_use = crate::physical_defense::item_use(state, slot);
            ScriptNpcOrderResult::ItemUsed {
                handle: handle.clone(),
                revision,
            }
        }
        ScriptNpcOrderOperation::DeployItem {
            equipment_slot,
            entity_uuid,
            support,
            position,
            heading_degrees,
            ..
        } => {
            let uuid = uuid::Uuid::parse_str(entity_uuid)
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            if state.entities.slot_of_uuid(uuid).is_some() || deployments.contains_key(entity_uuid)
            {
                return Err(ScriptOperationFailure::OperationConflict);
            }
            let endpoint = equipment(handle);
            let mut image = inventories::load(state, id, &endpoint)?;
            let stack = image
                .slots
                .get(equipment_slot)
                .filter(|stack| stack.count == 1)
                .cloned()
                .ok_or(ScriptOperationFailure::InsufficientItems)?;
            let collider = profile(state, &stack)
                .filter(|profile| profile.deployable)
                .ok_or(ScriptOperationFailure::Forbidden)?;
            if stack.damage.unwrap_or(0).max(0) as u32
                >= stack
                    .extended()
                    .max_damage
                    .unwrap_or(collider.max_durability)
            {
                return Err(ScriptOperationFailure::InvalidRequest);
            }
            let pos = [position.x(), position.y(), position.z()];
            if distance(state.entities.pos(slot), pos) > 16.0
                || pos[0].floor() != f64::from(support.x)
                || pos[2].floor() != f64::from(support.z)
                || pos[1] != f64::from(support.y + 1)
            {
                return Err(ScriptOperationFailure::Blocked);
            }
            let support_state = state
                .world
                .block_at(support.x, support.y, support.z)
                .ok_or(ScriptOperationFailure::Unloaded)?;
            let support_boxes = state
                .registry
                .collision_boxes
                .get(support_state as usize)
                .ok_or(ScriptOperationFailure::Blocked)?;
            if !support_boxes
                .iter()
                .any(|shape| shape.as_blocks()[4] >= 1.0)
            {
                return Err(ScriptOperationFailure::Blocked);
            }
            let type_id = state
                .registry
                .entity_types
                .id_of(&Identifier::parse("minecraft:armor_stand").expect("native stand ID"))
                .ok_or(ScriptOperationFailure::RuntimeUnavailable)?;
            let blueprint = state
                .registry
                .blueprint(type_id)
                .ok_or(ScriptOperationFailure::RuntimeUnavailable)?;
            if !crate::settlements::allocator::clear_body(
                &state.world,
                &state.registry,
                pos,
                blueprint.half_width,
                blueprint.height,
            ) {
                return Err(ScriptOperationFailure::Blocked);
            }
            let health = blueprint.params.max_health;
            image.slots.insert(*equipment_slot, ItemStack::EMPTY);
            inventories::commit(state, id, BTreeMap::from([(endpoint, image)]))?;
            let entity = state.spawn_mob(type_id, pos, health);
            let stand = state
                .entities
                .slot_of(entity)
                .expect("native deployed stand");
            state.entities.set_uuid(stand, uuid);
            state
                .entities
                .set_rot(stand, f32::from(*heading_degrees), 0.0);
            let row = state.entities.row_mut(stand);
            row.persistent = true;
            row.controls.no_ai = true;
            row.controls.invulnerable = true;
            row.loot.drop_chances = [0.0; 6];
            std::sync::Arc::make_mut(&mut row.appearance).equipment[0] = Some(stack);
            state.plugin_spawned.push(entity);
            state.appearance_changed.push(entity);
            deployments.insert(
                entity_uuid.clone(),
                Deployment {
                    entity_uuid: uuid,
                    owner_uuid: actor_uuid,
                    principal: id.to_owned(),
                    dimension: state.dimension.name().to_owned(),
                    support: *support,
                    support_state,
                    collider,
                    started_tick: state.tick,
                    disabled_until: 0,
                },
            );
            ScriptNpcOrderResult::ItemDeployed {
                handle: handle.clone(),
                entity_uuid: entity_uuid.clone(),
                revision,
            }
        }
        ScriptNpcOrderOperation::RetractItem {
            entity_uuid,
            equipment_slot,
            ..
        } => {
            let deployment = deployments
                .get(entity_uuid)
                .ok_or(ScriptOperationFailure::NotFound)?;
            if deployment.principal != id || deployment.owner_uuid != actor_uuid {
                return Err(ScriptOperationFailure::Forbidden);
            }
            let target = state
                .entities
                .slot_of_uuid(deployment.entity_uuid)
                .ok_or(ScriptOperationFailure::Unloaded)?;
            if distance(state.entities.pos(slot), state.entities.pos(target)) > 16.0 {
                return Err(ScriptOperationFailure::Blocked);
            }
            let stack = state.entities.row(target).appearance.equipment[0]
                .clone()
                .filter(|stack| stack.count == 1)
                .ok_or(ScriptOperationFailure::NotFound)?;
            let endpoint = equipment(handle);
            let mut image = inventories::load(state, id, &endpoint)?;
            if image
                .slots
                .get(equipment_slot)
                .is_none_or(|stack| !stack.is_empty())
            {
                return Err(ScriptOperationFailure::Capacity);
            }
            image.slots.insert(*equipment_slot, stack);
            inventories::commit(state, id, BTreeMap::from([(endpoint, image)]))?;
            retire(state, target);
            deployments.remove(entity_uuid);
            ScriptNpcOrderResult::ItemRetracted {
                handle: handle.clone(),
                entity_uuid: entity_uuid.clone(),
                revision,
            }
        }
        ScriptNpcOrderOperation::CaptureItem { entity_uuid, .. } => {
            let deployment = deployments
                .get_mut(entity_uuid)
                .ok_or(ScriptOperationFailure::NotFound)?;
            let target = state
                .entities
                .slot_of_uuid(deployment.entity_uuid)
                .ok_or(ScriptOperationFailure::Unloaded)?;
            if deployment.dimension != state.dimension.name()
                || distance(state.entities.pos(slot), state.entities.pos(target)) > 16.0
                || state.entities.row(target).appearance.equipment[0]
                    .as_ref()
                    .is_none_or(|stack| stack.count != 1)
            {
                return Err(ScriptOperationFailure::Blocked);
            }
            if deployment.principal == id && deployment.owner_uuid == actor_uuid {
                return Err(ScriptOperationFailure::OperationConflict);
            }
            deployment.principal = id.to_owned();
            deployment.owner_uuid = actor_uuid;
            state
                .appearance_changed
                .push(state.entities.body(target).id);
            ScriptNpcOrderResult::ItemCaptured {
                handle: handle.clone(),
                entity_uuid: entity_uuid.clone(),
                revision,
            }
        }
        _ => return Err(ScriptOperationFailure::InvalidRequest),
    };
    record.controls.revision = revision;
    save_records(state, id, &records, revision);
    save(state, NATIVE, DEPLOYMENTS, &deployments);
    Ok(order_result(revision, result))
}
fn retire(state: &mut TickState, slot: EntitySlot) {
    let (body, row) = state.entities.parts_mut(slot);
    std::sync::Arc::make_mut(&mut row.appearance).equipment[0] = None;
    row.health = 0.0;
    body.alive = false;
    row.loot.death_handled = true;
    state.appearance_changed.push(body.id);
}

pub(crate) fn projectile_hit(
    state: &TickState,
    slot: EntitySlot,
    start: [f64; 3],
    end: [f64; 3],
    radius: f64,
) -> Option<Option<f64>> {
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let value = state
        .plugins
        .get_domain(NATIVE, DEPLOYMENTS)?
        .get(row.uuid.to_string())?;
    let deployment: Deployment = serde_json::from_value(value.clone()).ok()?;
    if !body.alive
        || row.appearance.equipment[0]
            .as_ref()
            .is_none_or(|stack| stack.count != 1)
        || deployment.dimension != state.dimension.name()
        || state.tick < deployment.started_tick + u64::from(deployment.collider.activation_ticks)
        || state.tick < deployment.disabled_until
        || state.world.block_at(
            deployment.support.x,
            deployment.support.y,
            deployment.support.z,
        ) != Some(deployment.support_state)
    {
        return Some(None);
    }
    let p = state.entities.pos(slot);
    let yaw = f64::from(state.entities.rot(slot)[0]).to_radians();
    let normal = [-f64::from(mth::sin(yaw)), f64::from(mth::cos(yaw))];
    let tangent = [normal[1], -normal[0]];
    let plane = [
        p[0] + normal[0] * deployment.collider.forward_offset_blocks,
        p[2] + normal[1] * deployment.collider.forward_offset_blocks,
    ];
    let before = (start[0] - plane[0]) * normal[0] + (start[2] - plane[1]) * normal[1];
    let after = (end[0] - plane[0]) * normal[0] + (end[2] - plane[1]) * normal[1];
    if before <= radius || after >= before {
        return Some(None);
    }
    let t = (before - radius) / (before - after);
    if !(0.0..=1.0).contains(&t) {
        return Some(None);
    }
    let x = start[0] + (end[0] - start[0]) * t;
    let y = start[1] + (end[1] - start[1]) * t;
    let z = start[2] + (end[2] - start[2]) * t;
    let lateral = (x - plane[0]) * tangent[0] + (z - plane[1]) * tangent[1];
    let bottom = p[1] + deployment.collider.bottom_offset_blocks;
    if lateral.abs() > deployment.collider.width_blocks * 0.5 + radius
        || y < bottom - radius
        || y > bottom + deployment.collider.height_blocks + radius
    {
        return Some(None);
    }
    Some(Some(t))
}
pub(crate) fn absorb_impact(
    state: &mut TickState,
    slot: EntitySlot,
    impact: crate::physical_defense::PhysicalImpact,
) -> bool {
    apply_impact(state, slot, impact, false).is_some()
}

pub(crate) fn adjudicate_projectile(
    state: &mut TickState,
    slot: EntitySlot,
    impact: &crate::physical_defense::PhysicalImpact,
) -> Option<bool> {
    apply_impact(state, slot, *impact, true)
}

fn apply_impact(
    state: &mut TickState,
    slot: EntitySlot,
    impact: crate::physical_defense::PhysicalImpact,
    projectile: bool,
) -> Option<bool> {
    use crate::physical_defense::{
        DeployedShieldPose, ShieldActivation, plan_deployed_item_damage,
        plan_deployed_shield_impact,
    };
    let uuid = state.entities.row(slot).uuid;
    let mut deployments = all(state).ok()?;
    let deployment = deployments.get_mut(&uuid.to_string())?;
    if !state.entities.body(slot).alive
        || deployment.dimension != state.dimension.name()
        || state.world.block_at(
            deployment.support.x,
            deployment.support.y,
            deployment.support.z,
        ) != Some(deployment.support_state)
    {
        return Some(false);
    }
    let Some(mut stack) = state.entities.row(slot).appearance.equipment[0]
        .clone()
        .filter(|stack| stack.count == 1)
    else {
        return Some(false);
    };
    let maximum = stack
        .extended()
        .max_damage
        .unwrap_or(deployment.collider.max_durability);
    let damage = stack.damage.unwrap_or(0).max(0) as u32;
    let pose = DeployedShieldPose {
        position: state.entities.pos(slot),
        yaw: state.entities.rot(slot)[0],
    };
    let activation = ShieldActivation {
        slot: 0,
        started_tick: deployment.started_tick,
        disabled_until: deployment.disabled_until,
    };
    let plan = plan_deployed_shield_impact(
        &deployment.collider,
        pose,
        impact,
        damage,
        maximum,
        activation,
        state.tick,
    );
    let blocked = plan.as_ref().is_some_and(|plan| plan.shield_blocked);
    let wear = if let Some(plan) = plan {
        let changed = plan
            .shield_disabled_until
            .is_some_and(|disabled| disabled != deployment.disabled_until);
        if let Some(disabled) = plan.shield_disabled_until {
            deployment.disabled_until = disabled;
        }
        if plan.wear[0].is_none() {
            if changed {
                save(state, NATIVE, DEPLOYMENTS, &deployments);
            }
            return Some(blocked);
        }
        plan.wear[0]
    } else if projectile {
        return Some(false);
    } else {
        plan_deployed_item_damage(&deployment.collider, impact, damage, maximum)
    };
    let Some(wear) = wear else {
        return Some(blocked);
    };
    if wear.expected_damage != damage {
        return Some(false);
    }
    if wear.broken {
        retire(state, slot);
        deployments.remove(&uuid.to_string());
    } else {
        stack.damage = Some(wear.updated_damage as i32);
        let (body, row) = state.entities.parts_mut(slot);
        std::sync::Arc::make_mut(&mut row.appearance).equipment[0] = Some(stack);
        state.appearance_changed.push(body.id);
    }
    save(state, NATIVE, DEPLOYMENTS, &deployments);
    Some(blocked)
}

pub(in crate::plugin_npcs) fn validate(state: &TickState) -> Result<(), ScriptOperationFailure> {
    for (uuid, record) in all(state)? {
        if uuid != record.entity_uuid.to_string()
            || record.entity_uuid.is_nil()
            || record.owner_uuid.is_nil()
            || record.principal.is_empty()
            || record.principal.len() > MAX_PLUGIN_ID_BYTES
            || crate::dimensions::Dimension::parse(&record.dimension).is_none()
            || record.started_tick > MAX_SCRIPT_WORLD_TIME
            || record.disabled_until > MAX_SCRIPT_WORLD_TIME
            || !record.collider.deployable
            || record.collider.max_durability == 0
            || !record.collider.width_blocks.is_finite()
            || record.collider.width_blocks <= 0.0
            || !record.collider.height_blocks.is_finite()
            || record.collider.height_blocks <= 0.0
            || !record.collider.forward_offset_blocks.is_finite()
            || !record.collider.bottom_offset_blocks.is_finite()
        {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        record
            .support
            .validate()
            .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    }
    Ok(())
}
