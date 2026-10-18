//! Typed settlement customization over the authoritative native economic domains.
//! Both the plugin API and owner CLI resolve stock identities and fences here.
use mc_script::{
    ScriptOperationFailure as Failure, ScriptSettlementEconomyAction as Action,
    ScriptSettlementEconomySnapshot, ScriptSettlementEmploymentSnapshot,
    ScriptSettlementInventoryEndpoint as ScriptEndpoint, ScriptSettlementMandateSnapshot,
    ScriptSettlementProductionSnapshot, ScriptSettlementQuoteSnapshot,
    ScriptSettlementShipmentSnapshot,
};
use uuid::Uuid;

use super::inventory::{Endpoint, load};
use super::{SettlementId, caravan, commerce, economy, production};
use crate::tick::TickState;

fn uuid(value: &str) -> Result<Uuid, Failure> {
    value.parse().map_err(|_| Failure::InvalidRequest)
}

fn endpoint(world: &TickState, source: &ScriptEndpoint) -> Result<Endpoint, Failure> {
    Ok(match source {
        ScriptEndpoint::Carry { uuid: value } => Endpoint::VillagerCarry { uuid: uuid(value)? },
        ScriptEndpoint::Equipment { uuid: value } => {
            Endpoint::VillagerEquipment { uuid: uuid(value)? }
        }
        ScriptEndpoint::Player { uuid: value } => Endpoint::Player { uuid: uuid(value)? },
        ScriptEndpoint::Container { position } => Endpoint::Container {
            dimension: world.dimension.name().to_owned(),
            position: *position,
        },
        _ => return Err(Failure::InvalidRequest),
    })
}

fn manifest(
    world: &TickState,
    source: &Endpoint,
    slot: u16,
    quantity: u32,
) -> Result<mc_data::ItemStack, Failure> {
    let image = load(world, source)?;
    let count = i32::try_from(quantity).map_err(|_| Failure::InvalidRequest)?;
    let mut goods = image
        .slots
        .get(usize::from(slot))
        .cloned()
        .ok_or(Failure::InvalidRequest)?;
    if count <= 0 || count > goods.count || goods.item_id == 0 {
        return Err(Failure::InsufficientItems);
    }
    goods.count = count;
    Ok(goods)
}

