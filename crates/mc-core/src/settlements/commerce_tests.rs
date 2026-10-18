//! Native commerce behavioral tests through [`execute`](super::execute)/[`tick`](super::tick).
//!
//! Real prerequisites only: exact living member bodies, real native chests,
//! and the parent inventory bridge. These tests pin emerald/commodity
//! conservation, stale fences leaving both sides unchanged, component
//! identity, payroll accrual + budget/earned/paid + stopped-debt, single
//! debit on duplicate acceptance/payment, serde restart replay, retirement
//! (no lifetime shutdown: 65 sequential completions with FIRST-id replay
//! refused, hire-stop-dismiss-hire x70 with FIRST-contract replay refused),
//! cancelled-quote removal + replay refusal, and that terminal journals alone
//! never keep a town ticking.

use super::*;
use crate::boundary::{ChunkPos, EntityId};
use crate::settlements::SettlementId;
use crate::tick::TickState;
use mc_script::{
    ScriptOperation, ScriptOperationFailure as Failure, ScriptOperationPayload,
    ScriptOperationRequest, ScriptSettlementBuildingRef, ScriptWorksiteInteractionPoint,
    ScriptWorksiteRequest, ScriptWorldBounds, ScriptWorldStructureOperation,
    ScriptWorldStructureResult,
};
use std::sync::Arc;

const OWNER: &str = "owner";
const TREASURY_CHEST: [i32; 3] = [10, 201, 10];
const TREASURY_DOOR: [i32; 3] = [10, 201, 11];
const ESCROW_CHEST: [i32; 3] = [20, 201, 10];
const ESCROW_DOOR: [i32; 3] = [20, 201, 11];

fn state() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 200, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(name).unwrap();
    let default = state.registry.blocks.block(&id).unwrap().default;
    let mut full = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (key, value) in props {
        if let Some(entry) = full.iter_mut().find(|(k, _)| k == key) {
            entry.1 = (*value).to_owned();
        }
    }
    state
        .registry
        .blocks
        .by_name_and_props(&id, &full)
        .unwrap()
        .0
}

fn ensure(state: &mut TickState) {
    for (i, (x, z)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
        state
            .tickets
            .insert(format!("commerce-{i}"), (ChunkPos { x, z }, 0));
    }
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    state.refresh_active_chunks(state.tick);
}

fn floor(state: &mut TickState) {
    ensure(state);
    let stone = block(state, "minecraft:stone", &[]);
    let air = state.registry.states.air;
    for x in 0..32 {
        for z in 0..32 {
            state.world.set_block(x, 200, z, stone);
            for y in 201..=204 {
                state.world.set_block(x, y, z, air);
            }
        }
    }
    state.world.fold_overlay();
}

fn place_chest(state: &mut TickState, at: [i32; 3]) {
    let id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let default = state.registry.blocks.block(&id).unwrap().default;
    let mut properties = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (name, value) in &mut properties {
        if name == "type" {
            *value = "single".to_owned();
        }
        if name == "facing" {
            *value = "north".to_owned();
        }
    }
    let state_id = state
        .registry
        .blocks
        .by_name_and_props(&id, &properties)
        .unwrap();
    state.world.set_block(at[0], at[1], at[2], state_id.0);
    state.block_entities.insert(
        (at[0], at[1], at[2]),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    state.world.fold_overlay();
}

fn town(state: &mut TickState, name: &str, anchor: [i32; 3], radius: u32) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            name.into(),
            OWNER.into(),
            anchor,
            radius,
        )
        .unwrap()
}

fn stray(state: &mut TickState, pos: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState::default()));
    id
}

fn resident(state: &mut TickState, settlement: SettlementId, pos: [f64; 3]) -> EntityId {
    let id = stray(state, pos);
    crate::settlements::bind(state, settlement, id).unwrap();
    id
}

fn uuid_of(state: &TickState, id: EntityId) -> Uuid {
    state.entities.row(state.entities.slot_of(id).unwrap()).uuid
}

