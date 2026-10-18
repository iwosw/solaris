//! A retirement fence rejects new replays, never an older still-live contract.
use super::*;
use crate::settlements::{Registry, economy, persistence};

fn native(f: &mut Fixture, request: Request) {
    economy::execute(
        &mut f.state,
        f.settlement,
        economy::Request::Commerce(request),
    )
    .unwrap();
}

fn recover_domain(f: &mut Fixture, checkpoint: bool) {
    let tick = f.state.tick;
    let sequence = f.state.settlement_sequence;
    let rows = persistence::checkpoint_rows(&f.state, tick).unwrap();
    let mut entries = Vec::new();
    persistence::journal_entries(&mut f.state, tick, &mut entries);
    assert_eq!(entries.len(), 1, "only this settlement was touched");
    let replay = crate::journal::apply_batch_images(
        crate::journal::Replay::default(),
        &[(tick, tick, entries)],
    );
    f.state.settlements = Registry::default();
    if checkpoint {
        persistence::hydrate_dimension(
            &mut f.state,
            &rows,
            &crate::journal::Replay::default(),
            sequence,
        )
        .unwrap();
    } else {
        persistence::hydrate_dimension(&mut f.state, &Default::default(), &replay, sequence)
            .unwrap();
    }
    persistence::finish_recovery(&mut f.state).unwrap();
}

fn seed_carry(f: &mut Fixture, entity: EntityId, item: u32, count: i32) {
    let slot = f.state.entities.slot_of(entity).unwrap();
    f.state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .inventory[0] = Some(ItemStack::new(item, count));
}

#[test]
fn lower_live_quote_completes_after_higher_retirement_and_native_recovery() {
    for checkpoint in [false, true] {
        let mut f = fixture();
        stock(&mut f.state, TREASURY_CHEST, f.bread, 6);
        stock(&mut f.state, ESCROW_CHEST, f.emerald, 10);
        let worker = f.worker;
        let buyer = f.buyer;
        let bread = f.bread;
        let emerald = f.emerald;
        seed_carry(&mut f, worker, bread, 6);
        seed_carry(&mut f, buyer, emerald, 10);
        let quote1 = quote_request(&f, "1", ItemStack::new(bread, 6), 10);
        native(&mut f, quote1);
        let seller = carry(f.worker_uuid);
        let buyer = carry(f.buyer_uuid);
        let seller_fence = fences(&f, &seller);
        let buyer_fence = fences(&f, &buyer);
        native(
            &mut f,
            Request::Quote {
                id: "2".into(),
                seller,
                buyer,
                goods: ItemStack::new(bread, 6),
                price: 10,
                seller_fence: seller_fence.clone(),
                buyer_fence: buyer_fence.clone(),
            },
        );
        native(
            &mut f,
            Request::AcceptQuote {
                id: "2".into(),
                seller_fence,
                buyer_fence,
            },
        );
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        assert_eq!(commerce.completed_quote, 2);
        assert!(commerce.quotes.contains_key("1"));
        recover_domain(&mut f, checkpoint);
        let quote1 = accept_request(&f, "1");
        native(&mut f, quote1);
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        assert_eq!(
            commerce.completed_quote, 2,
            "retirement never moves backward"
        );
        assert!(commerce.quotes.is_empty());
        assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 10);
        assert_eq!(emerald_total(&f.state, ESCROW_CHEST), 0);
        assert_eq!(carry_total(&f.state, f.worker), 10);
        assert_eq!(carry_total(&f.state, f.buyer), 6);
        let replay = quote_request(&f, "1", ItemStack::new(bread, 1), 1);
        assert_eq!(
            economy::execute(
                &mut f.state,
                f.settlement,
                economy::Request::Commerce(replay),
            ),
            Err(Failure::OperationConflict)
        );
    }
}

