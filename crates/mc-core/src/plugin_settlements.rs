//! Native settlement query/action execution (D006 S1–S4).
//!
//! The customization owner behind `ScriptOperation::SettlementQuery` and
//! `ScriptOperation::Settlement`. Dispatch arms live in `plugin::operation`
//! (owned outside this slice), which also records durable receipts; this
//! module only decides. Reads are open native observations; mutations go
//! through native registry, household, district and physical economic custody,
//! every one fenced by the monotonic record id AND the
//! revision the caller read: after remove/recreate the name and revision can
//! repeat while the id never does, so a stale id refuses (ABA-safe).
//! Like every other family, validation stays in the `mc_script` DTOs.

use mc_script::{
    ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest,
    ScriptSettlementHouseholdSnapshot, ScriptSettlementOperation, ScriptSettlementPage,
    ScriptSettlementQuery, ScriptSettlementResult, ScriptSettlementSnapshot,
};

use crate::settlements::SettlementId;
use crate::tick::TickState;

pub(crate) fn execute(
    state: &mut TickState,
    owner: &str,
    request: &ScriptOperationRequest,
) -> ScriptOperationOutcome {
    use mc_script::ScriptOperation;
    let result = match request.operation() {
        ScriptOperation::SettlementQuery { query } => execute_query(state, query),
        ScriptOperation::Settlement { operation } => execute_action(state, owner, operation),
        _ => Err(ScriptOperationFailure::InvalidRequest),
    };
    result.unwrap_or_else(ScriptOperationOutcome::rejected)
}

fn snapshot(
    state: &TickState,
    record: &crate::settlements::SettlementRecord,
) -> Result<ScriptSettlementSnapshot, ScriptOperationFailure> {
    let member_count =
        u32::try_from(crate::settlements::member_count(state, record.id)).unwrap_or(u32::MAX);
    let mut members: std::collections::BTreeMap<&str, Vec<String>> =
        std::collections::BTreeMap::new();
    for slot in state.entities.slots() {
        let row = state.entities.row(slot);
        if state.entities.body(slot).alive
            && let Some(v) = row.villager.as_ref()
            && v.settlement == Some(record.id)
            && let Some(family) = v.household.as_deref()
        {
            members
                .entry(family)
                .or_default()
                .push(row.uuid.to_string());
        }
    }
    let mut snapshot = ScriptSettlementSnapshot::new(
        record.id.0,
        record.name.clone(),
        record.owner.clone(),
        record.anchor,
        record.radius,
        record.revision,
        member_count,
    );
    snapshot.households = record
        .households
        .values()
        .map(|family| {
            let mut people = members.remove(family.id.as_str()).unwrap_or_default();
            people.sort_unstable();
            ScriptSettlementHouseholdSnapshot::new(
                family.id.clone(),
                family.revision,
                family.origin.as_str().to_owned(),
                family.home,
                family.needs.clone(),
                family.policy.clone(),
                people,
            )
        })
        .collect();
    snapshot.economy = Some(crate::settlements::economy_customization::observe(
        state,
        &record.economy,
    )?);
    Ok(snapshot)
}