fn entrance(at: [i32; 3]) -> ScriptWorksiteInteractionPoint {
    ScriptWorksiteInteractionPoint::Entrance { at }
}

fn worksite(id: &str, purpose: &str) -> ScriptSettlementBuildingRef {
    ScriptSettlementBuildingRef::new("worksite".into(), id.into(), purpose.into())
}

fn survey_bounds(
    state: &mut TickState,
    owner: &str,
    min: [i32; 3],
    max: [i32; 3],
) -> (String, u64) {
    let request = ScriptOperationRequest::try_new(
        "survey-commerce",
        ScriptOperation::WorldStructure {
            operation: ScriptWorldStructureOperation::Survey {
                dimension: state.dimension.name().into(),
                bounds: ScriptWorldBounds::new(min, max).unwrap(),
            },
        },
    )
    .unwrap();
    let done = crate::plugin::operation(state, owner, &request);
    assert_eq!(done.failure(), None);
    let ScriptOperationPayload::WorldStructure { result } = done.payload() else {
        panic!("survey payload")
    };
    let ScriptWorldStructureResult::Survey { survey } = result.as_ref() else {
        panic!("survey result")
    };
    (survey.survey_token.clone(), survey.world_revision)
}

fn register(
    state: &mut TickState,
    owner: &str,
    operation_id: &str,
    request_id: &str,
    request: ScriptWorksiteRequest,
) -> String {
    let request = ScriptOperationRequest::try_new(
        request_id,
        ScriptOperation::WorldStructure {
            operation: ScriptWorldStructureOperation::RegisterWorksite {
                operation_id: operation_id.into(),
                request: Box::new(request),
            },
        },
    )
    .unwrap();
    let done = crate::plugin::operation(state, owner, &request);
    assert_eq!(done.failure(), None);
    let ScriptOperationPayload::WorldStructure { result } = done.payload() else {
        panic!("worksite payload")
    };
    let ScriptWorldStructureResult::Worksite { worksite } = result.as_ref() else {
        panic!("worksite result")
    };
    worksite.worksite_id.clone()
}

fn warehouse(
    state: &mut TickState,
    owner: &str,
    token: &str,
    revision: u64,
    name: &str,
    door: [i32; 3],
    chest: [i32; 3],
) -> String {
    register(
        state,
        owner,
        name,
        name,
        ScriptWorksiteRequest::new(
            ScriptWorldBounds::new(
                [door[0].min(chest[0]), door[1], door[2].min(chest[2])],
                [door[0].max(chest[0]), door[1], door[2].max(chest[2])],
            )
            .unwrap(),
            vec![
                entrance(door),
                ScriptWorksiteInteractionPoint::Container { at: chest },
            ],
            token.to_owned(),
            revision,
        ),
    )
}

struct Fixture {
    state: TickState,
    settlement: SettlementId,
    treasury: Endpoint,
    escrow: Endpoint,
    worker: EntityId,
    worker_uuid: Uuid,
    buyer: EntityId,
    buyer_uuid: Uuid,
    emerald: u32,
    bread: u32,
}

fn fixture() -> Fixture {
    fixture_in(state())
}

