//! The settlements family of the contract, converted to and from the
//! server's own DTOs.
//!
//! The generated `settlements` interface is a rename of the server's own
//! vocabulary and nothing more: `decode_query`/`decode_action` build exactly
//! the `mc_script::ScriptSettlementQuery`/`ScriptSettlementOperation` the
//! guest described, `encode_result` renames the server's typed result back
//! into the contract's `operation-payload`, and none of them invents a value.
//! Bounds stay the DTO's own: `ScriptOperationRequest::try_new` validates and
//! canonicalizes before any owner sees the operation.

use crate::bindings::exports::solaris::plugin::events as wire_events;
use crate::bindings::solaris::plugin::settlements as wire;
use mc_script::{
    ScriptDtoError, ScriptSettlementBuildingRef, ScriptSettlementEconomyAction,
    ScriptSettlementEconomySnapshot, ScriptSettlementEmploymentSnapshot,
    ScriptSettlementInventoryEndpoint, ScriptSettlementMandateSnapshot, ScriptSettlementOperation,
    ScriptSettlementProductionSnapshot, ScriptSettlementQuery, ScriptSettlementQuoteSnapshot,
    ScriptSettlementResult, ScriptSettlementShipmentSnapshot,
};

/// Largest single string inside one settlement operation, for the host-call
/// text budget. Every member is bounded by the DTO (64-byte ids/names,
/// 36-byte UUIDs, 64-byte building ids, 128-byte purposes); the longest string
/// stands in for the record size.
pub(crate) fn max_text_bytes(value: &wire::SettlementOperation) -> usize {
    match value {
        wire::SettlementOperation::Bind(bind) => {
            bind.operation_id.len().max(bind.entity_uuid.len())
        }
        wire::SettlementOperation::Unbind(unbind) => {
            unbind.operation_id.len().max(unbind.entity_uuid.len())
        }
        wire::SettlementOperation::Rename(rename) => {
            rename.operation_id.len().max(rename.new_name.len())
        }
        wire::SettlementOperation::FormHousehold(form) => form
            .entity_uuids
            .iter()
            .map(String::len)
            .chain(std::iter::once(form.operation_id.len()))
            .max()
            .unwrap_or(0),
        wire::SettlementOperation::RelocateHousehold(movement) => {
            movement.operation_id.len().max(movement.household.len())
        }
        wire::SettlementOperation::PlanDistrict(plan) => plan
            .buildings
            .iter()
            .map(|building| {
                building
                    .kind
                    .len()
                    .max(building.id.len())
                    .max(building.purpose.len())
            })
            .chain(std::iter::once(
                plan.operation_id.len().max(plan.branch.len()),
            ))
            .max()
            .unwrap_or(0),
        wire::SettlementOperation::Economy(economy) => economy
            .operation_id
            .len()
            .max(economy_action_text_bytes(&economy.action)),
    }
}

/// Largest single string inside one typed economic action, for the same host-call
/// text budget. Every member is bounded by the DTO; the longest string stands in
/// for the record size. UUID strings inside carry/equipment/player endpoints and
/// every record ref and reason count, exactly like the inventory endpoints that
/// name a handle by its own string.
fn economy_action_text_bytes(action: &wire::EconomyAction) -> usize {
    fn endpoint_bytes(endpoint: &wire::EconomicInventoryEndpoint) -> usize {
        match endpoint {
            wire::EconomicInventoryEndpoint::Carry(uuid)
            | wire::EconomicInventoryEndpoint::Equipment(uuid)
            | wire::EconomicInventoryEndpoint::Player(uuid) => uuid.len(),
            wire::EconomicInventoryEndpoint::Container(_) => 0,
        }
    }
    fn building_bytes(building: &wire::DistrictBuildingRef) -> usize {
        building
            .kind
            .len()
            .max(building.id.len())
            .max(building.purpose.len())
    }
    match action {
        wire::EconomyAction::ProductionStart(start) => start
            .job
            .len()
            .max(start.worker.len())
            .max(start.recipe.len())
            .max(building_bytes(&start.building)),
        wire::EconomyAction::ProductionPause(job)
        | wire::EconomyAction::ProductionResume(job)
        | wire::EconomyAction::ProductionCancel(job) => job.len(),
        wire::EconomyAction::Employ(employ) => employ
            .contract
            .len()
            .max(employ.worker.len())
            .max(endpoint_bytes(&employ.treasury)),
        wire::EconomyAction::StopEmployment(contract)
        | wire::EconomyAction::DismissEmployment(contract)
        | wire::EconomyAction::Pay(contract) => contract.len(),
        wire::EconomyAction::Quote(quote) => quote
            .id
            .len()
            .max(endpoint_bytes(&quote.seller))
            .max(endpoint_bytes(&quote.buyer)),
        wire::EconomyAction::AcceptQuote(id) | wire::EconomyAction::CancelQuote(id) => id.len(),
        wire::EconomyAction::EscrowReserve(reserve) => reserve
            .id
            .len()
            .max(reserve.shipment.len())
            .max(endpoint_bytes(&reserve.payer))
            .max(endpoint_bytes(&reserve.escrow))
            .max(endpoint_bytes(&reserve.payee)),
        wire::EconomyAction::EscrowPayout(id) => id.len(),
        wire::EconomyAction::CaravanOpen(open) => open
            .id
            .len()
            .max(open.carrier.len())
            .max(building_bytes(&open.source_building))
            .max(building_bytes(&open.destination_building))
            .max(open.paid_mandate.as_ref().map_or(0, String::len)),
        wire::EconomyAction::CaravanPickup(id)
        | wire::EconomyAction::CaravanUnload(id)
        | wire::EconomyAction::CaravanCancel(id) => id.len(),
        wire::EconomyAction::CaravanBlock(block) => block.id.len().max(block.reason.len()),
    }
}

