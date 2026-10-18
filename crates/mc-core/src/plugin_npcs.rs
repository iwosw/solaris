//! Generic native NPC identity, physical controls and route authority.
//! Domain records retain ownership and receipts; bodies and cargo stay native.

use crate::boundary::{BlockPos, EntityId};
use crate::entities::EntitySlot;
use crate::tick::TickState;
use mc_script::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod death;
mod orders;
mod routes;
mod work;

const RECORDS: &str = "npc/records";
const RECORD_PREFIX: &str = "npc/record/";
const NATIVE: &str = "__native_npcs";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct NpcRecord {
    handle: String,
    entity_uuid: uuid::Uuid,
    generation_id: Option<String>,
    dimension: String,
    revision: u64,
    disposition: ScriptNpcLifecycle,
    previous_no_ai: bool,
    previous_persistent: bool,
    controls: ScriptNpcControlSnapshot,
    formation_slot: u16,
    formation_size: u16,
    patrol_cursor: usize,
    work_cursor: u32,
    haul_slot: Option<u8>,
    haul_hash: Option<String>,
}

pub(super) fn read_domain<T: serde::de::DeserializeOwned + Default>(
    state: &TickState,
    id: &str,
    key: &str,
) -> Result<T, ScriptOperationFailure> {
    state
        .plugins
        .get_domain(id, key)
        .map_or(Ok(T::default()), |value| {
            T::deserialize(value).map_err(|_| ScriptOperationFailure::RuntimeUnavailable)
        })
}

pub(super) fn save<T: Serialize>(state: &mut TickState, id: &str, key: &str, value: &T) {
    state.plugins.set_domain(
        id,
        key,
        serde_json::to_value(value).expect("native plugin domain image"),
    );
}

pub(super) fn records(
    state: &TickState,
    id: &str,
) -> Result<BTreeMap<String, NpcRecord>, ScriptOperationFailure> {
    if state.plugins.get_domain(id, RECORDS).is_some() {
        return Err(ScriptOperationFailure::RuntimeUnavailable);
    }
    state
        .plugins
        .domain_entries(id)
        .filter_map(|(key, value)| {
            key.strip_prefix(RECORD_PREFIX)
                .map(|handle| (handle, value))
        })
        .map(|(handle, value)| {
            NpcRecord::deserialize(value)
                .map(|record| (handle.to_owned(), record))
                .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)
        })
        .collect()
}

pub(super) fn next_revision(state: &TickState, id: &str) -> u64 {
    let previous = state
        .plugins
        .get_domain(id, "npc/revision")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    previous
        .max(state.tick)
        .saturating_add(1)
        .min(MAX_SCRIPT_WORLD_TIME)
}

pub(super) fn save_records(
    state: &mut TickState,
    id: &str,
    records: &BTreeMap<String, NpcRecord>,
    revision: u64,
) {
    for record in records.values() {
        save_record(state, id, record);
    }
    state
        .plugins
        .set_domain(id, "npc/revision", serde_json::Value::from(revision));
}

fn save_record(state: &mut TickState, id: &str, record: &NpcRecord) {
    let key = format!("{RECORD_PREFIX}{}", record.handle);
    let image = serde_json::to_value(record).expect("native NPC record");
    if state.plugins.get_domain(id, &key) != Some(&image) {
        state.plugins.set_domain(id, &key, image);
    }
}

pub(super) fn committed(revision: u64, payload: ScriptOperationPayload) -> ScriptOperationOutcome {
    ScriptOperationOutcome::committed(revision, payload).expect("native NPC typed outcome")
}

pub(super) fn npc_result(revision: u64, result: ScriptNpcResult) -> ScriptOperationOutcome {
    committed(
        revision,
        ScriptOperationPayload::Npc {
            result: Box::new(result),
        },
    )
}

pub(super) fn order_result(revision: u64, result: ScriptNpcOrderResult) -> ScriptOperationOutcome {
    committed(
        revision,
        ScriptOperationPayload::NpcOrder {
            result: Box::new(result),
        },
    )
}

pub(crate) fn execute(
    state: &mut TickState,
    plugin_id: &str,
    request: &ScriptOperationRequest,
) -> ScriptOperationOutcome {
    let result = match request.operation() {
        ScriptOperation::Npc { operation } => execute_identity(state, plugin_id, operation),
        ScriptOperation::NpcOrder { operation } => orders::execute(state, plugin_id, operation),
        ScriptOperation::NpcRoute { operation } => routes::execute(state, plugin_id, operation),
        _ => Err(ScriptOperationFailure::InvalidRequest),
    };
    result.unwrap_or_else(ScriptOperationOutcome::rejected)
}