fn fixture_in(mut state: TickState) -> Fixture {
    floor(&mut state);
    place_chest(&mut state, TREASURY_CHEST);
    place_chest(&mut state, ESCROW_CHEST);
    ensure(&mut state);
    let (token, revision) = survey_bounds(&mut state, OWNER, [9, 201, 9], [21, 201, 12]);
    let treasury_site = warehouse(
        &mut state,
        OWNER,
        &token,
        revision,
        "reg-treasury",
        TREASURY_DOOR,
        TREASURY_CHEST,
    );
    let escrow_site = warehouse(
        &mut state,
        OWNER,
        &token,
        revision,
        "reg-escrow",
        ESCROW_DOOR,
        ESCROW_CHEST,
    );
    let settlement = town(&mut state, "market", [8, 201, 8], 48);
    super::super::districts::plan(
        &mut state,
        settlement,
        1,
        super::super::districts::DistrictPlan {
            owner: OWNER.into(),
            branch: "none".into(),
            level: 0,
            buildings: vec![
                worksite(&treasury_site, "solaris:warehouse"),
                worksite(&escrow_site, "solaris:warehouse"),
            ],
        },
    )
    .unwrap();
    let worker = resident(&mut state, settlement, [8.5, 201.0, 20.5]);
    let buyer = resident(&mut state, settlement, [9.5, 201.0, 20.5]);
    let worker_uuid = uuid_of(&state, worker);
    let buyer_uuid = uuid_of(&state, buyer);
    let dimension = state.dimension.name().to_owned();
    let emerald = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    let bread = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:bread").unwrap())
        .unwrap();
    Fixture {
        state,
        settlement,
        treasury: Endpoint::Container {
            dimension: dimension.clone(),
            position: TREASURY_CHEST,
        },
        escrow: Endpoint::Container {
            dimension,
            position: ESCROW_CHEST,
        },
        worker,
        worker_uuid,
        buyer,
        buyer_uuid,
        emerald,
        bread,
    }
}

fn carry(id: Uuid) -> Endpoint {
    Endpoint::VillagerCarry { uuid: id }
}

fn stock(state: &mut TickState, at: [i32; 3], item_id: u32, count: i32) {
    state
        .block_entities
        .get_mut(&(at[0], at[1], at[2]))
        .expect("chest block entity")
        .slots[0] = mc_data::ItemStack::new(item_id, count);
}

fn emerald_total(state: &TickState, at: [i32; 3]) -> i32 {
    state.block_entities[&(at[0], at[1], at[2])]
        .slots
        .iter()
        .filter(|stack| {
            !stack.is_empty()
                && state
                    .registry
                    .items
                    .name_of(stack.item_id)
                    .is_some_and(|id| id.as_str() == "minecraft:emerald")
        })
        .map(|stack| stack.count)
        .sum()
}

fn carry_total(state: &TickState, id: EntityId) -> i32 {
    let slot = state.entities.slot_of_any(id).unwrap();
    state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .inventory
        .iter()
        .flatten()
        .map(|stack| stack.count)
        .sum()
}

fn run(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    request: Request,
) -> bool {
    execute(world, settlement, OWNER, state, request).unwrap()
}

fn fail(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    request: Request,
) -> Failure {
    execute(world, settlement, OWNER, state, request).unwrap_err()
}

fn employ(f: &Fixture, contract: &str, wage: u32, period: u64, budget: u64) -> Request {
    Request::Employ {
        contract: contract.into(),
        worker: f.worker_uuid,
        treasury: f.treasury.clone(),
        wage,
        period,
        budget,
    }
}

fn hire(f: &mut Fixture, policy: &mut State, contract: &str, wage: u32, budget: u64) {
    let request = employ(f, contract, wage, 5, budget);
    assert!(run(&mut f.state, f.settlement, policy, request));
}

fn fences(f: &Fixture, endpoint: &Endpoint) -> ScriptInventoryFence {
    load(&f.state, endpoint).unwrap().fence
}

/// One simulation period pays automatically; then stop and dismiss.
fn earn_pay_stop_dismiss(f: &mut Fixture, policy: &mut State, contract: &str) {
    f.state.tick += 5;
    f.state.tick_budget.reset(f.state.tick);
    assert!(tick(&mut f.state, f.settlement, policy));
    assert_eq!(
        policy.employments[contract].earned,
        policy.employments[contract].paid
    );
    assert!(run(
        &mut f.state,
        f.settlement,
        policy,
        Request::StopEmployment {
            contract: contract.into(),
        }
    ));
    assert!(run(
        &mut f.state,
        f.settlement,
        policy,
        Request::DismissEmployment {
            contract: contract.into(),
        }
    ));
}