/// Largest single string inside one settlement query, for the same budget.
pub(crate) fn max_query_text_bytes(value: &wire::QuerySettlement) -> usize {
    value
        .names
        .iter()
        .map(String::len)
        .chain(value.cursor.as_ref().map(String::len))
        .max()
        .unwrap_or(0)
}

/// One settlement query, as the server's own DTO.
pub(crate) fn decode_query(
    value: wire::QuerySettlement,
) -> Result<ScriptSettlementQuery, ScriptDtoError> {
    Ok(ScriptSettlementQuery::new(value.names, value.cursor))
}

/// One closed settlement mutation, as the server's own DTO. The wire carries
/// the monotonic record id, never the reusable name.
pub(crate) fn decode_action(
    value: wire::SettlementOperation,
) -> Result<ScriptSettlementOperation, ScriptDtoError> {
    Ok(match value {
        wire::SettlementOperation::Bind(bind) => ScriptSettlementOperation::Bind {
            operation_id: bind.operation_id,
            settlement: bind.settlement,
            entity_uuid: bind.entity_uuid,
            expected_revision: bind.expected_revision,
        },
        wire::SettlementOperation::Unbind(unbind) => ScriptSettlementOperation::Unbind {
            operation_id: unbind.operation_id,
            settlement: unbind.settlement,
            entity_uuid: unbind.entity_uuid,
            expected_revision: unbind.expected_revision,
        },
        wire::SettlementOperation::Rename(rename) => ScriptSettlementOperation::Rename {
            operation_id: rename.operation_id,
            settlement: rename.settlement,
            new_name: rename.new_name,
            expected_revision: rename.expected_revision,
        },
        wire::SettlementOperation::FormHousehold(form) => {
            ScriptSettlementOperation::FormHousehold {
                operation_id: form.operation_id,
                settlement: form.settlement,
                entity_uuids: form.entity_uuids,
                expected_revision: form.expected_revision,
            }
        }
        wire::SettlementOperation::RelocateHousehold(movement) => {
            ScriptSettlementOperation::RelocateHousehold {
                operation_id: movement.operation_id,
                settlement: movement.settlement,
                expected_revision: movement.expected_revision,
                household: movement.household,
                destination: movement.destination,
                expected_destination_revision: movement.expected_destination_revision,
            }
        }
        wire::SettlementOperation::PlanDistrict(plan) => ScriptSettlementOperation::PlanDistrict {
            operation_id: plan.operation_id,
            settlement: plan.settlement,
            expected_revision: plan.expected_revision,
            branch: plan.branch,
            level: plan.level,
            buildings: plan
                .buildings
                .into_iter()
                .map(|building| {
                    ScriptSettlementBuildingRef::new(building.kind, building.id, building.purpose)
                })
                .collect(),
        },
        wire::SettlementOperation::Economy(economy) => ScriptSettlementOperation::Economy {
            operation_id: economy.operation_id,
            settlement: economy.settlement,
            expected_revision: economy.expected_revision,
            action: Box::new(economy_action(economy.action)?),
        },
    })
}

/// One typed economic action, as the server's own DTO. Every endpoint and every
/// position maps losslessly; there are no aliases, defaults, or fallbacks for
/// known values.
fn economy_action(
    value: wire::EconomyAction,
) -> Result<ScriptSettlementEconomyAction, ScriptDtoError> {
    Ok(match value {
        wire::EconomyAction::ProductionStart(start) => {
            ScriptSettlementEconomyAction::ProductionStart {
                job: start.job,
                worker: start.worker,
                recipe: start.recipe,
                count: start.count,
                building: building_ref(start.building),
                station: block_position(start.station),
                stand_at: block_position(start.stand_at),
            }
        }
        wire::EconomyAction::ProductionPause(job) => {
            ScriptSettlementEconomyAction::ProductionPause { job }
        }
        wire::EconomyAction::ProductionResume(job) => {
            ScriptSettlementEconomyAction::ProductionResume { job }
        }
        wire::EconomyAction::ProductionCancel(job) => {
            ScriptSettlementEconomyAction::ProductionCancel { job }
        }
        wire::EconomyAction::Employ(employ) => ScriptSettlementEconomyAction::Employ {
            contract: employ.contract,
            worker: employ.worker,
            treasury: inventory_endpoint(employ.treasury),
            wage: employ.wage,
            period: employ.period,
            budget: employ.budget,
        },
        wire::EconomyAction::StopEmployment(contract) => {
            ScriptSettlementEconomyAction::StopEmployment { contract }
        }
        wire::EconomyAction::DismissEmployment(contract) => {
            ScriptSettlementEconomyAction::DismissEmployment { contract }
        }
        wire::EconomyAction::Pay(contract) => ScriptSettlementEconomyAction::Pay { contract },
        wire::EconomyAction::Quote(quote) => ScriptSettlementEconomyAction::Quote {
            id: quote.id,
            seller: inventory_endpoint(quote.seller),
            buyer: inventory_endpoint(quote.buyer),
            source_slot: quote.source_slot,
            quantity: quote.quantity,
            price: quote.price,
        },
        wire::EconomyAction::AcceptQuote(id) => ScriptSettlementEconomyAction::AcceptQuote { id },
        wire::EconomyAction::CancelQuote(id) => ScriptSettlementEconomyAction::CancelQuote { id },
        wire::EconomyAction::EscrowReserve(reserve) => {
            ScriptSettlementEconomyAction::EscrowReserve {
                id: reserve.id,
                shipment: reserve.shipment,
                payer: inventory_endpoint(reserve.payer),
                escrow: inventory_endpoint(reserve.escrow),
                payee: inventory_endpoint(reserve.payee),
                price: reserve.price,
            }
        }
        wire::EconomyAction::EscrowPayout(id) => ScriptSettlementEconomyAction::EscrowPayout { id },
        wire::EconomyAction::CaravanOpen(open) => ScriptSettlementEconomyAction::CaravanOpen {
            id: open.id,
            carrier: open.carrier,
            source_building: building_ref(open.source_building),
            source_position: block_position(open.source_position),
            destination_building: building_ref(open.destination_building),
            destination_position: block_position(open.destination_position),
            source_slot: open.source_slot,
            quantity: open.quantity,
            waypoints: open.waypoints.into_iter().map(block_position).collect(),
            max_ticks: open.max_ticks,
            paid_mandate: open.paid_mandate,
        },
        wire::EconomyAction::CaravanPickup(id) => {
            ScriptSettlementEconomyAction::CaravanPickup { id }
        }
        wire::EconomyAction::CaravanUnload(id) => {
            ScriptSettlementEconomyAction::CaravanUnload { id }
        }
        wire::EconomyAction::CaravanCancel(id) => {
            ScriptSettlementEconomyAction::CaravanCancel { id }
        }
        wire::EconomyAction::CaravanBlock(block) => ScriptSettlementEconomyAction::CaravanBlock {
            id: block.id,
            reason: block.reason,
        },
    })
}