fn deliver(f: &mut Fixture, shipment: &str, mandate: &str) {
    let plan = f
        .state
        .settlements
        .get(f.settlement)
        .unwrap()
        .district
        .as_ref()
        .unwrap();
    let source = crate::settlements::caravan::ContainerRef {
        building: plan.buildings[0].clone(),
        dimension: f.state.dimension.name().into(),
        position: TREASURY_CHEST,
    };
    let destination = crate::settlements::caravan::ContainerRef {
        building: plan.buildings[1].clone(),
        dimension: f.state.dimension.name().into(),
        position: ESCROW_CHEST,
    };
    let request =
        crate::settlements::caravan::Request::Open(Box::new(crate::settlements::caravan::Open {
            id: shipment.into(),
            source,
            destination,
            carrier: f.worker_uuid,
            manifest: ItemStack::new(f.bread, 1),
            quantity: 1,
            waypoints: vec![TREASURY_DOOR, ESCROW_DOOR],
            max_ticks: 72_000,
            paid_mandate: Some(mandate.into()),
        }));
    economy::execute(
        &mut f.state,
        f.settlement,
        economy::Request::Caravan(request),
    )
    .unwrap();
    let slot = f.state.entities.slot_of(f.worker).unwrap();
    for position in [TREASURY_DOOR, ESCROW_DOOR] {
        f.state.entities.set_pos(
            slot,
            [
                f64::from(position[0]) + 0.5,
                f64::from(position[1]),
                f64::from(position[2]) + 0.5,
            ],
        );
        economy::tick(&mut f.state);
    }
    assert!(crate::settlements::caravan::delivered(
        &f.state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .caravan,
        shipment,
    ));
}

#[test]
fn lower_held_escrow_pays_after_higher_settlement_and_native_recovery() {
    for checkpoint in [false, true] {
        let mut f = fixture();
        stock(&mut f.state, TREASURY_CHEST, f.emerald, 20);
        f.state
            .block_entities
            .get_mut(&(TREASURY_CHEST[0], TREASURY_CHEST[1], TREASURY_CHEST[2]))
            .unwrap()
            .slots[1] = ItemStack::new(f.bread, 2);
        for (id, shipment) in [("1", "shipment-1"), ("2", "shipment-2")] {
            let request = Request::EscrowReserve {
                id: id.into(),
                shipment: shipment.into(),
                payer: f.treasury.clone(),
                escrow: f.escrow.clone(),
                payee: carry(f.buyer_uuid),
                price: 6,
                payer_fence: fences(&f, &f.treasury),
            };
            native(&mut f, request);
        }
        deliver(&mut f, "shipment-2", "2");
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        assert_eq!(commerce.settled_mandate, 2);
        assert!(commerce.mandates.contains_key("1"));
        assert_eq!(carry_total(&f.state, f.buyer), 6);
        recover_domain(&mut f, checkpoint);
        deliver(&mut f, "shipment-1", "1");
        // No payout command: real delivery drives the physical escrow release.
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        assert_eq!(commerce.settled_mandate, 2);
        assert!(commerce.mandates.is_empty());
        assert_eq!(carry_total(&f.state, f.buyer), 12);
        assert_eq!(f.state.block_entities[&(10, 201, 10)].slots[0].count, 8);
        let escrow = &f.state.block_entities[&(20, 201, 10)].slots;
        assert_eq!(
            escrow
                .iter()
                .filter(|s| s.item_id == f.emerald)
                .map(|s| s.count)
                .sum::<i32>(),
            0
        );
        assert_eq!(
            escrow
                .iter()
                .filter(|s| s.item_id == f.bread)
                .map(|s| s.count)
                .sum::<i32>(),
            2
        );
        let replay = Request::EscrowReserve {
            id: "1".into(),
            shipment: "replay".into(),
            payer: f.treasury.clone(),
            escrow: f.escrow.clone(),
            payee: carry(f.buyer_uuid),
            price: 1,
            payer_fence: fences(&f, &f.treasury),
        };
        assert_eq!(
            economy::execute(
                &mut f.state,
                f.settlement,
                economy::Request::Commerce(replay),
            ),
            Err(Failure::OperationConflict)
        );
    }
}

#[test]
fn lower_live_employment_remains_valid_after_higher_dismissal() {
    for checkpoint in [false, true] {
        let mut f = fixture();
        let request = employ(&f, "1", 1, 5, 10);
        native(&mut f, request);
        let request = Request::Employ {
            contract: "2".into(),
            worker: f.buyer_uuid,
            treasury: f.treasury.clone(),
            wage: 1,
            period: 5,
            budget: 10,
        };
        native(&mut f, request);
        for contract in ["2", "1"] {
            native(
                &mut f,
                Request::StopEmployment {
                    contract: contract.into(),
                },
            );
            native(
                &mut f,
                Request::DismissEmployment {
                    contract: contract.into(),
                },
            );
            if contract == "2" {
                recover_domain(&mut f, checkpoint);
            }
        }
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        assert_eq!(commerce.ended_employment, 2);
        assert!(commerce.employments.is_empty());
    }
}