fn quote_request(f: &Fixture, id: &str, goods: ItemStack, price: u32) -> Request {
    Request::Quote {
        id: id.into(),
        seller: f.treasury.clone(),
        buyer: f.escrow.clone(),
        goods,
        price,
        seller_fence: fences(f, &f.treasury),
        buyer_fence: fences(f, &f.escrow),
    }
}

fn accept_request(f: &Fixture, id: &str) -> Request {
    Request::AcceptQuote {
        id: id.into(),
        seller_fence: fences(f, &f.treasury),
        buyer_fence: fences(f, &f.escrow),
    }
}

#[test]
fn payroll_accrual_budget_earned_paid_and_stopped_debt() {
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.emerald, 20);
    let mut policy = State::default();
    hire(&mut f, &mut policy, "1", 4, 6);
    assert!(active(&policy));
    let employment = &policy.employments["1"];
    assert_eq!((employment.earned, employment.paid), (0, 0));
    // One period passes: 4 earned.
    f.state.tick += 5;
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.employments["1"].earned, 4);
    let employment = &policy.employments["1"];
    assert_eq!(
        (employment.earned, employment.paid, employment.budget),
        (4, 4, 2)
    );
    assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 16);
    assert_eq!(carry_total(&f.state, f.worker), 4);
    // Two more periods accrue 8; the remaining budget of 2 cannot cover the 8 owed.
    f.state.tick += 10;
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.employments["1"].earned, 12);
    // Stopping ends accrual but preserves the earned debt.
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::StopEmployment {
            contract: "1".into(),
        }
    ));
    f.state.tick += 50;
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.employments["1"].earned, 12);
    // Debt beyond the remaining budget cannot be paid; policy unchanged.
    let before = policy.clone();
    let attempt = policy.employments["1"].attempt + 1;
    let treasury_fence = fences(&f, &f.treasury);
    let worker_fence = fences(&f, &carry(f.worker_uuid));
    assert_eq!(
        fail(
            &mut f.state,
            f.settlement,
            &mut policy,
            Request::Pay {
                contract: "1".into(),
                treasury_fence,
                worker_fence,
                op: pay_op("1", attempt),
            }
        ),
        Failure::InsufficientItems
    );
    assert_eq!(policy, before);
    assert_eq!(policy.employments["1"].paid, 4);
    assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 16);
    validate(&policy).unwrap();
}

#[test]
fn duplicate_payment_cannot_debit_twice() {
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.emerald, 20);
    let mut policy = State::default();
    hire(&mut f, &mut policy, "1", 4, 12);
    f.state.tick += 5;
    let op = pay_op("1", 1);
    let treasury = fences(&f, &f.treasury);
    let worker = fences(&f, &carry(f.worker_uuid));
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::Pay {
            contract: "1".into(),
            treasury_fence: treasury.clone(),
            worker_fence: worker.clone(),
            op: op.clone(),
        }
    ));
    // Exact replay is idempotent and returns false (no second debit).
    assert!(!run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::Pay {
            contract: "1".into(),
            treasury_fence: treasury,
            worker_fence: worker,
            op,
        }
    ));
    assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 16);
    assert_eq!(carry_total(&f.state, f.worker), 4);
    validate(&policy).unwrap();
}