fn execute_query(
    state: &mut TickState,
    query: &ScriptSettlementQuery,
) -> Result<ScriptOperationOutcome, ScriptOperationFailure> {
    let mut records: Vec<&crate::settlements::SettlementRecord> = if query.names.is_empty() {
        state.settlements.records().collect()
    } else {
        let mut records = Vec::with_capacity(query.names.len());
        for name in &query.names {
            let Some(record) = state.settlements.find(name) else {
                return Err(ScriptOperationFailure::NotFound);
            };
            records.push(record);
        }
        records
    };
    records.sort_by(|left, right| left.name.cmp(&right.name));
    let start = match &query.cursor {
        None => 0,
        Some(cursor) => records
            .iter()
            .position(|record| record.name.as_str() > cursor.as_str())
            .unwrap_or(records.len()),
    };
    let end = (start + mc_script::MAX_SETTLEMENT_PAGE).min(records.len());
    let cursor = (end < records.len()).then(|| records[end - 1].name.clone());
    // Detach the response and small certificate plan, not a copy of every
    // household, job, receipt and shipment in the authoritative town record.
    let detached = records[start..end]
        .iter()
        .map(|record| Ok((snapshot(state, record)?, record.district.clone())))
        .collect::<Result<Vec<_>, ScriptOperationFailure>>()?;
    drop(records);
    let page = ScriptSettlementPage::new(
        detached
            .into_iter()
            .map(|(mut snapshot, plan)| {
                snapshot.district = crate::settlements::districts::observe(
                    state,
                    SettlementId(snapshot.id),
                    snapshot.anchor,
                    snapshot.radius,
                    plan.as_ref(),
                );
                snapshot
            })
            .collect(),
        cursor,
    );
    page.validate()
        .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    ScriptOperationOutcome::committed(
        state.plugins.revision(),
        ScriptOperationPayload::Settlement {
            result: Box::new(ScriptSettlementResult::Page(page)),
        },
    )
    .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)
}