pub(crate) fn owned_slot(
    state: &TickState,
    id: &str,
    handle: &str,
) -> Result<EntitySlot, ScriptOperationFailure> {
    let value = state
        .plugins
        .get_domain(id, &format!("{RECORD_PREFIX}{handle}"))
        .ok_or(ScriptOperationFailure::Forbidden)?;
    let record =
        NpcRecord::deserialize(value).map_err(|_| ScriptOperationFailure::RuntimeUnavailable)?;
    if record.disposition != ScriptNpcLifecycle::AliveUnloaded
        || record.dimension != state.dimension.name()
    {
        return Err(if record.disposition == ScriptNpcLifecycle::AliveUnloaded {
            ScriptOperationFailure::Unloaded
        } else {
            ScriptOperationFailure::NotFound
        });
    }
    state
        .entities
        .slot_of_uuid(record.entity_uuid)
        .filter(|slot| physical_slot_loaded(state, *slot))
        .ok_or(ScriptOperationFailure::Unloaded)
}

/// Global custody/control records stay with the root while bodies belong to
/// their dimension. Pending work retirement must read both authoritative owners.
pub(crate) fn pending_work_is_current(
    state: &TickState,
    owner: &str,
    handle: &str,
    entity_uuid: uuid::Uuid,
    work_revision: u64,
    dimension: crate::dimensions::Dimension,
) -> bool {
    let Some(value) = state
        .plugins
        .get_domain(owner, &format!("{RECORD_PREFIX}{handle}"))
    else {
        return false;
    };
    let Ok(record) = NpcRecord::deserialize(value) else {
        return false;
    };
    if record.entity_uuid != entity_uuid
        || record.disposition != ScriptNpcLifecycle::AliveUnloaded
        || record.dimension != dimension.name()
    {
        return false;
    }
    let Ok(world) = crate::plugin_inventories::world(state, &record.dimension) else {
        return false;
    };
    let Some(slot) = world.entities.slot_of_uuid(entity_uuid) else {
        return false;
    };
    world.entities.body(slot).alive
        && physical_slot_loaded(world, slot)
        && record.controls.work.is_some_and(|work| {
            work.revision == work_revision
                && !matches!(
                    work.state,
                    ScriptWorkState::Suspended
                        | ScriptWorkState::Cancelled
                        | ScriptWorkState::Committed
                )
        })
}

pub(crate) fn validate_permit(
    state: &TickState,
    id: &str,
    handle: &str,
    permit: Option<&ScriptNpcExecutionPermit>,
) -> Result<(), ScriptOperationFailure> {
    routes::validate_permit(state, id, handle, permit)
}

fn foreign_owner(
    state: &TickState,
    id: &str,
    entity: uuid::Uuid,
) -> Result<bool, ScriptOperationFailure> {
    let custody: BTreeMap<String, (String, String)> = read_domain(state, NATIVE, "npc/custody")?;
    Ok(custody
        .get(&entity.to_string())
        .is_some_and(|(owner, _)| owner != id))
}

pub(super) fn loaded(state: &TickState, slot: EntitySlot) -> ScriptNpcLoadedState {
    let row = state.entities.row(slot);
    let position = state.entities.pos(slot);
    let rotation = state.entities.rot(slot);
    let carried =
        row.villager
            .as_ref()
            .into_iter()
            .flat_map(|v| &v.inventory)
            .flatten()
            .filter_map(|stack| {
                state.registry.items.name_of(stack.item_id).map(|name| {
                    ScriptNpcItemSummary::new(name.to_string(), stack.count.max(0) as u32)
                })
            })
            .collect();
    let mut value = ScriptNpcLoadedState::new(
        state.dimension.name().to_owned(),
        ScriptPosition::try_new(position[0], position[1], position[2])
            .expect("native finite position"),
        row.health,
        max_health(state, slot) as f32,
        rotation[0],
        carried,
    );
    value.native_age_ticks = row.villager.as_ref().map(|v| v.age);
    value.birth_origin = row.villager.as_ref().and_then(|v| v.birth_origin.clone());
    value
}

pub(super) fn snapshot(state: &TickState, record: &NpcRecord) -> ScriptNpcSnapshot {
    let pose = (record.disposition == ScriptNpcLifecycle::AliveUnloaded
        && record.dimension == state.dimension.name())
    .then(|| state.entities.slot_of_uuid(record.entity_uuid))
    .flatten()
    .filter(|slot| physical_slot_loaded(state, *slot))
    .map(|slot| loaded(state, slot));
    ScriptNpcSnapshot::new(
        record.handle.clone(),
        record.entity_uuid.to_string(),
        if pose.is_some() {
            ScriptNpcLifecycle::AliveLoaded
        } else {
            record.disposition
        },
        record.revision,
        record.generation_id.clone(),
        pose,
    )
}

pub(super) fn max_health(state: &TickState, slot: EntitySlot) -> f64 {
    let body = state.entities.body(slot);
    let base = match body.kind {
        crate::entities::EntityKind::Mob { type_id } => state
            .registry
            .blueprint(type_id)
            .map_or(20.0, |b| b.params.max_health),
        _ => 20.0,
    };
    crate::entity_loot::effective_attribute(
        &state.registry.item_rules(),
        state.entities.row(slot),
        "max_health",
        f64::from(base),
    )
}