#[test]
fn quote_exchange_conserves_both_sides_and_preserves_components() {
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.bread, 6);
    stock(&mut f.state, ESCROW_CHEST, f.emerald, 10);
    // Seller's bread carries a custom name: the exchange must preserve it.
    f.state
        .block_entities
        .get_mut(&(TREASURY_CHEST[0], TREASURY_CHEST[1], TREASURY_CHEST[2]))
        .unwrap()
        .slots[0]
        .custom_name = Some("market loaf".into());
    let mut policy = State::default();
    let mut goods = ItemStack::new(f.bread, 6);
    goods.custom_name = Some("market loaf".into());
    let request = quote_request(&f, "1", goods, 10);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    let request = accept_request(&f, "1");
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    // Bread moved seller -> buyer, emeralds buyer -> seller, nothing minted.
    assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 10);
    let buyer_slots =
        &f.state.block_entities[&(ESCROW_CHEST[0], ESCROW_CHEST[1], ESCROW_CHEST[2])].slots;
    let bread: i32 = buyer_slots
        .iter()
        .filter(|stack| stack.item_id == f.bread)
        .map(|stack| stack.count)
        .sum();
    assert_eq!(bread, 6);
    assert!(
        buyer_slots
            .iter()
            .any(|stack| stack.custom_name.as_deref() == Some("market loaf"))
    );
    let seller_emeralds: i32 = f.state.block_entities
        [&(TREASURY_CHEST[0], TREASURY_CHEST[1], TREASURY_CHEST[2])]
        .slots
        .iter()
        .filter(|stack| stack.item_id == f.emerald)
        .map(|stack| stack.count)
        .sum();
    assert_eq!(seller_emeralds, 10);
    // Completed quotes retire; the id stays refused via the watermark.
    assert!(!policy.quotes.contains_key("1"));
    assert_eq!(policy.completed_quote, 1);
    assert!(!active(&policy));
    validate(&policy).unwrap();
}

#[test]
fn stale_fence_leaves_both_sides_unchanged() {
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.bread, 6);
    stock(&mut f.state, ESCROW_CHEST, f.emerald, 10);
    let mut policy = State::default();
    let seller_fence = fences(&f, &f.treasury);
    let buyer_fence = fences(&f, &f.escrow);
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::Quote {
            id: "1".into(),
            seller: f.treasury.clone(),
            buyer: f.escrow.clone(),
            goods: ItemStack::new(f.bread, 6),
            price: 10,
            seller_fence: seller_fence.clone(),
            buyer_fence: buyer_fence.clone(),
        }
    ));
    // Unrelated stock movement invalidates the captured fence.
    stock(&mut f.state, TREASURY_CHEST, f.bread, 9);
    let seller_now = load(&f.state, &f.treasury).unwrap();
    assert_ne!(seller_now.fence, seller_fence);
    let before = policy.clone();
    assert_eq!(
        fail(
            &mut f.state,
            f.settlement,
            &mut policy,
            Request::AcceptQuote {
                id: "1".into(),
                seller_fence,
                buyer_fence,
            }
        ),
        Failure::StaleRevision
    );
    // Both sides are exactly what the third party left: no half-move, no policy change.
    assert_eq!(policy, before);
    assert_eq!(seller_now.slots, load(&f.state, &f.treasury).unwrap().slots);
    assert_eq!(emerald_total(&f.state, ESCROW_CHEST), 10);
    assert!(policy.quotes.contains_key("1"));
    validate(&policy).unwrap();
}

#[test]
fn duplicate_acceptance_and_conflicting_replay_refused() {
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.bread, 6);
    stock(&mut f.state, ESCROW_CHEST, f.emerald, 10);
    let mut policy = State::default();
    let request = quote_request(&f, "1", ItemStack::new(f.bread, 6), 10);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    let request = accept_request(&f, "1");
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    // The committed record retires; any late duplicate is a finished id, not a debit.
    let request = accept_request(&f, "1");
    assert_eq!(
        fail(&mut f.state, f.settlement, &mut policy, request),
        Failure::NotFound
    );
    // A conflicting replay under the finished id is refused by the watermark.
    let before = policy.clone();
    let seller_fence = fences(&f, &f.treasury);
    let buyer_fence = fences(&f, &carry(f.buyer_uuid));
    assert_eq!(
        fail(
            &mut f.state,
            f.settlement,
            &mut policy,
            Request::Quote {
                id: "1".into(),
                seller: f.treasury.clone(),
                buyer: carry(f.buyer_uuid),
                goods: ItemStack::new(f.bread, 1),
                price: 1,
                seller_fence,
                buyer_fence,
            }
        ),
        Failure::OperationConflict
    );
    assert_eq!(policy, before);
    assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 10);
    validate(&policy).unwrap();
}