fn execute_action(
    state: &mut TickState,
    owner: &str,
    operation: &ScriptSettlementOperation,
) -> Result<ScriptOperationOutcome, ScriptOperationFailure> {
    let (id, name) = match operation {
        ScriptSettlementOperation::Bind {
            settlement,
            entity_uuid,
            expected_revision,
            ..
        } => {
            let id = SettlementId(*settlement);
            let revision = state
                .settlements
                .get(id)
                .map(|record| record.revision)
                .ok_or(ScriptOperationFailure::NotFound)?;
            if revision != *expected_revision {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            let uuid = entity_uuid
                .parse::<uuid::Uuid>()
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            let entity = state
                .entities
                .slot_of_uuid(uuid)
                .filter(|slot| state.entities.row(*slot).uuid == uuid)
                .map(|slot| state.entities.body(slot).id)
                .ok_or(ScriptOperationFailure::NotFound)?;
            crate::settlements::bind(state, id, entity).map_err(map_bind_refusal)?;
            let name = state
                .settlements
                .get(id)
                .map(|record| record.name.clone())
                .ok_or(ScriptOperationFailure::RuntimeUnavailable)?;
            (id, name)
        }
        ScriptSettlementOperation::Unbind {
            settlement,
            entity_uuid,
            expected_revision,
            ..
        } => {
            let id = SettlementId(*settlement);
            let uuid = entity_uuid
                .parse::<uuid::Uuid>()
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            let (entity, current) = state
                .entities
                .slot_of_uuid(uuid)
                .and_then(|slot| {
                    let row = state.entities.row(slot);
                    if row.uuid != uuid {
                        return None;
                    }
                    let settlement = row.villager.as_ref()?.settlement;
                    Some((state.entities.body(slot).id, settlement))
                })
                .ok_or(ScriptOperationFailure::NotFound)?;
            // The villager's authoritative column carries the id: a recreated
            // same-name record never matches a stale request.
            if current != Some(id) {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            let (name, revision) = state
                .settlements
                .get(id)
                .map(|record| (record.name.clone(), record.revision))
                .ok_or(ScriptOperationFailure::NotFound)?;
            if revision != *expected_revision {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            crate::settlements::unbind(state, entity)
                .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)?;
            (id, name)
        }
        ScriptSettlementOperation::Rename {
            settlement,
            new_name,
            expected_revision,
            ..
        } => {
            let id = SettlementId(*settlement);
            let revision = state
                .settlements
                .get(id)
                .map(|record| record.revision)
                .ok_or(ScriptOperationFailure::NotFound)?;
            if revision != *expected_revision {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            state
                .settlements
                .rename(id, new_name.clone())
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            (id, new_name.clone())
        }
        ScriptSettlementOperation::FormHousehold {
            settlement,
            entity_uuids,
            expected_revision,
            ..
        } => {
            let id = SettlementId(*settlement);
            let record = state
                .settlements
                .get(id)
                .ok_or(ScriptOperationFailure::NotFound)?;
            if record.revision != *expected_revision {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            let name = record.name.clone();
            let people = entity_uuids
                .iter()
                .map(|uuid| {
                    let uuid = uuid
                        .parse::<uuid::Uuid>()
                        .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
                    state
                        .entities
                        .slot_of_uuid(uuid)
                        .map(|slot| state.entities.body(slot).id)
                        .ok_or(ScriptOperationFailure::NotFound)
                })
                .collect::<Result<Vec<_>, _>>()?;
            crate::settlements::households::form(
                state,
                id,
                &people,
                crate::settlements::households::HouseholdOrigin::Adoption,
            )
            .map_err(map_household_refusal)?;
            (id, name)
        }
        ScriptSettlementOperation::RelocateHousehold {
            settlement,
            expected_revision,
            household,
            destination,
            expected_destination_revision,
            ..
        } => {
            let id = SettlementId(*settlement);
            let destination = SettlementId(*destination);
            let source = state
                .settlements
                .get(id)
                .ok_or(ScriptOperationFailure::NotFound)?;
            let target = state
                .settlements
                .get(destination)
                .ok_or(ScriptOperationFailure::NotFound)?;
            if source.revision != *expected_revision
                || target.revision != *expected_destination_revision
            {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            let name = source.name.clone();
            crate::settlements::households::relocate(state, id, household, destination)
                .map_err(map_household_refusal)?;
            (id, name)
        }
        ScriptSettlementOperation::PlanDistrict {
            settlement,
            expected_revision,
            branch,
            level,
            buildings,
            ..
        } => {
            let id = SettlementId(*settlement);
            crate::settlements::districts::plan(
                state,
                id,
                *expected_revision,
                crate::settlements::districts::DistrictPlan {
                    owner: owner.to_owned(),
                    branch: branch.clone(),
                    level: *level,
                    buildings: buildings.clone(),
                },
            )?;
            let name = state
                .settlements
                .get(id)
                .expect("planned settlement")
                .name
                .clone();
            (id, name)
        }
        ScriptSettlementOperation::Economy {
            settlement,
            expected_revision,
            action,
            ..
        } => {
            let id = SettlementId(*settlement);
            let record = state
                .settlements
                .get(id)
                .ok_or(ScriptOperationFailure::NotFound)?;
            if record.revision != *expected_revision {
                return Err(ScriptOperationFailure::StaleRevision);
            }
            let name = record.name.clone();
            crate::settlements::economy_customization::execute(state, id, action)?;
            (id, name)
        }
        _ => return Err(ScriptOperationFailure::InvalidRequest),
    };
    ScriptOperationOutcome::committed(
        state.plugins.revision(),
        ScriptOperationPayload::Settlement {
            result: Box::new(ScriptSettlementResult::Changed { id: id.0, name }),
        },
    )
    .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)
}

fn map_bind_refusal(message: &'static str) -> ScriptOperationFailure {
    match message {
        "settlement not found" => ScriptOperationFailure::NotFound,
        "villager is outside the settlement" => ScriptOperationFailure::Blocked,
        "villager already belongs to another settlement" => ScriptOperationFailure::Forbidden,
        _ => ScriptOperationFailure::RuntimeUnavailable,
    }
}

fn map_household_refusal(message: &'static str) -> ScriptOperationFailure {
    match message {
        "settlement not found"
        | "destination settlement not found"
        | "household not found"
        | "villager not found" => ScriptOperationFailure::NotFound,
        "household limit reached" | "destination household limit or identity conflict" => {
            ScriptOperationFailure::Capacity
        }
        "villager belongs to another settlement" | "villager already belongs to a household" => {
            ScriptOperationFailure::Forbidden
        }
        _ => ScriptOperationFailure::InvalidRequest,
    }
}