#[test]
fn sixty_five_native_purchases_survive_restart_without_lifetime_cap() {
    for checkpoint in [false, true] {
        let mut f = fixture();
        for sequence in 1..=65 {
            f.state.tick += 1;
            f.state.tick_budget.reset(f.state.tick);
            let id = sequence.to_string();
            stock(&mut f.state, TREASURY_CHEST, f.bread, 1);
            stock(&mut f.state, ESCROW_CHEST, f.emerald, 1);
            let quote = quote_request(&f, &id, ItemStack::new(f.bread, 1), 1);
            native(&mut f, quote);
            let accept = accept_request(&f, &id);
            native(&mut f, accept);
            if sequence == 32 {
                recover_domain(&mut f, checkpoint);
            }
        }
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        assert_eq!(commerce.completed_quote, 65);
        assert!(commerce.quotes.is_empty());
        let replay = quote_request(&f, "1", ItemStack::new(f.bread, 1), 1);
        assert_eq!(
            economy::execute(
                &mut f.state,
                f.settlement,
                economy::Request::Commerce(replay),
            ),
            Err(Failure::OperationConflict)
        );
    }
}

#[test]
fn seventy_native_hire_dismiss_cycles_survive_restart_without_lifetime_cap() {
    for checkpoint in [false, true] {
        let mut f = fixture();
        for sequence in 1..=70 {
            f.state.tick += 1;
            f.state.tick_budget.reset(f.state.tick);
            let contract = sequence.to_string();
            let request = employ(&f, &contract, 1, 5, 10);
            native(&mut f, request);
            native(
                &mut f,
                Request::StopEmployment {
                    contract: contract.clone(),
                },
            );
            native(&mut f, Request::DismissEmployment { contract });
            if sequence == 32 {
                recover_domain(&mut f, checkpoint);
            }
        }
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        assert_eq!(commerce.ended_employment, 70);
        assert!(commerce.employments.is_empty());
        let replay = employ(&f, "1", 1, 5, 10);
        assert_eq!(
            economy::execute(
                &mut f.state,
                f.settlement,
                economy::Request::Commerce(replay),
            ),
            Err(Failure::OperationConflict)
        );
    }
}

#[test]
fn typed_customization_queries_real_terms_fences_and_durable_quote_replay() {
    use mc_script::{
        ScriptSettlementEconomyAction as Action, ScriptSettlementInventoryEndpoint as Endpoint,
        ScriptSettlementOperation as Operation, ScriptSettlementQuery, ScriptSettlementResult,
    };
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.bread, 6);
    stock(&mut f.state, ESCROW_CHEST, f.emerald, 3);
    f.state
        .block_entities
        .get_mut(&(10, 201, 10))
        .unwrap()
        .slots[0]
        .custom_name = Some("native market loaf".into());
    let before = f.state.settlements.get(f.settlement).unwrap().revision;
    let quote = ScriptOperationRequest::try_new(
        "typed-quote",
        ScriptOperation::Settlement {
            operation: Operation::Economy {
                operation_id: "native-quote-1".into(),
                settlement: f.settlement.0,
                expected_revision: before,
                action: Box::new(Action::Quote {
                    id: "1".into(),
                    seller: Endpoint::Container {
                        position: TREASURY_CHEST,
                    },
                    buyer: Endpoint::Container {
                        position: ESCROW_CHEST,
                    },
                    source_slot: 0,
                    quantity: 2,
                    price: 3,
                }),
            },
        },
    )
    .unwrap();
    let quoted = crate::plugin::operation(&mut f.state, OWNER, &quote);
    assert_eq!(quoted.failure(), None);
    let record = f.state.settlements.get(f.settlement).unwrap();
    assert_eq!(record.revision, before + 1);
    assert_eq!(
        record.economy.commerce.quotes["1"]
            .goods
            .custom_name
            .as_deref(),
        Some("native market loaf")
    );
    let query = ScriptOperationRequest::try_new(
        "typed-economic-query",
        ScriptOperation::SettlementQuery {
            query: ScriptSettlementQuery::new(vec!["market".into()], None),
        },
    )
    .unwrap();
    let observed = crate::plugin::operation(&mut f.state, OWNER, &query);
    assert_eq!(observed.failure(), None);
    let ScriptOperationPayload::Settlement { result } = observed.payload() else {
        panic!("settlement observation payload")
    };
    let ScriptSettlementResult::Page(page) = result.as_ref() else {
        panic!("settlement page")
    };
    let economy = page.settlements[0].economy.as_ref().unwrap();
    assert_eq!(economy.quotes.len(), 1);
    assert_eq!(economy.quotes[0].id, "1");
    assert_eq!(economy.quotes[0].item_id, "minecraft:bread");
    assert_eq!(
        (economy.quotes[0].quantity, economy.quotes[0].price),
        (2, 3)
    );
    let stale = ScriptOperationRequest::try_new(
        "stale-economic-request",
        ScriptOperation::Settlement {
            operation: Operation::Economy {
                operation_id: "stale-economic-action".into(),
                settlement: f.settlement.0,
                expected_revision: before,
                action: Box::new(Action::CancelQuote { id: "1".into() }),
            },
        },
    )
    .unwrap();
    assert_eq!(
        crate::plugin::operation(&mut f.state, OWNER, &stale).failure(),
        Some(Failure::StaleRevision)
    );
    let accept = ScriptOperationRequest::try_new(
        "typed-accept",
        ScriptOperation::Settlement {
            operation: Operation::Economy {
                operation_id: "native-accept-1".into(),
                settlement: f.settlement.0,
                expected_revision: before + 1,
                action: Box::new(Action::AcceptQuote { id: "1".into() }),
            },
        },
    )
    .unwrap();
    assert_eq!(
        crate::plugin::operation(&mut f.state, OWNER, &accept).failure(),
        None
    );
    assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 3);
    let revision = f.state.settlements.get(f.settlement).unwrap().revision;
    // The original durable operation receipt wins over its now-stale revision;
    // no live quote or second stock exchange is recreated.
    assert_eq!(
        crate::plugin::operation(&mut f.state, OWNER, &quote),
        quoted
    );
    let record = f.state.settlements.get(f.settlement).unwrap();
    assert_eq!(record.revision, revision);
    assert_eq!(record.economy.commerce.completed_quote, 1);
    assert!(record.economy.commerce.quotes.is_empty());
    assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 3);
}