pub(crate) fn execute(
    world: &mut TickState,
    settlement: SettlementId,
    action: &Action,
) -> Result<(), Failure> {
    action.validate().map_err(|_| Failure::InvalidRequest)?;
    let request = match action {
        Action::ProductionStart {
            job,
            worker,
            recipe,
            count,
            building,
            station,
            stand_at,
        } => economy::Request::Production(production::Request::Start {
            job: job.clone(),
            worker: uuid(worker)?,
            recipe: recipe.clone(),
            count: *count,
            building: building.clone(),
            station: *station,
            stand_at: *stand_at,
        }),
        Action::ProductionPause { job } => {
            economy::Request::Production(production::Request::Pause { job: job.clone() })
        }
        Action::ProductionResume { job } => {
            economy::Request::Production(production::Request::Resume { job: job.clone() })
        }
        Action::ProductionCancel { job } => {
            economy::Request::Production(production::Request::Cancel { job: job.clone() })
        }
        Action::Employ {
            contract,
            worker,
            treasury,
            wage,
            period,
            budget,
        } => economy::Request::Commerce(commerce::Request::Employ {
            contract: contract.clone(),
            worker: uuid(worker)?,
            treasury: endpoint(world, treasury)?,
            wage: *wage,
            period: *period,
            budget: *budget,
        }),
        Action::StopEmployment { contract } => {
            economy::Request::Commerce(commerce::Request::StopEmployment {
                contract: contract.clone(),
            })
        }
        Action::DismissEmployment { contract } => {
            economy::Request::Commerce(commerce::Request::DismissEmployment {
                contract: contract.clone(),
            })
        }
        Action::Pay { contract } => {
            let employment = world
                .settlements
                .get(settlement)
                .ok_or(Failure::NotFound)?
                .economy
                .commerce
                .employments
                .get(contract)
                .ok_or(Failure::NotFound)?;
            let attempt = employment.attempt.checked_add(1).ok_or(Failure::Capacity)?;
            economy::Request::Commerce(commerce::Request::Pay {
                contract: contract.clone(),
                treasury_fence: load(world, &employment.treasury)?.fence,
                worker_fence: load(
                    world,
                    &Endpoint::VillagerCarry {
                        uuid: employment.worker,
                    },
                )?
                .fence,
                op: commerce::pay_op(contract, attempt),
            })
        }
        Action::Quote {
            id,
            seller,
            buyer,
            source_slot,
            quantity,
            price,
        } => {
            let seller = endpoint(world, seller)?;
            let buyer = endpoint(world, buyer)?;
            economy::Request::Commerce(commerce::Request::Quote {
                id: id.clone(),
                goods: manifest(world, &seller, *source_slot, *quantity)?,
                price: *price,
                seller_fence: load(world, &seller)?.fence,
                buyer_fence: load(world, &buyer)?.fence,
                seller,
                buyer,
            })
        }
        Action::AcceptQuote { id } => {
            let quote = world
                .settlements
                .get(settlement)
                .ok_or(Failure::NotFound)?
                .economy
                .commerce
                .quotes
                .get(id)
                .ok_or(Failure::NotFound)?;
            economy::Request::Commerce(commerce::Request::AcceptQuote {
                id: id.clone(),
                seller_fence: load(world, &quote.seller)?.fence,
                buyer_fence: load(world, &quote.buyer)?.fence,
            })
        }
        Action::CancelQuote { id } => {
            economy::Request::Commerce(commerce::Request::CancelQuote { id: id.clone() })
        }
        Action::EscrowReserve {
            id,
            shipment,
            payer,
            escrow,
            payee,
            price,
        } => {
            let payer = endpoint(world, payer)?;
            economy::Request::Commerce(commerce::Request::EscrowReserve {
                id: id.clone(),
                shipment: shipment.clone(),
                payer_fence: load(world, &payer)?.fence,
                payer,
                escrow: endpoint(world, escrow)?,
                payee: endpoint(world, payee)?,
                price: *price,
            })
        }
        Action::EscrowPayout { id } => {
            let mandate = world
                .settlements
                .get(settlement)
                .ok_or(Failure::NotFound)?
                .economy
                .commerce
                .mandates
                .get(id)
                .ok_or(Failure::NotFound)?;
            economy::Request::Commerce(commerce::Request::EscrowPayout {
                id: id.clone(),
                op: commerce::payout_op(id),
                escrow_fence: load(world, &mandate.escrow)?.fence,
                payee_fence: load(world, &mandate.payee)?.fence,
            })
        }
        Action::CaravanOpen {
            id,
            carrier,
            source_building,
            source_position,
            destination_building,
            destination_position,
            source_slot,
            quantity,
            waypoints,
            max_ticks,
            paid_mandate,
        } => {
            let source = caravan::ContainerRef {
                building: source_building.clone(),
                dimension: world.dimension.name().to_owned(),
                position: *source_position,
            };
            let destination = caravan::ContainerRef {
                building: destination_building.clone(),
                dimension: world.dimension.name().to_owned(),
                position: *destination_position,
            };
            let source_endpoint = Endpoint::Container {
                dimension: source.dimension.clone(),
                position: source.position,
            };
            economy::Request::Caravan(caravan::Request::Open(Box::new(caravan::Open {
                id: id.clone(),
                source,
                destination,
                carrier: uuid(carrier)?,
                manifest: manifest(world, &source_endpoint, *source_slot, *quantity)?,
                quantity: *quantity,
                waypoints: waypoints.clone(),
                max_ticks: *max_ticks,
                paid_mandate: paid_mandate.clone(),
            })))
        }
        Action::CaravanPickup { id } => {
            economy::Request::Caravan(caravan::Request::Pickup { id: id.clone() })
        }
        Action::CaravanUnload { id } => {
            economy::Request::Caravan(caravan::Request::Unload { id: id.clone() })
        }
        Action::CaravanCancel { id } => {
            economy::Request::Caravan(caravan::Request::Cancel { id: id.clone() })
        }
        Action::CaravanBlock { id, reason } => economy::Request::Caravan(caravan::Request::Block {
            id: id.clone(),
            reason: reason.clone(),
        }),
        _ => return Err(Failure::InvalidRequest),
    };
    economy::execute(world, settlement, request)
}

fn observed_endpoint(endpoint: &Endpoint) -> ScriptEndpoint {
    match endpoint {
        Endpoint::VillagerCarry { uuid } => ScriptEndpoint::Carry {
            uuid: uuid.to_string(),
        },
        Endpoint::VillagerEquipment { uuid } => ScriptEndpoint::Equipment {
            uuid: uuid.to_string(),
        },
        Endpoint::Player { uuid } => ScriptEndpoint::Player {
            uuid: uuid.to_string(),
        },
        Endpoint::Container { position, .. } => ScriptEndpoint::Container {
            position: *position,
        },
    }
}