fn new_record(
    state: &mut TickState,
    id: &str,
    slot: EntitySlot,
    generation_id: Option<String>,
    revision: u64,
) -> Result<NpcRecord, ScriptOperationFailure> {
    let mut custody: BTreeMap<String, (String, String)> =
        read_domain(state, NATIVE, "npc/custody")?;
    let row = state.entities.row(slot);
    if row
        .villager
        .as_ref()
        .is_none_or(|v| v.wandering || v.conversion_time.is_some())
    {
        return Err(ScriptOperationFailure::Forbidden);
    }
    let handle = if let Some(generation) = &generation_id {
        npc_handle_for_generation(id, generation)
    } else {
        npc_handle_for_entity(id, &row.uuid.to_string())
    }
    .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    let record = NpcRecord {
        handle: handle.clone(),
        entity_uuid: row.uuid,
        generation_id,
        dimension: state.dimension.name().to_owned(),
        revision,
        disposition: ScriptNpcLifecycle::AliveUnloaded,
        previous_no_ai: row.controls.no_ai,
        previous_persistent: row.persistent,
        controls: ScriptNpcControlSnapshot::new(
            handle,
            0,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            Vec::new(),
        ),
        formation_slot: 0,
        formation_size: 1,
        patrol_cursor: 0,
        work_cursor: 0,
        haul_slot: None,
        haul_hash: None,
    };
    if custody
        .get(&record.entity_uuid.to_string())
        .is_some_and(|binding| binding.0 != id || binding.1 != record.handle)
    {
        return Err(ScriptOperationFailure::Forbidden);
    }
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    row.controls.no_ai = true;
    state.appearance_changed.push(state.entities.body(slot).id);
    custody.insert(
        record.entity_uuid.to_string(),
        (id.to_owned(), record.handle.clone()),
    );
    save(state, NATIVE, "npc/custody", &custody);
    Ok(record)
}