/// One wire building certificate as the server's own DTO.
fn building_ref(value: wire::DistrictBuildingRef) -> ScriptSettlementBuildingRef {
    ScriptSettlementBuildingRef::new(value.kind, value.id, value.purpose)
}

/// One wire economic endpoint as the server's own DTO. Container positions are
/// exact native coordinates; the server infers the current dimension.
fn inventory_endpoint(value: wire::EconomicInventoryEndpoint) -> ScriptSettlementInventoryEndpoint {
    match value {
        wire::EconomicInventoryEndpoint::Carry(uuid) => {
            ScriptSettlementInventoryEndpoint::Carry { uuid }
        }
        wire::EconomicInventoryEndpoint::Equipment(uuid) => {
            ScriptSettlementInventoryEndpoint::Equipment { uuid }
        }
        wire::EconomicInventoryEndpoint::Container(position) => {
            ScriptSettlementInventoryEndpoint::Container {
                position: block_position(position),
            }
        }
        wire::EconomicInventoryEndpoint::Player(uuid) => {
            ScriptSettlementInventoryEndpoint::Player { uuid }
        }
    }
}

/// One wire block coordinate triple.
fn block_position(value: wire::BlockPosition) -> [i32; 3] {
    [value.x, value.y, value.z]
}

/// One triple as the wire's block coordinate record.
fn wire_block_position(value: [i32; 3]) -> wire::BlockPosition {
    wire::BlockPosition {
        x: value[0],
        y: value[1],
        z: value[2],
    }
}

/// One server settlement result, renamed into the contract's
/// `operation-payload`. A future server variant the contract cannot express
/// answers `None` and the answer is dropped rather than delivered with an
/// invented shape.
pub(crate) fn encode_result(
    result: &ScriptSettlementResult,
) -> Option<wire_events::OperationPayload> {
    match result {
        ScriptSettlementResult::Page(page) => Some(wire_events::OperationPayload::SettlementPage(
            wire::SettlementPage {
                settlements: page
                    .settlements
                    .iter()
                    .map(|snapshot| wire::SettlementSnapshot {
                        id: snapshot.id,
                        name: snapshot.name.clone(),
                        owner: snapshot.owner.clone(),
                        anchor_x: snapshot.anchor[0],
                        anchor_y: snapshot.anchor[1],
                        anchor_z: snapshot.anchor[2],
                        radius: snapshot.radius,
                        revision: snapshot.revision,
                        member_count: snapshot.member_count,
                        households: snapshot
                            .households
                            .iter()
                            .map(|household| wire::HouseholdSnapshot {
                                id: household.id.clone(),
                                revision: household.revision,
                                origin: household.origin.clone(),
                                home_x: household.home.map(|home| home[0]),
                                home_y: household.home.map(|home| home[1]),
                                home_z: household.home.map(|home| home[2]),
                                needs: household.needs.clone(),
                                policy: household.policy.clone(),
                                members: household.members.clone(),
                            })
                            .collect(),
                        district: snapshot.district.as_ref().map(|district| {
                            wire::DistrictSnapshot {
                                requested_branch: district.requested_branch.clone(),
                                requested_level: district.requested_level,
                                stage: district.stage.clone(),
                                branch: district.branch.clone(),
                                level: district.level,
                                population: district.population,
                                housed: district.housed,
                                food: district.food,
                                working: district.working.clone(),
                                reason: district.reason.clone(),
                                observed_tick: district.observed_tick,
                                world_revision: district.world_revision,
                                buildings: district
                                    .buildings
                                    .iter()
                                    .map(|building| wire::DistrictBuildingRef {
                                        kind: building.kind.clone(),
                                        id: building.id.clone(),
                                        purpose: building.purpose.clone(),
                                    })
                                    .collect(),
                            }
                        }),
                        economy: snapshot.economy.as_ref().and_then(economy_snapshot),
                    })
                    .collect(),
                cursor: page.cursor.clone(),
            },
        )),
        ScriptSettlementResult::Changed { id, name } => Some(
            wire_events::OperationPayload::SettlementMutation(wire::SettlementMutation {
                id: *id,
                name: name.clone(),
            }),
        ),
        _ => None,
    }
}