fn observed_item(world: &TickState, stack: &mc_data::ItemStack) -> Result<String, Failure> {
    world
        .registry
        .items
        .name_of(stack.item_id)
        .map(|id| id.as_str().to_owned())
        .ok_or(Failure::InvalidRequest)
}

/// Detached contract projections only. Actual inventory contents, bodies and
/// district readiness are never cloned into settlement policy or a KV mirror.
pub(crate) fn observe(
    world: &TickState,
    state: &economy::State,
) -> Result<ScriptSettlementEconomySnapshot, Failure> {
    let mut snapshot = ScriptSettlementEconomySnapshot::new();
    snapshot.production = state
        .production
        .jobs
        .values()
        .map(|job| {
            let mut view = ScriptSettlementProductionSnapshot::new(
                job.id.clone(),
                job.worker.to_string(),
                job.recipe.clone(),
                job.recipe_revision.clone(),
                job.count,
            );
            view.completed = job.done;
            view.paused.clone_from(&job.paused);
            view.cancelled = job.cancelled;
            view.next_due = job.next_due;
            view
        })
        .collect();
    snapshot.employments = state
        .commerce
        .employments
        .iter()
        .map(|(id, contract)| {
            let mut view = ScriptSettlementEmploymentSnapshot::new(
                id.clone(),
                contract.worker.to_string(),
                observed_endpoint(&contract.treasury),
                contract.wage,
                contract.period_ticks,
                contract.budget,
            );
            view.earned = contract.earned;
            view.paid = contract.paid;
            view.active = contract.active;
            view.next_due = contract.next_due;
            view
        })
        .collect();
    snapshot.quotes = state
        .commerce
        .quotes
        .iter()
        .map(|(id, quote)| {
            Ok(ScriptSettlementQuoteSnapshot::new(
                id.clone(),
                observed_endpoint(&quote.seller),
                observed_endpoint(&quote.buyer),
                observed_item(world, &quote.goods)?,
                u32::try_from(quote.goods.count).map_err(|_| Failure::InvalidRequest)?,
                quote.price,
            ))
        })
        .collect::<Result<_, Failure>>()?;
    snapshot.mandates = state
        .commerce
        .mandates
        .iter()
        .map(|(id, mandate)| {
            let mut view = ScriptSettlementMandateSnapshot::new(
                id.clone(),
                mandate.shipment.clone(),
                observed_endpoint(&mandate.payer),
                observed_endpoint(&mandate.escrow),
                observed_endpoint(&mandate.payee),
                mandate.price,
            );
            view.status = match mandate.state {
                commerce::EscrowState::Escrowed => "escrowed",
                commerce::EscrowState::Lost => "lost",
            }
            .to_owned();
            view
        })
        .collect();
    snapshot.shipments = state
        .caravan
        .shipments
        .values()
        .map(|shipment| {
            let mut view = ScriptSettlementShipmentSnapshot::new(
                shipment.id.clone(),
                shipment.carrier.to_string(),
                ScriptEndpoint::Container {
                    position: shipment.source.position,
                },
                ScriptEndpoint::Container {
                    position: shipment.destination.position,
                },
                observed_item(world, &shipment.manifest)?,
                shipment.quantity,
            );
            view.stage = match shipment.status {
                caravan::Status::Loading => "loading",
                caravan::Status::Transit => "transit",
                caravan::Status::Arrived => "arrived",
                caravan::Status::Blocked => "blocked",
                caravan::Status::Delivered => "delivered",
                caravan::Status::Lost => "lost",
                caravan::Status::Cancelled => "cancelled",
            }
            .to_owned();
            view.waypoint = u32::try_from(shipment.cursor).map_err(|_| Failure::InvalidRequest)?;
            view.waypoints.clone_from(&shipment.waypoints);
            view.blocked.clone_from(&shipment.blocked);
            view.paid_mandate.clone_from(&shipment.paid_mandate);
            Ok(view)
        })
        .collect::<Result<_, Failure>>()?;
    snapshot.ended_employment = state.commerce.ended_employment;
    snapshot.completed_quote = state.commerce.completed_quote;
    snapshot.settled_mandate = state.commerce.settled_mandate;
    Ok(snapshot)
}