fn execute_identity(
    state: &mut TickState,
    id: &str,
    operation: &ScriptNpcOperation,
) -> Result<ScriptOperationOutcome, ScriptOperationFailure> {
    let mut all = records(state, id)?;
    let revision = next_revision(state, id);
    match operation {
        ScriptNpcOperation::Query { handles, cursor } => {
            let mut page = Vec::new();
            let selected = all.values().filter(|record| {
                (handles.is_empty() || handles.binary_search(&record.handle).is_ok())
                    && cursor.as_ref().is_none_or(|cursor| &record.handle > cursor)
            });
            for record in selected.take(MAX_NPC_PAGE + 1) {
                page.push(snapshot(state, record));
            }
            let next = if page.len() > MAX_NPC_PAGE {
                page.pop();
                page.last().map(|npc| npc.handle.clone())
            } else {
                None
            };
            Ok(npc_result(
                revision,
                ScriptNpcResult::Page {
                    npcs: page,
                    cursor: next,
                },
            ))
        }
        ScriptNpcOperation::Spawn {
            generation_id,
            position,
            entity_type,
            ..
        } => {
            let handle = npc_handle_for_generation(id, generation_id)
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            if let Some(record) = all.get(&handle) {
                return Ok(npc_result(
                    record.revision,
                    ScriptNpcResult::Snapshot {
                        npc: snapshot(state, record),
                    },
                ));
            }
            // Native carry is specifically the vanilla eight-slot villager inventory.
            if entity_type != "minecraft:villager" {
                return Err(ScriptOperationFailure::Forbidden);
            }
            let type_id = state
                .registry
                .entity_types
                .id_of(
                    &mc_data::Identifier::parse(entity_type)
                        .map_err(|_| ScriptOperationFailure::InvalidRequest)?,
                )
                .ok_or(ScriptOperationFailure::NotFound)?;
            let blueprint = state
                .registry
                .blueprint(type_id)
                .ok_or(ScriptOperationFailure::RuntimeUnavailable)?;
            let health = blueprint.params.max_health;
            let pos = [position.x(), position.y(), position.z()];
            if !crate::settlements::allocator::clear_body(
                &state.world,
                &state.registry,
                pos,
                blueprint.half_width,
                blueprint.height,
            ) {
                return Err(ScriptOperationFailure::Blocked);
            }
            let uuid = uuid::Uuid::from_bytes(
                npc_entity_uuid(generation_id)
                    .map_err(|_| ScriptOperationFailure::InvalidRequest)?,
            );
            if foreign_owner(state, id, uuid)? {
                return Err(ScriptOperationFailure::Forbidden);
            }
            let slot = if let Some(slot) = state.entities.slot_of_uuid(uuid) {
                slot
            } else {
                let entity = state.spawn_mob(type_id, pos, health);
                let slot = state
                    .entities
                    .slot_of(entity)
                    .expect("native spawned villager");
                state.entities.set_uuid(slot, uuid);
                state.plugin_spawned.push(entity);
                slot
            };
            let record = new_record(state, id, slot, Some(generation_id.clone()), revision)?;
            let npc = snapshot(state, &record);
            all.insert(record.handle.clone(), record);
            save_records(state, id, &all, revision);
            Ok(npc_result(revision, ScriptNpcResult::Snapshot { npc }))
        }
        ScriptNpcOperation::Adopt {
            actor_id,
            entity_uuid,
            expected_entity_revision,
            ..
        } => {
            let uuid = uuid::Uuid::parse_str(entity_uuid)
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            if let Some(record) = all.values().find(|record| {
                record.entity_uuid == uuid && record.disposition != ScriptNpcLifecycle::Released
            }) {
                if record.revision != *expected_entity_revision {
                    return Err(ScriptOperationFailure::StaleRevision);
                }
                return Ok(npc_result(
                    record.revision,
                    ScriptNpcResult::Snapshot {
                        npc: snapshot(state, record),
                    },
                ));
            }
            let actor_id = state
                .connections
                .entity_of(crate::boundary::ConnectionId(*actor_id))
                .ok_or(ScriptOperationFailure::Forbidden)?;
            let actor = state
                .entities
                .slot_of(actor_id)
                .filter(|_| state.players.contains_key(&actor_id))
                .ok_or(ScriptOperationFailure::Forbidden)?;
            let slot = state
                .entities
                .slot_of_uuid(uuid)
                .ok_or(ScriptOperationFailure::Unloaded)?;
            let a = state.entities.pos(actor);
            let b = state.entities.pos(slot);
            if distance(a, b) > MAX_NPC_CLAIM_DISTANCE.powi(2) {
                return Err(ScriptOperationFailure::Forbidden);
            }
            adopt(
                state,
                id,
                &mut all,
                slot,
                *expected_entity_revision,
                revision,
            )
        }
        ScriptNpcOperation::AdoptNativeChild {
            entity_uuid,
            parent_handles,
            expected_entity_revision,
            ..
        } => {
            let uuid = uuid::Uuid::parse_str(entity_uuid)
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            let parents = parent_handles
                .iter()
                .map(|handle| {
                    all.get(handle)
                        .filter(|p| p.disposition == ScriptNpcLifecycle::AliveUnloaded)
                        .map(|p| p.entity_uuid.to_string())
                        .ok_or(ScriptOperationFailure::Forbidden)
                })
                .collect::<Result<Vec<_>, _>>()?;
            // Birth provenance lives on the native villager columns, not a plugin
            // ledger: retained bodies (loaded or not) authorize custody.
            let slot = state
                .entities
                .slot_of_uuid_any(uuid)
                .ok_or(ScriptOperationFailure::Unloaded)?;
            let proven = state
                .entities
                .row(slot)
                .villager
                .as_ref()
                .and_then(|v| v.birth_origin.as_ref())
                .is_some_and(|origin| origin.parents.iter().all(|parent| parents.contains(parent)));
            if !proven {
                return Err(ScriptOperationFailure::Forbidden);
            }
            let slot = state
                .entities
                .slot_of_uuid(uuid)
                .ok_or(ScriptOperationFailure::Unloaded)?;
            adopt(
                state,
                id,
                &mut all,
                slot,
                *expected_entity_revision,
                revision,
            )
        }
        ScriptNpcOperation::QueryBirths {
            parent_handles,
            cursor,
            limit,
        } => {
            let parents = parent_handles
                .iter()
                .map(|handle| {
                    all.get(handle)
                        .map(|p| p.entity_uuid.to_string())
                        .ok_or(ScriptOperationFailure::Forbidden)
                })
                .collect::<Result<Vec<_>, _>>()?;
            // Observations cover retained native bodies only; removed rows are
            // gone, not paged as tombstones. The cursor is validated against
            // the current tick ceiling so a removed newest child never turns a
            // valid resume cursor into InvalidRequest.
            let after = cursor.unwrap_or(0);
            let ceiling = state.tick.saturating_mul(65_536).saturating_add(65_535);
            if after > ceiling {
                return Err(ScriptOperationFailure::InvalidRequest);
            }
            let mut found: Vec<(u64, EntitySlot)> = state
                .entities
                .slots()
                .filter_map(|slot| {
                    let row = state.entities.row(slot);
                    let v = row.villager.as_ref()?;
                    let sequence = v.birth_sequence?;
                    if sequence <= after {
                        return None;
                    }
                    let origin = v.birth_origin.as_ref()?;
                    origin
                        .parents
                        .iter()
                        .any(|parent| parents.contains(parent))
                        .then_some((sequence, slot))
                })
                .collect();
            found.sort_unstable_by_key(|(sequence, _)| *sequence);
            let mut births = Vec::new();
            let mut inspected = after;
            let mut more = false;
            for (sequence, slot) in found {
                if births.len() == usize::from(*limit) {
                    more = true;
                    break;
                }
                inspected = sequence;
                let row = state.entities.row(slot);
                let origin = row
                    .villager
                    .as_ref()
                    .and_then(|v| v.birth_origin.clone())
                    .expect("scanned birth origin");
                let dimension = state.dimension.name().to_owned();
                let pose = if state.entities.body(slot).alive {
                    state
                        .entities
                        .slot_of_uuid(row.uuid)
                        .filter(|live| physical_slot_loaded(state, *live))
                        .map(|live| loaded(state, live))
                } else {
                    None
                };
                births.push(ScriptNativeBirthObservation::new(
                    row.uuid.to_string(),
                    origin,
                    if !state.entities.body(slot).alive {
                        ScriptNpcLifecycle::Dead
                    } else if pose.is_some() {
                        ScriptNpcLifecycle::AliveLoaded
                    } else {
                        ScriptNpcLifecycle::AliveUnloaded
                    },
                    dimension,
                    pose,
                ));
            }
            Ok(npc_result(
                revision,
                ScriptNpcResult::BirthPage {
                    births,
                    cursor: more.then_some(inspected),
                    history_start: 0,
                    resume_cursor: inspected,
                },
            ))
        }
        ScriptNpcOperation::Release {
            handle,
            expected_revision,
            ..
        } => {
            validate_permit(state, id, handle, None)?;
            let record = all
                .get_mut(handle)
                .ok_or(ScriptOperationFailure::Forbidden)?;
            if record.revision != *expected_revision {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            if record.disposition != ScriptNpcLifecycle::AliveUnloaded {
                return Err(ScriptOperationFailure::NotFound);
            }
            if let Some(slot) = state.entities.slot_of_uuid(record.entity_uuid) {
                let row = state.entities.row_mut(slot);
                row.controls.no_ai = record.previous_no_ai;
                row.persistent = record.previous_persistent;
                if let Some(v) = &mut row.villager {
                    v.navigation_path.clear();
                    v.navigation_target = None;
                    v.navigation_chunks.clear();
                }
                state.appearance_changed.push(state.entities.body(slot).id);
            }
            record.disposition = ScriptNpcLifecycle::Released;
            record.revision = revision;
            record.controls.order = None;
            record.controls.order_revision = 0;
            record.controls.pending_work = None;
            if let Some(work) = &mut record.controls.work {
                work.state = ScriptWorkState::Cancelled;
                work.reason = None;
                work.due_tick = None;
                work.suspension_reason = None;
                work.revision = revision;
            }
            record.controls.revision = revision;
            let npc = snapshot(state, record);
            let mut custody: BTreeMap<String, (String, String)> =
                read_domain(state, NATIVE, "npc/custody")?;
            custody.remove(&record.entity_uuid.to_string());
            save(state, NATIVE, "npc/custody", &custody);
            save_records(state, id, &all, revision);
            Ok(npc_result(revision, ScriptNpcResult::Snapshot { npc }))
        }
        ScriptNpcOperation::ApplyHealthDelta {
            handle,
            expected_revision,
            source,
            material,
            health_delta_milli,
            ..
        } => {
            validate_permit(state, id, handle, None)?;
            let record = all
                .get_mut(handle)
                .ok_or(ScriptOperationFailure::Forbidden)?;
            if record.revision != *expected_revision {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            let slot = owned_slot(state, id, handle)?;
            let row = state.entities.row(slot);
            let after = row.health + *health_delta_milli as f32 / 1000.0;
            if *health_delta_milli == 0
                || after <= 0.0
                || f64::from(after) > max_health(state, slot)
            {
                return Err(ScriptOperationFailure::InvalidRequest);
            }
            let image = crate::plugin_inventories::plan_debit(
                state,
                id,
                source,
                &BTreeMap::from([(material.resource_id.clone(), material.quantity)]),
            )?;
            crate::plugin_inventories::commit(
                state,
                id,
                BTreeMap::from([(source.clone(), image)]),
            )?;
            let row = state.entities.row_mut(slot);
            row.health = after;
            std::sync::Arc::make_mut(&mut row.appearance).health = after;
            state.appearance_changed.push(state.entities.body(slot).id);
            record.revision = revision;
            save_records(state, id, &all, revision);
            Ok(npc_result(
                revision,
                ScriptNpcResult::HealthChanged {
                    handle: handle.clone(),
                },
            ))
        }
        _ => Err(ScriptOperationFailure::InvalidRequest),
    }
}

fn adopt(
    state: &mut TickState,
    id: &str,
    all: &mut BTreeMap<String, NpcRecord>,
    slot: EntitySlot,
    expected: u64,
    revision: u64,
) -> Result<ScriptOperationOutcome, ScriptOperationFailure> {
    let uuid = state.entities.row(slot).uuid;
    if foreign_owner(state, id, uuid)? {
        return Err(ScriptOperationFailure::Forbidden);
    }
    if let Some(record) = all.values().find(|record| {
        record.entity_uuid == uuid && record.disposition != ScriptNpcLifecycle::Released
    }) {
        if record.revision != expected {
            return Err(ScriptOperationFailure::StaleRevision);
        }
        return Ok(npc_result(
            record.revision,
            ScriptNpcResult::Snapshot {
                npc: snapshot(state, record),
            },
        ));
    }
    if expected != 0 {
        return Err(ScriptOperationFailure::StaleRevision);
    }
    let record = new_record(state, id, slot, None, revision)?;
    let npc = snapshot(state, &record);
    all.insert(record.handle.clone(), record);
    save_records(state, id, all, revision);
    Ok(npc_result(revision, ScriptNpcResult::Snapshot { npc }))
}

pub(super) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}
pub(super) fn centre(pos: ScriptBlockPosition) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}
pub(super) fn block(pos: ScriptBlockPosition) -> BlockPos {
    BlockPos {
        x: pos.x,
        y: pos.y,
        z: pos.z,
    }
}

