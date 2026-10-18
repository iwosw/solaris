//! Owner-authorized tokens for the same typed native actions plugins request.
use mc_script::{
    ScriptOperationFailure as Failure, ScriptSettlementBuildingRef,
    ScriptSettlementEconomyAction as Action, ScriptSettlementInventoryEndpoint as Endpoint,
};

use super::commands::EconomyDomain;
use super::{SettlementId, economy_customization};
use crate::tick::TickState;

fn number<T: std::str::FromStr>(value: &str) -> Result<T, Failure> {
    value.parse().map_err(|_| Failure::InvalidRequest)
}

fn position(value: &str) -> Result<[i32; 3], Failure> {
    let mut parts = value.split(',');
    let mut result = [0; 3];
    for coordinate in &mut result {
        *coordinate = number(parts.next().ok_or(Failure::InvalidRequest)?)?;
    }
    if parts.next().is_some() {
        return Err(Failure::InvalidRequest);
    }
    Ok(result)
}

fn endpoint(token: &str) -> Result<Endpoint, Failure> {
    let (kind, identity) = token.split_once(':').ok_or(Failure::InvalidRequest)?;
    match kind {
        "carry" => Ok(Endpoint::Carry {
            uuid: identity.to_owned(),
        }),
        "equipment" => Ok(Endpoint::Equipment {
            uuid: identity.to_owned(),
        }),
        "player" => Ok(Endpoint::Player {
            uuid: identity.to_owned(),
        }),
        "container" => Ok(Endpoint::Container {
            position: position(identity)?,
        }),
        _ => Err(Failure::InvalidRequest),
    }
}

fn building(
    world: &TickState,
    id: SettlementId,
    token: &str,
) -> Result<ScriptSettlementBuildingRef, Failure> {
    world
        .settlements
        .get(id)
        .and_then(|record| record.district.as_ref())
        .and_then(|plan| {
            plan.buildings
                .iter()
                .find(|reference| reference.id == token)
        })
        .cloned()
        .ok_or(Failure::NotFound)
}

fn route(value: &str) -> Result<Vec<[i32; 3]>, Failure> {
    let mut route = Vec::new();
    for waypoint in value.split(';') {
        if route.len() == 16 {
            return Err(Failure::InvalidRequest);
        }
        route.push(position(waypoint)?);
    }
    Ok(route)
}

pub(crate) fn execute(
    world: &mut TickState,
    id: SettlementId,
    domain: EconomyDomain,
    arguments: &[String],
) -> Result<String, Failure> {
    let [verb, rest @ ..] = arguments else {
        return Err(Failure::InvalidRequest);
    };
    if verb == "status" && rest.is_empty() {
        let state = &world.settlements.get(id).ok_or(Failure::NotFound)?.economy;
        return Ok(match domain {
            EconomyDomain::Production => format!(
                "Production: jobs={} completed={} cancelled={}",
                state.production.jobs.len(),
                state.production.completed,
                state.production.cancelled,
            ),
            EconomyDomain::Commerce => format!(
                "Commerce: quotes={} mandates={}",
                state.commerce.quotes.len(),
                state.commerce.mandates.len(),
            ),
            EconomyDomain::Payroll => {
                format!("Payroll: employments={}", state.commerce.employments.len())
            }
            EconomyDomain::Caravan => {
                format!("Caravan: shipments={}", state.caravan.shipments.len())
            }
        });
    }
    let action = match domain {
        EconomyDomain::Production => match (verb.as_str(), rest) {
            ("start", [job, worker, recipe, count, certificate, station, stand_at]) => {
                Action::ProductionStart {
                    job: job.clone(),
                    worker: worker.clone(),
                    recipe: recipe.clone(),
                    count: number(count)?,
                    building: building(world, id, certificate)?,
                    station: position(station)?,
                    stand_at: position(stand_at)?,
                }
            }
            ("pause", [job]) => Action::ProductionPause { job: job.clone() },
            ("resume", [job]) => Action::ProductionResume { job: job.clone() },
            ("cancel", [job]) => Action::ProductionCancel { job: job.clone() },
            _ => return Err(Failure::InvalidRequest),
        },
        EconomyDomain::Payroll => match (verb.as_str(), rest) {
            ("employ", [contract, worker, treasury, wage, period, budget]) => Action::Employ {
                contract: contract.clone(),
                worker: worker.clone(),
                treasury: endpoint(treasury)?,
                wage: number(wage)?,
                period: number(period)?,
                budget: number(budget)?,
            },
            ("stop", [contract]) => Action::StopEmployment {
                contract: contract.clone(),
            },
            ("dismiss", [contract]) => Action::DismissEmployment {
                contract: contract.clone(),
            },
            ("pay", [contract]) => Action::Pay {
                contract: contract.clone(),
            },
            _ => return Err(Failure::InvalidRequest),
        },
        EconomyDomain::Commerce => match (verb.as_str(), rest) {
            ("quote", [quote, seller, buyer, slot, quantity, price]) => Action::Quote {
                id: quote.clone(),
                seller: endpoint(seller)?,
                buyer: endpoint(buyer)?,
                source_slot: number(slot)?,
                quantity: number(quantity)?,
                price: number(price)?,
            },
            ("accept", [quote]) => Action::AcceptQuote { id: quote.clone() },
            ("cancel", [quote]) => Action::CancelQuote { id: quote.clone() },
            ("reserve", [mandate, shipment, payer, escrow, payee, price]) => {
                Action::EscrowReserve {
                    id: mandate.clone(),
                    shipment: shipment.clone(),
                    payer: endpoint(payer)?,
                    escrow: endpoint(escrow)?,
                    payee: endpoint(payee)?,
                    price: number(price)?,
                }
            }
            ("payout", [mandate]) => Action::EscrowPayout {
                id: mandate.clone(),
            },
            _ => return Err(Failure::InvalidRequest),
        },
        EconomyDomain::Caravan => match (verb.as_str(), rest) {
            (
                "open",
                [
                    shipment,
                    carrier,
                    source_building,
                    source_position,
                    destination_building,
                    destination_position,
                    source_slot,
                    quantity,
                    ticks,
                    waypoints,
                    optional @ ..,
                ],
            ) if optional.len() <= 1 => Action::CaravanOpen {
                id: shipment.clone(),
                carrier: carrier.clone(),
                source_building: building(world, id, source_building)?,
                source_position: position(source_position)?,
                destination_building: building(world, id, destination_building)?,
                destination_position: position(destination_position)?,
                source_slot: number(source_slot)?,
                quantity: number(quantity)?,
                waypoints: route(waypoints)?,
                max_ticks: number(ticks)?,
                paid_mandate: optional.first().cloned(),
            },
            ("pickup", [shipment]) => Action::CaravanPickup {
                id: shipment.clone(),
            },
            ("unload", [shipment]) => Action::CaravanUnload {
                id: shipment.clone(),
            },
            ("cancel", [shipment]) => Action::CaravanCancel {
                id: shipment.clone(),
            },
            ("block", [shipment, reason]) => Action::CaravanBlock {
                id: shipment.clone(),
                reason: reason.clone(),
            },
            _ => return Err(Failure::InvalidRequest),
        },
    };
    economy_customization::execute(world, id, &action)?;
    Ok(format!(
        "{domain:?} operation processed for settlement {}",
        id.0
    ))
}