#[test]
fn cancel_quote_removes_and_refuses_replay() {
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.bread, 6);
    stock(&mut f.state, ESCROW_CHEST, f.emerald, 10);
    let mut policy = State::default();
    let request = quote_request(&f, "1", ItemStack::new(f.bread, 6), 10);
    assert!(run(&mut f.state, f.settlement, &mut policy, request));
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::CancelQuote { id: "1".into() }
    ));
    // Cancelled quotes retire: no stock moved, the id never trades again.
    assert!(!policy.quotes.contains_key("1"));
    assert_eq!(policy.completed_quote, 1);
    assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 0);
    assert_eq!(emerald_total(&f.state, ESCROW_CHEST), 10);
    let bread: i32 = f.state.block_entities
        [&(TREASURY_CHEST[0], TREASURY_CHEST[1], TREASURY_CHEST[2])]
        .slots
        .iter()
        .filter(|stack| stack.item_id == f.bread)
        .map(|stack| stack.count)
        .sum();
    assert_eq!(bread, 6);
    let seller_fence = fences(&f, &f.treasury);
    let buyer_fence = fences(&f, &f.escrow);
    assert_eq!(
        fail(
            &mut f.state,
            f.settlement,
            &mut policy,
            Request::AcceptQuote {
                id: "1".into(),
                seller_fence,
                buyer_fence,
            }
        ),
        Failure::NotFound
    );
    let request = quote_request(&f, "1", ItemStack::new(f.bread, 1), 1);
    assert_eq!(
        fail(&mut f.state, f.settlement, &mut policy, request),
        Failure::OperationConflict
    );
    validate(&policy).unwrap();
}

#[test]
fn sixty_five_sequential_purchases_first_id_replay_rejected() {
    let mut f = fixture();
    let mut policy = State::default();
    for round in 1..=65u64 {
        // Distinct simulation frames preserve the real native route budget.
        f.state.tick += 1;
        f.state.tick_budget.reset(f.state.tick);
        let id = round.to_string();
        stock(&mut f.state, TREASURY_CHEST, f.bread, 2);
        stock(&mut f.state, ESCROW_CHEST, f.emerald, 3);
        let request = quote_request(&f, &id, ItemStack::new(f.bread, 2), 3);
        assert!(
            run(&mut f.state, f.settlement, &mut policy, request),
            "round {round}"
        );
        // Mid-run serde restart: the watermark survives the journal.
        if round == 33 {
            let bytes = serde_json::to_vec(&policy).unwrap();
            policy = serde_json::from_slice(&bytes).unwrap();
        }
        let request = accept_request(&f, &id);
        assert!(
            run(&mut f.state, f.settlement, &mut policy, request),
            "round {round}"
        );
        assert!(policy.quotes.is_empty(), "round {round}");
        assert_eq!(policy.completed_quote, round, "round {round}");
    }
    // The FIRST id stays refused after 65 completions: the watermark is monotonic.
    let before = policy.clone();
    let request = quote_request(&f, "1", ItemStack::new(f.bread, 2), 3);
    assert_eq!(
        fail(&mut f.state, f.settlement, &mut policy, request),
        Failure::OperationConflict
    );
    assert_eq!(policy, before);
    // ... and across a serde restart too.
    let bytes = serde_json::to_vec(&policy).unwrap();
    let mut restored: State = serde_json::from_slice(&bytes).unwrap();
    let request = quote_request(&f, "1", ItemStack::new(f.bread, 2), 3);
    assert_eq!(
        fail(&mut f.state, f.settlement, &mut restored, request),
        Failure::OperationConflict
    );
    assert!(!active(&restored));
    validate(&restored).unwrap();
}