/// Cheap row gate shared by [`motion_target`] and the tick's row preparation:
/// only persistent NoAI villagers are plugin-driven.
pub(crate) fn needs_state(row: &crate::entities::ColdRow) -> bool {
    row.controls.no_ai && row.persistent && row.villager.is_some()
}

/// Supplying a target does not change position: the tick's native motion resolver
/// consumes it once, with its usual collision, gravity and loaded-chunk checks.
pub(crate) fn motion_target(state: &TickState, slot: EntitySlot) -> Option<[f64; 3]> {
    let row = state.entities.row(slot);
    if !needs_state(row) {
        return None;
    }
    let v = row.villager.as_ref()?;
    let next = *v.navigation_path.last()?;
    crate::settlements::allocator::standing(
        &state.world,
        &state.registry,
        next,
        state.entities.body(slot).half_width,
        state.entities.body(slot).height,
    )
}

pub(super) fn navigate(
    state: &mut TickState,
    slot: EntitySlot,
    target: ScriptBlockPosition,
) -> bool {
    let Some(mut villager) = state.entities.row_mut(slot).villager.take() else {
        return false;
    };
    let target = block(target);
    let pos = state.entities.pos(slot);
    let row = state.entities.body(slot);
    let width = row.half_width;
    let height = row.height;
    let changed = villager.navigation_target != Some(target)
        || crate::settlements::allocator::route_changed(state, &villager);
    let mut reachable = true;
    if changed {
        let mut cache = crate::settlements::allocator::PoiCache::default();
        reachable = crate::settlements::allocator::standing(
            &state.world,
            &state.registry,
            target,
            width,
            height,
        )
        .is_some()
            && crate::settlements::allocator::find_path_exact(
                state,
                &mut cache,
                pos,
                target,
                width,
                height,
                &mut villager.navigation_path,
            );
        if reachable && villager.navigation_path.is_empty() {
            villager.navigation_path.push(target);
        }
        if !reachable {
            villager.navigation_path.clear();
        }
        if reachable {
            villager.navigation_target = Some(target);
            crate::settlements::allocator::capture_route(state, &mut villager, pos);
        }
    }
    while villager.navigation_path.last().is_some_and(|next| {
        crate::settlements::allocator::standing(&state.world, &state.registry, *next, width, height)
            .is_some_and(|p| {
                distance(pos, p)
                    < if villager.navigation_path.len() == 1 {
                        1.0e-8
                    } else {
                        0.04
                    }
            })
    }) {
        villager.navigation_path.pop();
    }
    if !changed && villager.navigation_path.is_empty() {
        reachable = crate::settlements::allocator::standing(
            &state.world,
            &state.registry,
            target,
            width,
            height,
        )
        .is_some_and(|p| distance(pos, p) < 1.0e-8);
    }
    state.entities.row_mut(slot).villager = Some(villager);
    reachable
}