/// Encode only economic observations whose endpoints the wire can represent.
fn economy_snapshot(value: &ScriptSettlementEconomySnapshot) -> Option<wire::EconomySnapshot> {
    Some(wire::EconomySnapshot {
        production: value.production.iter().map(production_snapshot).collect(),
        employments: value
            .employments
            .iter()
            .map(employment_snapshot)
            .collect::<Option<Vec<_>>>()?,
        quotes: value
            .quotes
            .iter()
            .map(quote_snapshot)
            .collect::<Option<Vec<_>>>()?,
        mandates: value
            .mandates
            .iter()
            .map(mandate_snapshot)
            .collect::<Option<Vec<_>>>()?,
        shipments: value
            .shipments
            .iter()
            .map(shipment_snapshot)
            .collect::<Option<Vec<_>>>()?,
        ended_employment: value.ended_employment,
        completed_quote: value.completed_quote,
        settled_mandate: value.settled_mandate,
    })
}

/// One server production row as the contract names it.
fn production_snapshot(value: &ScriptSettlementProductionSnapshot) -> wire::ProductionSnapshot {
    wire::ProductionSnapshot {
        id: value.id.clone(),
        worker: value.worker.clone(),
        recipe: value.recipe.clone(),
        recipe_revision: value.recipe_revision.clone(),
        requested: value.requested,
        completed: value.completed,
        paused: value.paused.clone(),
        cancelled: value.cancelled,
        next_due: value.next_due,
    }
}

/// One server employment row as the contract names it.
fn employment_snapshot(
    value: &ScriptSettlementEmploymentSnapshot,
) -> Option<wire::EmploymentSnapshot> {
    Some(wire::EmploymentSnapshot {
        id: value.id.clone(),
        worker: value.worker.clone(),
        treasury: wire_endpoint(&value.treasury)?,
        wage: value.wage,
        period: value.period,
        budget: value.budget,
        earned: value.earned,
        paid: value.paid,
        active: value.active,
        next_due: value.next_due,
    })
}

/// One server quote row as the contract names it. The item summary is a
/// read-only contract summary, not a storage mirror.
fn quote_snapshot(value: &ScriptSettlementQuoteSnapshot) -> Option<wire::QuoteSnapshot> {
    Some(wire::QuoteSnapshot {
        id: value.id.clone(),
        seller: wire_endpoint(&value.seller)?,
        buyer: wire_endpoint(&value.buyer)?,
        item_id: value.item_id.clone(),
        quantity: value.quantity,
        price: value.price,
    })
}

/// One server escrow mandate row as the contract names it.
fn mandate_snapshot(value: &ScriptSettlementMandateSnapshot) -> Option<wire::MandateSnapshot> {
    Some(wire::MandateSnapshot {
        id: value.id.clone(),
        shipment: value.shipment.clone(),
        payer: wire_endpoint(&value.payer)?,
        escrow: wire_endpoint(&value.escrow)?,
        payee: wire_endpoint(&value.payee)?,
        price: value.price,
        status: value.status.clone(),
    })
}

/// One server caravan shipment row as the contract names it.
fn shipment_snapshot(value: &ScriptSettlementShipmentSnapshot) -> Option<wire::ShipmentSnapshot> {
    Some(wire::ShipmentSnapshot {
        id: value.id.clone(),
        carrier: value.carrier.clone(),
        source: wire_endpoint(&value.source)?,
        destination: wire_endpoint(&value.destination)?,
        item_id: value.item_id.clone(),
        quantity: value.quantity,
        stage: value.stage.clone(),
        waypoint: value.waypoint,
        waypoints: value
            .waypoints
            .iter()
            .copied()
            .map(wire_block_position)
            .collect(),
        blocked: value.blocked.clone(),
        paid_mandate: value.paid_mandate.clone(),
    })
}