#[test]
fn hire_stop_dismiss_hire_first_contract_replay_rejected() {
    let mut f = fixture();
    f.state
        .block_entities
        .get_mut(&(TREASURY_CHEST[0], TREASURY_CHEST[1], TREASURY_CHEST[2]))
        .unwrap()
        .slots[..5]
        .fill(ItemStack::new(f.emerald, 64));
    let mut policy = State::default();
    for round in 1..=70u64 {
        let contract = round.to_string();
        hire(&mut f, &mut policy, &contract, 4, 4);
        // Mid-run serde restart: dismissal watermarks survive the journal.
        if round == 35 {
            let bytes = serde_json::to_vec(&policy).unwrap();
            policy = serde_json::from_slice(&bytes).unwrap();
        }
        earn_pay_stop_dismiss(&mut f, &mut policy, &contract);
        assert!(!policy.employments.contains_key(&contract), "round {round}");
        assert_eq!(policy.ended_employment, round, "round {round}");
    }
    // The FIRST contract stays refused after 70 dismissals.
    let before = policy.clone();
    let request = employ(&f, "1", 4, 5, 4);
    assert_eq!(
        fail(&mut f.state, f.settlement, &mut policy, request),
        Failure::OperationConflict
    );
    assert_eq!(policy, before);
    let bytes = serde_json::to_vec(&policy).unwrap();
    let mut restored: State = serde_json::from_slice(&bytes).unwrap();
    let request = employ(&f, "1", 4, 5, 4);
    assert_eq!(
        fail(&mut f.state, f.settlement, &mut restored, request),
        Failure::OperationConflict
    );
    assert!(!active(&restored));
    validate(&restored).unwrap();
}

#[test]
fn dismiss_requires_settled_debt() {
    let mut f = fixture();
    let mut policy = State::default();
    hire(&mut f, &mut policy, "1", 4, 12);
    f.state.tick += 5;
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    // Still active: dismissal is blocked, not silent.
    assert_eq!(
        fail(
            &mut f.state,
            f.settlement,
            &mut policy,
            Request::DismissEmployment {
                contract: "1".into(),
            }
        ),
        Failure::Blocked
    );
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::StopEmployment {
            contract: "1".into(),
        }
    ));
    // Unpaid debt blocks dismissal too: the worker is still owed.
    assert_eq!(
        fail(
            &mut f.state,
            f.settlement,
            &mut policy,
            Request::DismissEmployment {
                contract: "1".into(),
            }
        ),
        Failure::Blocked
    );
    validate(&policy).unwrap();
}

#[test]
fn policy_serde_restart_replay() {
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.emerald, 50);
    stock(&mut f.state, ESCROW_CHEST, f.emerald, 10);
    let mut policy = State::default();
    hire(&mut f, &mut policy, "7", 4, 12);
    // Serialize the live policy (as the WAL would) and restore it.
    let bytes = serde_json::to_vec(&policy).unwrap();
    let mut restored: State = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restored, policy);
    f.state.tick += 5;
    let op = pay_op("7", 1);
    let treasury = fences(&f, &f.treasury);
    let worker = fences(&f, &carry(f.worker_uuid));
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut restored,
        Request::Pay {
            contract: "7".into(),
            treasury_fence: treasury,
            worker_fence: worker,
            op: op.clone(),
        }
    ));
    // A restart replay of the settled op is idempotent; the watermark survives too.
    let bytes = serde_json::to_vec(&restored).unwrap();
    let mut again: State = serde_json::from_slice(&bytes).unwrap();
    let treasury_fence = fences(&f, &f.treasury);
    let worker_fence = fences(&f, &carry(f.worker_uuid));
    assert!(!run(
        &mut f.state,
        f.settlement,
        &mut again,
        Request::Pay {
            contract: "7".into(),
            treasury_fence,
            worker_fence,
            op,
        }
    ));
    validate(&again).unwrap();
}