pub(crate) fn advance(state: &mut TickState) {
    let owners = state
        .plugins
        .domain_plugins()
        .filter(|id| {
            *id != NATIVE
                && state
                    .plugins
                    .domain_entries(id)
                    .any(|(key, _)| key == RECORDS || key.starts_with(RECORD_PREFIX))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for id in owners {
        let Ok(mut all) = records(state, &id) else {
            crate::plugin::fail_native(state);
            return;
        };
        let revision = next_revision(state, &id);
        let mut changed = false;
        for record in all.values_mut() {
            if record.disposition != ScriptNpcLifecycle::AliveUnloaded
                || record.dimension != state.dimension.name()
            {
                continue;
            }
            let slot = state.entities.slot_of_uuid_any(record.entity_uuid);
            let dead = slot.is_some_and(|slot| !state.entities.body(slot).alive);
            if dead {
                record.disposition = ScriptNpcLifecycle::Dead;
                record.revision = revision;
                record.controls.order = None;
                record.controls.order_revision = 0;
                record.controls.pending_work = None;
                record.controls.revision = revision;
                if let Some(work) = &mut record.controls.work {
                    work.state = ScriptWorkState::Cancelled;
                    work.reason = None;
                    work.due_tick = None;
                    work.suspension_reason = None;
                    work.revision = revision;
                }
                changed = true;
                save_record(state, &id, record);
                continue;
            }
            if let Some(slot) = slot {
                let pos = state.entities.pos(slot);
                if !state.world.is_active(
                    (pos[0].floor() as i32).div_euclid(16),
                    (pos[2].floor() as i32).div_euclid(16),
                ) {
                    continue;
                }
                let record_changed = orders::advance(state, &id, record, slot, revision)
                    | work::advance(state, &id, record, slot, revision);
                if record_changed {
                    save_record(state, &id, record);
                    changed = true;
                }
            }
        }
        if changed {
            state
                .plugins
                .set_domain(&id, "npc/revision", serde_json::Value::from(revision));
        }
        routes::advance(state, &id, &all);
    }
}

#[cfg(test)]
#[path = "plugin_npcs_tests.rs"]
mod tests;

pub(crate) fn standable(state: &TickState, dimension: &str, at: [i32; 3]) -> Option<bool> {
    let state = crate::plugin_inventories::world(state, dimension).ok()?;
    state.world.block_at(at[0], at[1], at[2])?;
    Some(
        crate::settlements::allocator::standing(
            &state.world,
            &state.registry,
            BlockPos {
                x: at[0],
                y: at[1],
                z: at[2],
            },
            0.3,
            1.95,
        )
        .is_some(),
    )
}

pub(crate) fn reachable(
    state: &mut TickState,
    dimension: &str,
    start: [i32; 3],
    goal: [i32; 3],
) -> Result<bool, ScriptOperationFailure> {
    let dimension =
        crate::dimensions::Dimension::parse(dimension).ok_or(ScriptOperationFailure::NotFound)?;
    let world = if dimension == state.dimension {
        &state.world
    } else {
        &state
            .other_dimensions
            .get(&dimension)
            .ok_or(ScriptOperationFailure::Unloaded)?
            .world
    };
    let mut ctx = crate::settlements::allocator::RouteCtx {
        world,
        registry: &state.registry,
        dimension,
        consumer: crate::tick_budget::Consumer::Allocator,
        budget: &mut state.tick_budget,
    };
    let mut cache = crate::settlements::allocator::PoiCache::default();
    let mut path = Vec::new();
    Ok(crate::settlements::allocator::find_path_exact_ctx(
        &mut ctx,
        &mut cache,
        [
            f64::from(start[0]) + 0.5,
            f64::from(start[1]),
            f64::from(start[2]) + 0.5,
        ],
        BlockPos {
            x: goal[0],
            y: goal[1],
            z: goal[2],
        },
        0.3,
        1.95,
        &mut path,
    ))
}

pub(crate) fn deployed_projectile_hit(
    state: &TickState,
    slot: EntitySlot,
    start: [f64; 3],
    end: [f64; 3],
    radius: f64,
) -> Option<Option<f64>> {
    orders::deployment::projectile_hit(state, slot, start, end, radius)
}

pub(crate) fn absorb_deployed_impact(
    state: &mut TickState,
    slot: EntitySlot,
    impact: crate::physical_defense::PhysicalImpact,
) -> bool {
    orders::deployment::absorb_impact(state, slot, impact)
}

/// Only actual committed damage supplies a combat receipt.
pub(crate) fn record_combat(
    state: &mut TickState,
    attacker: EntityId,
    victim_uuid: uuid::Uuid,
    damage: f32,
    killed: bool,
) {
    if !damage.is_finite() || damage <= 0.0 {
        return;
    }
    let Some(slot) = state.entities.slot_of_any(attacker) else {
        return;
    };
    let Some((id, handle)) = custody_for_entity(state, slot) else {
        return;
    };
    let Ok(mut all) = records(state, &id) else {
        crate::plugin::fail_native(state);
        return;
    };
    let Some(record) = all
        .get_mut(&handle)
        .filter(|record| record.disposition == ScriptNpcLifecycle::AliveUnloaded)
    else {
        return;
    };
    let Some(ScriptNpcOrder::Attack { targets, .. }) = &record.controls.order else {
        return;
    };
    let victim_reference = victim_uuid.to_string();
    let Some(reference) = targets
        .iter()
        .find(|target| target.target_ref.starts_with(&victim_reference))
    else {
        return;
    };
    let reference = reference.target_ref.clone();
    let revision = next_revision(state, &id);
    let event_id = state
        .plugins
        .get_domain(NATIVE, "npc/combat-sequence")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1);
    let event = ScriptCombatEvent::new(
        event_id,
        revision,
        record.handle.clone(),
        reference,
        record.controls.order_revision,
        (damage * 1000.0)
            .round()
            .max(1.0)
            .min((MAX_WORK_UNITS * 1000) as f32) as u32,
        killed,
    );
    let Some(mut control) = crate::physical_attacks::npc_combat_control(state, slot)
        .or_else(|| record.controls.combat.take().map(|control| *control))
    else {
        return;
    };
    control.last_event = Some(Box::new(event));
    record.controls.combat = Some(Box::new(control));
    record.controls.revision = revision;
    save_records(state, &id, &all, revision);
    state.plugins.set_domain(
        NATIVE,
        "npc/combat-sequence",
        serde_json::Value::from(event_id),
    );
}

pub(crate) fn validate(state: &TickState) -> Result<(), crate::config::CoreError> {
    let malformed =
        |detail: String| crate::config::CoreError::Storage(format!("native NPC domain: {detail}"));
    let mut custody = BTreeMap::new();
    for id in state.plugins.domain_plugins() {
        let all =
            records(state, id).map_err(|_| malformed(format!("{id}: undecodable NPC records")))?;
        for (handle, record) in all {
            if handle != record.handle
                || handle != record.controls.handle
                || record.revision == 0
                || record.revision > MAX_SCRIPT_WORLD_TIME
                || !matches!(
                    record.disposition,
                    ScriptNpcLifecycle::AliveUnloaded
                        | ScriptNpcLifecycle::Dead
                        | ScriptNpcLifecycle::Released
                )
                || record.formation_size == 0
                || record.formation_slot >= record.formation_size
                || crate::dimensions::Dimension::parse(&record.dimension).is_none()
                || record.entity_uuid.is_nil()
            {
                return Err(malformed(format!("{id}: malformed record {handle}")));
            }
            validate_npc_handle(&handle).map_err(|_| malformed(format!("{id}: invalid handle")))?;
            record
                .controls
                .validate()
                .map_err(|_| malformed(format!("{id}: invalid native controls {handle}")))?;
            if record.disposition != ScriptNpcLifecycle::Released
                && custody
                    .insert(record.entity_uuid.to_string(), (id.to_owned(), handle))
                    .is_some()
            {
                return Err(malformed(format!("{id}: duplicate native custody")));
            }
        }
        routes::validate(state, id).map_err(|_| malformed(format!("{id}: malformed routes")))?;
    }
    let index: BTreeMap<String, (String, String)> = read_domain(state, NATIVE, "npc/custody")
        .map_err(|_| malformed("undecodable native custody index".into()))?;
    if index != custody {
        return Err(malformed(
            "native custody index disagrees with identity records".into(),
        ));
    }
    orders::deployment::validate(state)
        .map_err(|_| malformed("invalid deployed custody".into()))?;
    Ok(())
}

pub(crate) fn clear_segment(
    state: &TickState,
    from: [f64; 3],
    to: [f64; 3],
    allowed: Option<BlockPos>,
) -> bool {
    let min = std::array::from_fn::<_, 3, _>(|axis| from[axis].min(to[axis]).floor() as i32);
    let max = std::array::from_fn::<_, 3, _>(|axis| from[axis].max(to[axis]).floor() as i32);
    for x in min[0]..=max[0] {
        for y in min[1] - 1..=max[1] {
            for z in min[2]..=max[2] {
                let at = BlockPos { x, y, z };
                if allowed == Some(at) {
                    continue;
                }
                let Some(native) = state.world.block_at(x, y, z) else {
                    return false;
                };
                let Some(boxes) = state.registry.collision_boxes.get(native as usize) else {
                    return false;
                };
                for shape in boxes.iter() {
                    let bounds = shape.as_blocks();
                    let origin = [f64::from(x), f64::from(y), f64::from(z)];
                    let mut low = 0.0_f64;
                    let mut high = 1.0_f64;
                    for axis in 0..3 {
                        let start = from[axis];
                        let delta = to[axis] - start;
                        let a = origin[axis] + bounds[axis];
                        let b = origin[axis] + bounds[axis + 3];
                        if delta.abs() < 1.0e-9 {
                            if start <= a + 1.0e-9 || start >= b - 1.0e-9 {
                                low = 2.0;
                                break;
                            }
                        } else {
                            let a = (a - start) / delta;
                            let b = (b - start) / delta;
                            low = low.max(a.min(b));
                            high = high.min(a.max(b));
                        }
                    }
                    if low <= high && high > 1.0e-7 && low < 1.0 - 1.0e-7 {
                        return false;
                    }
                }
            }
        }
    }
    true
}

pub(crate) fn item_use(state: &TickState, slot: EntitySlot) -> Option<ScriptNpcItemUseControl> {
    crate::physical_defense::item_use(state, slot)
}

pub(crate) fn npc_controls(
    state: &TickState,
    slot: EntitySlot,
) -> Option<ScriptNpcControlSnapshot> {
    let (owner, handle) = custodian(state, state.entities.row(slot).uuid)?;
    state
        .plugins
        .get_domain(owner, &format!("{RECORD_PREFIX}{handle}"))?
        .get("controls")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

pub(crate) fn custodian(state: &TickState, uuid: uuid::Uuid) -> Option<(&str, &str)> {
    let binding = state
        .plugins
        .get_domain(NATIVE, "npc/custody")?
        .get(uuid.to_string())?
        .as_array()?;
    Some((binding.first()?.as_str()?, binding.get(1)?.as_str()?))
}

pub(crate) fn physical_attack_permitted(
    state: &TickState,
    actor: EntitySlot,
    target: EntitySlot,
    operation_id: &str,
    order_revision: u64,
) -> bool {
    orders::physical_attack_permitted(state, actor, target, operation_id, order_revision)
}

pub(crate) fn record_death(state: &mut TickState, uuid: uuid::Uuid) {
    death::record(state, uuid)
}

pub(crate) fn custody_for_entity(state: &TickState, slot: EntitySlot) -> Option<(String, String)> {
    let (owner, handle) = custodian(state, state.entities.row(slot).uuid)?;
    Some((owner.to_owned(), handle.to_owned()))
}

pub(crate) fn adjudicate_deployed_projectile(
    state: &mut TickState,
    slot: EntitySlot,
    impact: &crate::physical_defense::PhysicalImpact,
) -> Option<bool> {
    orders::deployment::adjudicate_projectile(state, slot, impact)
}

pub(super) fn physical_slot_loaded(state: &TickState, slot: EntitySlot) -> bool {
    let p = state.entities.pos(slot);
    state.entities.body(slot).alive
        && state
            .world
            .block_at(
                p[0].floor() as i32,
                p[1].floor() as i32,
                p[2].floor() as i32,
            )
            .is_some()
}