/// Unsupported future server endpoints have no wire representation.
fn wire_endpoint(
    value: &ScriptSettlementInventoryEndpoint,
) -> Option<wire::EconomicInventoryEndpoint> {
    Some(match value {
        ScriptSettlementInventoryEndpoint::Carry { uuid } => {
            wire::EconomicInventoryEndpoint::Carry(uuid.clone())
        }
        ScriptSettlementInventoryEndpoint::Equipment { uuid } => {
            wire::EconomicInventoryEndpoint::Equipment(uuid.clone())
        }
        ScriptSettlementInventoryEndpoint::Container { position } => {
            wire::EconomicInventoryEndpoint::Container(wire_block_position(*position))
        }
        ScriptSettlementInventoryEndpoint::Player { uuid } => {
            wire::EconomicInventoryEndpoint::Player(uuid.clone())
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_id_fenced_action_fields() {
        let action = decode_action(wire::SettlementOperation::Bind(wire::BindSettlement {
            operation_id: "op-1".into(),
            settlement: 7,
            entity_uuid: "12345678-1234-1234-1234-123456789012".into(),
            expected_revision: 3,
        }))
        .unwrap();
        assert_eq!(action.operation_id(), "op-1");
        action.validate().unwrap();
        let query = decode_query(wire::QuerySettlement {
            names: vec!["town".into()],
            cursor: None,
        })
        .unwrap();
        query.validate().unwrap();
        let snapshot = mc_script::ScriptSettlementSnapshot::new(
            7,
            "town".into(),
            "owner".into(),
            [0, 64, 0],
            48,
            1,
            2,
        );
        snapshot.validate().unwrap();
    }

    #[test]
    fn households_roundtrip_through_real_wire_conversions() {
        let person = "00000000-0000-0000-0000-000000000001".to_owned();
        let form = wire::SettlementOperation::FormHousehold(wire::FormHousehold {
            operation_id: "form".into(),
            settlement: 7,
            entity_uuids: vec![person.clone()],
            expected_revision: 3,
        });
        assert_eq!(max_text_bytes(&form), person.len());
        let form = decode_action(form).unwrap();
        form.validate().unwrap();
        assert!(matches!(
            form,
            ScriptSettlementOperation::FormHousehold { settlement: 7, .. }
        ));
        let movement = decode_action(wire::SettlementOperation::RelocateHousehold(
            wire::RelocateHousehold {
                operation_id: "move".into(),
                settlement: 7,
                expected_revision: 3,
                household: "7-1".into(),
                destination: 8,
                expected_destination_revision: 4,
            },
        ))
        .unwrap();
        movement.validate().unwrap();
        let mut snapshot = mc_script::ScriptSettlementSnapshot::new(
            7,
            "town".into(),
            "owner".into(),
            [0, 64, 0],
            48,
            3,
            1,
        );
        snapshot
            .households
            .push(mc_script::ScriptSettlementHouseholdSnapshot::new(
                "7-1".into(),
                2,
                "adoption".into(),
                Some([1, 64, 2]),
                Some("needs.basic".into()),
                Some("policy.local".into()),
                vec![person.clone()],
            ));
        snapshot.validate().unwrap();
        let result = ScriptSettlementResult::Page(mc_script::ScriptSettlementPage::new(
            vec![snapshot],
            None,
        ));
        let Some(wire_events::OperationPayload::SettlementPage(page)) = encode_result(&result)
        else {
            panic!("native household result converts to settlement page");
        };
        let family = &page.settlements[0].households[0];
        assert_eq!(family.id, "7-1");
        assert_eq!(
            (family.home_x, family.home_y, family.home_z),
            (Some(1), Some(64), Some(2))
        );
        assert_eq!(family.members, vec![person]);
        assert_eq!(family.needs.as_deref(), Some("needs.basic"));
        assert_eq!(family.policy.as_deref(), Some("policy.local"));
    }

    fn district_wire_ref(purpose: &str) -> wire::DistrictBuildingRef {
        wire::DistrictBuildingRef {
            kind: "structure".into(),
            id: "granary-one".into(),
            purpose: purpose.into(),
        }
    }

    #[test]
    fn district_plan_roundtrip_through_real_wire_conversions() {
        let plan = wire::SettlementOperation::PlanDistrict(wire::PlanDistrict {
            operation_id: "plan".into(),
            settlement: 7,
            expected_revision: 3,
            branch: "estate".into(),
            level: 1,
            buildings: vec![district_wire_ref("solaris:warehouse")],
        });
        assert_eq!(max_text_bytes(&plan), "solaris:warehouse".len());
        let plan = decode_action(plan).unwrap();
        plan.validate().unwrap();
        assert!(matches!(
            plan,
            ScriptSettlementOperation::PlanDistrict { settlement: 7, .. }
        ));
        let mut observation = mc_script::ScriptSettlementDistrictSnapshot::new(
            "estate".into(),
            1,
            vec![mc_script::ScriptSettlementBuildingRef::new(
                "structure".into(),
                "granary-one".into(),
                "solaris:warehouse".into(),
            )],
        );
        observation.stage = Some("hamlet".into());
        observation.branch = "estate".into();
        observation.level = 1;
        observation.population = 30;
        observation.housed = 24;
        observation.food = 40;
        observation.working = vec!["solaris:warehouse".into()];
        observation.observed_tick = 7;
        observation.world_revision = 9;
        observation.validate().unwrap();
        let mut snapshot = mc_script::ScriptSettlementSnapshot::new(
            7,
            "town".into(),
            "owner".into(),
            [0, 64, 0],
            48,
            3,
            30,
        );
        snapshot.district = Some(observation);
        snapshot.validate().unwrap();
        let result = ScriptSettlementResult::Page(mc_script::ScriptSettlementPage::new(
            vec![snapshot],
            None,
        ));
        let Some(wire_events::OperationPayload::SettlementPage(page)) = encode_result(&result)
        else {
            panic!("native district result converts to settlement page");
        };
        let district = page.settlements[0]
            .district
            .as_ref()
            .expect("district converts");
        assert_eq!(district.requested_branch, "estate");
        assert_eq!(district.requested_level, 1);
        assert_eq!(district.stage.as_deref(), Some("hamlet"));
        assert_eq!(district.branch, "estate");
        assert_eq!(district.level, 1);
        assert_eq!(
            (district.population, district.housed, district.food),
            (30, 24, 40)
        );
        assert_eq!(district.working, vec!["solaris:warehouse".to_owned()]);
        assert_eq!((district.observed_tick, district.world_revision), (7, 9));
        assert_eq!(district.buildings.len(), 1);
        assert_eq!(district.buildings[0].kind, "structure");
        assert_eq!(district.buildings[0].id, "granary-one");
        assert_eq!(district.buildings[0].purpose, "solaris:warehouse");
    }

    fn economy_envelope(action: wire::EconomyAction) -> wire::SettlementOperation {
        wire::SettlementOperation::Economy(wire::EconomySettlement {
            operation_id: "eco-1".into(),
            settlement: 7,
            expected_revision: 3,
            action,
        })
    }

    fn economy_page(economy: ScriptSettlementEconomySnapshot) -> wire::SettlementPage {
        let mut snapshot = mc_script::ScriptSettlementSnapshot::new(
            7,
            "town".into(),
            "owner".into(),
            [0, 64, 0],
            48,
            3,
            1,
        );
        snapshot.economy = Some(economy);
        snapshot.validate().unwrap();
        let result = ScriptSettlementResult::Page(mc_script::ScriptSettlementPage::new(
            vec![snapshot],
            None,
        ));
        let Some(wire_events::OperationPayload::SettlementPage(page)) = encode_result(&result)
        else {
            panic!("native economy result converts to settlement page");
        };
        page
    }

    #[test]
    fn production_start_roundtrip_preserves_real_fields() {
        let action = decode_action(economy_envelope(wire::EconomyAction::ProductionStart(
            wire::ProductionStart {
                job: "bake-bread".into(),
                worker: "12345678-1234-1234-1234-123456789012".into(),
                recipe: "minecraft:bread".into(),
                count: 4,
                building: wire::DistrictBuildingRef {
                    kind: "worksite".into(),
                    id: "bakery-one".into(),
                    purpose: "minecraft:bread".into(),
                },
                station: wire::BlockPosition { x: 1, y: 64, z: 2 },
                stand_at: wire::BlockPosition { x: 3, y: 64, z: 4 },
            },
        )))
        .unwrap();
        assert_eq!(action.operation_id(), "eco-1");
        action.validate().unwrap();
        let ScriptSettlementOperation::Economy { action, .. } = action else {
            panic!("production start decodes to economy arm");
        };
        assert!(matches!(
            *action,
            ScriptSettlementEconomyAction::ProductionStart {
                count: 4,
                station: [1, 64, 2],
                stand_at: [3, 64, 4],
                ..
            }
        ));
        for id in ["pause", "resume", "cancel"] {
            let action = match id {
                "pause" => wire::EconomyAction::ProductionPause("bake-bread".into()),
                "resume" => wire::EconomyAction::ProductionResume("bake-bread".into()),
                _ => wire::EconomyAction::ProductionCancel("bake-bread".into()),
            };
            let action = decode_action(economy_envelope(action)).unwrap();
            action.validate().unwrap();
        }
    }

    #[test]
    fn quote_cancel_roundtrip_preserves_endpoints_and_slots() {
        let seller =
            wire::EconomicInventoryEndpoint::Carry("12345678-1234-1234-1234-123456789012".into());
        let buyer =
            wire::EconomicInventoryEndpoint::Container(wire::BlockPosition { x: 5, y: 64, z: 6 });
        let quote = wire::Quote {
            id: "11".into(),
            seller,
            buyer,
            source_slot: 3,
            quantity: 64,
            price: 7,
        };
        let quoted = quote.clone();
        let action = decode_action(economy_envelope(wire::EconomyAction::Quote(quote))).unwrap();
        action.validate().unwrap();
        let ScriptSettlementOperation::Economy { action, .. } = action else {
            panic!("quote decodes to economy arm");
        };
        assert!(matches!(
            *action,
            ScriptSettlementEconomyAction::Quote {
                source_slot: 3,
                quantity: 64,
                price: 7,
                ..
            }
        ));
        assert_eq!(
            max_text_bytes(&economy_envelope(wire::EconomyAction::Quote(quoted))),
            "12345678-1234-1234-1234-123456789012".len()
        );
        for (variant, id) in [
            (
                wire::EconomyAction::AcceptQuote("11".into()),
                "accept decodes",
            ),
            (
                wire::EconomyAction::CancelQuote("11".into()),
                "cancel decodes",
            ),
        ] {
            let action = decode_action(economy_envelope(variant)).unwrap();
            action.validate().unwrap();
            assert!(
                matches!(action, ScriptSettlementOperation::Economy { .. }),
                "{id}"
            );
        }
        let bad = decode_action(economy_envelope(wire::EconomyAction::CancelQuote(
            "0".into(),
        )))
        .unwrap();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn caravan_open_roundtrip_preserves_route_and_mandate() {
        let waypoints = vec![
            wire::BlockPosition { x: 0, y: 64, z: 0 },
            wire::BlockPosition {
                x: 100,
                y: 64,
                z: 0,
            },
            wire::BlockPosition {
                x: 100,
                y: 64,
                z: 100,
            },
        ];
        let open = wire::CaravanOpen {
            id: "caravan-one".into(),
            carrier: "12345678-1234-1234-1234-123456789012".into(),
            source_building: wire::DistrictBuildingRef {
                kind: "structure".into(),
                id: "granary-one".into(),
                purpose: "solaris:warehouse".into(),
            },
            source_position: wire::BlockPosition { x: 0, y: 64, z: 0 },
            destination_building: wire::DistrictBuildingRef {
                kind: "structure".into(),
                id: "granary-two".into(),
                purpose: "solaris:warehouse".into(),
            },
            destination_position: wire::BlockPosition {
                x: 100,
                y: 64,
                z: 100,
            },
            source_slot: 9,
            quantity: 32,
            waypoints,
            max_ticks: 6000,
            paid_mandate: Some("21".into()),
        };
        let action =
            decode_action(economy_envelope(wire::EconomyAction::CaravanOpen(open))).unwrap();
        action.validate().unwrap();
        let ScriptSettlementOperation::Economy { action, .. } = action else {
            panic!("caravan open decodes to economy arm");
        };
        let ScriptSettlementEconomyAction::CaravanOpen {
            source_slot,
            quantity,
            waypoints,
            max_ticks,
            paid_mandate,
            ..
        } = *action
        else {
            panic!("caravan open maps losslessly");
        };
        assert_eq!((source_slot, quantity, max_ticks), (9, 32, 6000));
        assert_eq!(waypoints, vec![[0, 64, 0], [100, 64, 0], [100, 64, 100]]);
        assert_eq!(paid_mandate.as_deref(), Some("21"));
        for (variant, label) in [
            (
                wire::EconomyAction::CaravanPickup("caravan-one".into()),
                "pickup decodes",
            ),
            (
                wire::EconomyAction::CaravanUnload("caravan-one".into()),
                "unload decodes",
            ),
            (
                wire::EconomyAction::CaravanCancel("caravan-one".into()),
                "cancel decodes",
            ),
            (
                wire::EconomyAction::CaravanBlock(wire::CaravanBlock {
                    id: "caravan-one".into(),
                    reason: "bridge out".into(),
                }),
                "block decodes",
            ),
        ] {
            let action = decode_action(economy_envelope(variant)).unwrap();
            action.validate().unwrap();
            assert!(
                matches!(action, ScriptSettlementOperation::Economy { .. }),
                "{label}"
            );
        }
    }

    #[test]
    fn employment_and_escrow_roundtrip_preserves_treasuries() {
        let treasury =
            wire::EconomicInventoryEndpoint::Carry("12345678-1234-1234-1234-123456789012".into());
        let action = decode_action(economy_envelope(wire::EconomyAction::Employ(
            wire::Employ {
                contract: "12".into(),
                worker: "12345678-1234-1234-1234-123456789013".into(),
                treasury,
                wage: 5,
                period: 24000,
                budget: 500,
            },
        )))
        .unwrap();
        action.validate().unwrap();
        for variant in [
            wire::EconomyAction::StopEmployment("12".into()),
            wire::EconomyAction::DismissEmployment("12".into()),
            wire::EconomyAction::Pay("12".into()),
        ] {
            decode_action(economy_envelope(variant))
                .unwrap()
                .validate()
                .unwrap();
        }
        let escrow =
            wire::EconomicInventoryEndpoint::Container(wire::BlockPosition { x: 7, y: 64, z: 8 });
        let action = decode_action(economy_envelope(wire::EconomyAction::EscrowReserve(
            wire::EscrowReserve {
                id: "13".into(),
                shipment: "caravan-one".into(),
                payer: wire::EconomicInventoryEndpoint::Carry(
                    "12345678-1234-1234-1234-123456789012".into(),
                ),
                escrow,
                payee: wire::EconomicInventoryEndpoint::Player(
                    "12345678-1234-1234-1234-123456789014".into(),
                ),
                price: 40,
            },
        )))
        .unwrap();
        action.validate().unwrap();
        decode_action(economy_envelope(wire::EconomyAction::EscrowPayout(
            "13".into(),
        )))
        .unwrap()
        .validate()
        .unwrap();
    }

    #[test]
    fn economy_snapshot_roundtrip_maps_every_field_and_fence() {
        let worker = "12345678-1234-1234-1234-123456789012".to_owned();
        let mut production = ScriptSettlementProductionSnapshot::new(
            "bake-one".into(),
            worker.clone(),
            "minecraft:bread".into(),
            "rev-9".into(),
            4,
        );
        production.completed = 2;
        production.paused = Some("paused".into());
        production.next_due = 900;
        let mut employment = ScriptSettlementEmploymentSnapshot::new(
            "12".into(),
            worker.clone(),
            ScriptSettlementInventoryEndpoint::Carry {
                uuid: worker.clone(),
            },
            5,
            24000,
            500,
        );
        employment.earned = 15;
        employment.paid = 10;
        employment.next_due = 1200;
        let quote = ScriptSettlementQuoteSnapshot::new(
            "11".into(),
            ScriptSettlementInventoryEndpoint::Carry {
                uuid: worker.clone(),
            },
            ScriptSettlementInventoryEndpoint::Container {
                position: [5, 64, 6],
            },
            "minecraft:wheat".into(),
            64,
            7,
        );
        let mut mandate = ScriptSettlementMandateSnapshot::new(
            "13".into(),
            "caravan-one".into(),
            ScriptSettlementInventoryEndpoint::Carry {
                uuid: worker.clone(),
            },
            ScriptSettlementInventoryEndpoint::Container {
                position: [7, 64, 8],
            },
            ScriptSettlementInventoryEndpoint::Player {
                uuid: worker.clone(),
            },
            40,
        );
        mandate.status = "lost".into();
        let mut shipment = ScriptSettlementShipmentSnapshot::new(
            "caravan-one".into(),
            worker.clone(),
            ScriptSettlementInventoryEndpoint::Container {
                position: [0, 64, 0],
            },
            ScriptSettlementInventoryEndpoint::Container {
                position: [100, 64, 100],
            },
            "minecraft:wheat".into(),
            32,
        );
        shipment.stage = "blocked".into();
        shipment.waypoint = 1;
        shipment.waypoints = vec![[0, 64, 0], [100, 64, 0], [100, 64, 100]];
        shipment.blocked = Some("bridge out".into());
        shipment.paid_mandate = Some("13".into());
        let mut economy = ScriptSettlementEconomySnapshot::new();
        economy.production.push(production);
        economy.employments.push(employment);
        economy.quotes.push(quote);
        economy.mandates.push(mandate);
        economy.shipments.push(shipment);
        economy.ended_employment = 2;
        economy.completed_quote = 11;
        economy.settled_mandate = 13;
        economy.validate().unwrap();
        let page = economy_page(economy);
        let economy = page.settlements[0]
            .economy
            .as_ref()
            .expect("economy converts");
        let production = &economy.production[0];
        assert_eq!(production.id, "bake-one");
        assert_eq!(production.worker, worker);
        assert_eq!(production.recipe, "minecraft:bread");
        assert_eq!(production.recipe_revision, "rev-9");
        assert_eq!((production.requested, production.completed), (4, 2));
        assert_eq!(production.paused.as_deref(), Some("paused"));
        assert!(!production.cancelled);
        assert_eq!(production.next_due, 900);
        let employment = &economy.employments[0];
        assert_eq!(employment.id, "12");
        assert_eq!(
            (employment.wage, employment.period, employment.budget),
            (5, 24000, 500)
        );
        assert_eq!((employment.earned, employment.paid), (15, 10));
        assert!(employment.active);
        assert_eq!(employment.next_due, 1200);
        let quote = &economy.quotes[0];
        assert_eq!(
            (quote.item_id.as_str(), quote.quantity, quote.price),
            ("minecraft:wheat", 64, 7)
        );
        let mandate = &economy.mandates[0];
        assert_eq!(
            (mandate.shipment.as_str(), mandate.status.as_str()),
            ("caravan-one", "lost")
        );
        assert_eq!(mandate.price, 40);
        let shipment = &economy.shipments[0];
        assert_eq!(shipment.carrier, worker);
        assert_eq!(shipment.item_id, "minecraft:wheat");
        assert_eq!(shipment.quantity, 32);
        assert_eq!(shipment.stage, "blocked");
        assert_eq!(shipment.waypoint, 1);
        assert!(
            shipment
                .waypoints
                .iter()
                .map(|point| [point.x, point.y, point.z])
                .eq([[0, 64, 0], [100, 64, 0], [100, 64, 100]])
        );
        assert_eq!(shipment.blocked.as_deref(), Some("bridge out"));
        assert_eq!(shipment.paid_mandate.as_deref(), Some("13"));
        assert_eq!(
            (
                economy.ended_employment,
                economy.completed_quote,
                economy.settled_mandate
            ),
            (2, 11, 13)
        );
    }

    #[test]
    fn every_known_economic_endpoint_roundtrips_without_fallback() {
        let uuid = "12345678-1234-1234-1234-123456789012".to_owned();
        for endpoint in [
            ScriptSettlementInventoryEndpoint::Carry { uuid: uuid.clone() },
            ScriptSettlementInventoryEndpoint::Equipment { uuid: uuid.clone() },
            ScriptSettlementInventoryEndpoint::Player { uuid },
            ScriptSettlementInventoryEndpoint::Container {
                position: [4, 64, 9],
            },
        ] {
            let wire = wire_endpoint(&endpoint).expect("known endpoint is representable");
            assert_eq!(inventory_endpoint(wire), endpoint);
        }
    }

    #[test]
    fn malformed_economy_shapes_fail_dto_validation() {
        let bad_quote = decode_action(economy_envelope(wire::EconomyAction::Quote(wire::Quote {
            id: "0".into(),
            seller: wire::EconomicInventoryEndpoint::Carry("not-a-uuid".into()),
            buyer: wire::EconomicInventoryEndpoint::Carry(
                "12345678-1234-1234-1234-123456789012".into(),
            ),
            source_slot: 0,
            quantity: 0,
            price: 0,
        })))
        .unwrap();
        assert!(bad_quote.validate().is_err());
        let bad_route = decode_action(economy_envelope(wire::EconomyAction::CaravanOpen(
            wire::CaravanOpen {
                id: "caravan-one".into(),
                carrier: "12345678-1234-1234-1234-123456789012".into(),
                source_building: wire::DistrictBuildingRef {
                    kind: "structure".into(),
                    id: "granary-one".into(),
                    purpose: "solaris:warehouse".into(),
                },
                source_position: wire::BlockPosition { x: 0, y: 64, z: 0 },
                destination_building: wire::DistrictBuildingRef {
                    kind: "structure".into(),
                    id: "granary-two".into(),
                    purpose: "solaris:warehouse".into(),
                },
                destination_position: wire::BlockPosition {
                    x: 100,
                    y: 64,
                    z: 100,
                },
                source_slot: 9,
                quantity: 32,
                waypoints: vec![wire::BlockPosition { x: 0, y: 64, z: 0 }],
                max_ticks: 0,
                paid_mandate: None,
            },
        )))
        .unwrap();
        assert!(bad_route.validate().is_err());
    }
}