#[test]
fn escrow_holds_money_and_wrong_shipment_never_pays() {
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.emerald, 10);
    let mut policy = State::default();
    let payer_fence = fences(&f, &f.treasury);
    assert!(run(
        &mut f.state,
        f.settlement,
        &mut policy,
        Request::EscrowReserve {
            id: "3".into(),
            shipment: "shipment-1".into(),
            payer: f.treasury.clone(),
            escrow: f.escrow.clone(),
            payee: carry(f.worker_uuid),
            price: 6,
            payer_fence,
        }
    ));
    // Only money moved payer -> escrow: no commodity was created or copied.
    assert_eq!(emerald_total(&f.state, TREASURY_CHEST), 4);
    assert_eq!(emerald_total(&f.state, ESCROW_CHEST), 6);
    assert!(active(&policy));
    // No genuine caravan delivery exists: payout is blocked, escrow untouched.
    let before = policy.clone();
    let escrow_fence = fences(&f, &f.escrow);
    let payee_fence = fences(&f, &carry(f.worker_uuid));
    assert_eq!(
        fail(
            &mut f.state,
            f.settlement,
            &mut policy,
            Request::EscrowPayout {
                id: "3".into(),
                op: payout_op("3"),
                escrow_fence,
                payee_fence,
            }
        ),
        Failure::Blocked
    );
    assert_eq!(policy, before);
    assert_eq!(emerald_total(&f.state, ESCROW_CHEST), 6);
    assert_eq!(carry_total(&f.state, f.worker), 0);
    validate(&policy).unwrap();
}

#[test]
fn owner_check_and_foreign_endpoints_refused() {
    let mut f = fixture();
    let mut policy = State::default();
    let outsider = stray(&mut f.state, [30.5, 201.0, 30.5]);
    let outsider_uuid = uuid_of(&f.state, outsider);
    let request = employ(&f, "99", 1, 5, 1);
    assert_eq!(
        execute(&mut f.state, f.settlement, "intruder", &mut policy, request).unwrap_err(),
        Failure::Forbidden
    );
    assert_eq!(
        fail(
            &mut f.state,
            f.settlement,
            &mut policy,
            Request::Employ {
                contract: "99".into(),
                worker: outsider_uuid,
                treasury: f.treasury.clone(),
                wage: 1,
                period: 5,
                budget: 5,
            }
        ),
        Failure::Forbidden
    );
    assert!(policy.employments.is_empty());
    validate(&policy).unwrap();
}

#[path = "commerce_fence_tests.rs"]
mod fence_tests;

#[path = "commerce_disk_tests.rs"]
mod disk_tests;

#[test]
fn unloaded_escrow_is_unknown_not_evidence_of_lost_money() {
    let mut f = fixture();
    stock(&mut f.state, TREASURY_CHEST, f.emerald, 10);
    let mut policy = State::default();
    let request = Request::EscrowReserve {
        id: "1".into(),
        shipment: "cold-route".into(),
        payer: f.treasury.clone(),
        escrow: f.escrow.clone(),
        payee: carry(f.worker_uuid),
        price: 4,
        payer_fence: fences(&f, &f.treasury),
    };
    run(&mut f.state, f.settlement, &mut policy, request);
    let before = policy.clone();
    // A restored policy can exist before its native chunks have loaded.
    let mut cold = state();
    assert!(!tick(&mut cold, f.settlement, &mut policy));
    assert_eq!(policy, before);
    assert_eq!(policy.mandates["1"].state, EscrowState::Escrowed);
    // Actual observation of an emptied loaded escrow is real loss evidence.
    stock(&mut f.state, ESCROW_CHEST, f.emerald, 0);
    assert!(tick(&mut f.state, f.settlement, &mut policy));
    assert_eq!(policy.mandates["1"].state, EscrowState::Lost);
}

#[test]
fn sequence_ids_are_canonical_positive_full_u64_values() {
    for bad in ["", "0", "01", "+1", "-1", "18446744073709551616"] {
        assert_eq!(sequence(bad), Err(Failure::InvalidRequest), "{bad}");
    }
    assert_eq!(sequence("1"), Ok(1));
    assert_eq!(sequence("18446744073709551615"), Ok(u64::MAX));
}