#[test]
fn native_tick_pays_each_due_period_without_pay_commands_across_domain_recovery() {
    for checkpoint in [false, true] {
        let mut f = fixture();
        stock(&mut f.state, TREASURY_CHEST, f.emerald, 6);
        let request = employ(&f, "1", 2, 5, 6);
        native(&mut f, request);
        for period in 1..=3 {
            f.state.tick = period * 5;
            f.state.tick_budget.reset(f.state.tick);
            economy::tick(&mut f.state);
            assert_eq!(carry_total(&f.state, f.worker), (period * 2) as i32);
            assert_eq!(
                emerald_total(&f.state, TREASURY_CHEST),
                (6 - period * 2) as i32
            );
            let contract = &f
                .state
                .settlements
                .get(f.settlement)
                .unwrap()
                .economy
                .commerce
                .employments["1"];
            assert_eq!((contract.earned, contract.paid), (period * 2, period * 2));
            assert_eq!(contract.attempt, period);
            // Another tick at the same clock never duplicates a physical wage.
            economy::tick(&mut f.state);
            assert_eq!(carry_total(&f.state, f.worker), (period * 2) as i32);
            if period == 2 {
                recover_domain(&mut f, checkpoint);
            }
        }
    }
}

#[test]
fn stopped_owed_wages_remain_active_and_pay_after_refill_and_domain_recovery() {
    for checkpoint in [false, true] {
        let mut f = fixture();
        let request = employ(&f, "1", 3, 5, 12);
        native(&mut f, request);
        f.state.tick = 5;
        economy::tick(&mut f.state);
        native(
            &mut f,
            Request::StopEmployment {
                contract: "1".into(),
            },
        );
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        assert_eq!(
            (
                commerce.employments["1"].earned,
                commerce.employments["1"].paid
            ),
            (3, 0)
        );
        assert!(
            super::active(commerce),
            "stopped unpaid debt must remain scheduled"
        );
        recover_domain(&mut f, checkpoint);
        stock(&mut f.state, TREASURY_CHEST, f.emerald, 3);
        f.state.tick = 6;
        economy::tick(&mut f.state);
        assert_eq!(
            carry_total(&f.state, f.worker),
            0,
            "retry only at the due period"
        );
        f.state.tick = 10;
        economy::tick(&mut f.state);
        assert_eq!(carry_total(&f.state, f.worker), 3);
        assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 0);
        let commerce = &f
            .state
            .settlements
            .get(f.settlement)
            .unwrap()
            .economy
            .commerce;
        assert_eq!(
            (
                commerce.employments["1"].earned,
                commerce.employments["1"].paid
            ),
            (3, 3)
        );
        assert!(!super::active(commerce));
        native(
            &mut f,
            Request::DismissEmployment {
                contract: "1".into(),
            },
        );
        assert_eq!(
            f.state
                .settlements
                .get(f.settlement)
                .unwrap()
                .economy
                .commerce
                .ended_employment,
            1
        );
    }
}
