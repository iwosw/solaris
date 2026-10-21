//! Native negotiated employment payroll, physical quote exchange and money-escrow paid shipments.
//!
//! Policy only: real stock lives in native inventories through the parent
//! `super::inventory` bridge (exact native UUID carry, owner-UUID player,
//! district-certified containers). Every admission and every mutation is
//! authorized by the parent `super::economy::authorize_endpoint`; money is
//! plain `minecraft:emerald` stacks, goods keep their full component identity.
//! Commodity motion belongs to caravan: escrow here holds MONEY (payer held),
//! never commodity, and payout needs genuine caravan delivery evidence.
//!
//! No lifetime tombstones: completed quotes, ended employments and settled
//! escrows leave the live sets; replay protection is a monotonic per-domain
//! watermark (`completed_seq` / `ended_seq` / `settled_seq` plus the last
//! completed id), never a retained record. Only native inventory capacity
//! and record-size fences remain; no arbitrary count or lifetime policy caps.
use std::collections::BTreeMap;

use mc_data::ItemStack;
use mc_script::{ScriptInventoryFence, ScriptOperationFailure as Failure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::inventory::{Change, Endpoint, commit, load};
use super::{SettlementId, economy};
use crate::tick::TickState;

/// Record ids are canonical positive decimal u64 sequence numbers (`1`, `2`, ...).
/// Creation requires an id newer than the monotonic retirement watermark.
/// Already-live older ids remain valid when a newer contract finishes first;
/// retiring them later never lowers the fence or permits replay of a removed id.
pub(crate) fn sequence(id: &str) -> Result<u64, Failure> {
    if id.starts_with('0') || id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Failure::InvalidRequest);
    }
    id.parse().map_err(|_| Failure::InvalidRequest)
}

/// Native operation id of one payroll attempt: per-contract strictly increasing.
/// A replay of a removed-but-settled payment is refused by the ended watermark.
pub(crate) fn pay_op(contract: &str, attempt: u64) -> String {
    format!("{contract}#{attempt}")
}

/// Native operation id of one escrow payout: exactly one payout ever exists.
pub(crate) fn payout_op(id: &str) -> String {
    format!("{id}#payout")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettledPayment {
    pub op: String,
    pub amount: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Employment {
    pub worker: Uuid,
    pub treasury: Endpoint,
    pub wage: u32,
    pub period_ticks: u64,
    pub next_due: u64,
    pub budget: u64,
    pub earned: u64,
    pub paid: u64,
    pub active: bool,
    pub attempt: u64,
    pub settled: Option<SettledPayment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Quote {
    pub seller: Endpoint,
    pub buyer: Endpoint,
    /// Full component identity; `count` is the total units to move.
    pub goods: ItemStack,
    /// Total plain emeralds the buyer pays.
    pub price: u32,
    pub seller_fence: ScriptInventoryFence,
    pub buyer_fence: ScriptInventoryFence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EscrowState {
    Escrowed,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EscrowMandate {
    /// Caravan shipment id; delivery evidence comes from live caravan state.
    pub shipment: String,
    pub payer: Endpoint,
    pub escrow: Endpoint,
    pub payee: Endpoint,
    pub price: u32,
    pub state: EscrowState,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    #[serde(default)]
    pub employments: BTreeMap<String, Employment>,
    #[serde(default)]
    pub quotes: BTreeMap<String, Quote>,
    #[serde(default)]
    pub mandates: BTreeMap<String, EscrowMandate>,
    /// Monotonic replay watermarks per domain. Retirement takes the maximum
    /// retired id; only creation requires an id strictly newer than its fence.
    #[serde(default)]
    pub ended_employment: u64,
    #[serde(default)]
    pub completed_quote: u64,
    #[serde(default)]
    pub settled_mandate: u64,
    #[serde(skip)]
    payroll_cursor: economy::WorkCursor,
    #[serde(skip)]
    escrow_cursor: economy::WorkCursor,
}

#[derive(Debug, Clone)]
pub(crate) enum Request {
    Employ {
        contract: String,
        worker: Uuid,
        treasury: Endpoint,
        wage: u32,
        period: u64,
        budget: u64,
    },
    StopEmployment {
        contract: String,
    },
    /// Remove a stopped, fully paid employment: frees hiring; refused on debt.
    DismissEmployment {
        contract: String,
    },
    Pay {
        contract: String,
        treasury_fence: ScriptInventoryFence,
        worker_fence: ScriptInventoryFence,
        op: String,
    },
    Quote {
        id: String,
        seller: Endpoint,
        buyer: Endpoint,
        goods: ItemStack,
        price: u32,
        seller_fence: ScriptInventoryFence,
        buyer_fence: ScriptInventoryFence,
    },
    AcceptQuote {
        id: String,
        seller_fence: ScriptInventoryFence,
        buyer_fence: ScriptInventoryFence,
    },
    /// Remove a pending quote without exchange; advances the quote watermark.
    CancelQuote {
        id: String,
    },
    EscrowReserve {
        id: String,
        shipment: String,
        payer: Endpoint,
        escrow: Endpoint,
        payee: Endpoint,
        price: u32,
        payer_fence: ScriptInventoryFence,
    },
    EscrowPayout {
        id: String,
        op: String,
        escrow_fence: ScriptInventoryFence,
        payee_fence: ScriptInventoryFence,
    },
}

fn retire_employment(state: &mut State, id: u64) {
    state.ended_employment = state.ended_employment.max(id);
}

fn retire_quote(state: &mut State, id: u64) {
    state.completed_quote = state.completed_quote.max(id);
}

fn retire_mandate(state: &mut State, id: u64) {
    state.settled_mandate = state.settled_mandate.max(id);
}

pub(crate) fn validate(state: &State) -> Result<(), &'static str> {
    for (id, e) in &state.employments {
        sequence(id).map_err(|_| "invalid employment id")?;
        if e.wage == 0 || e.period_ticks == 0 {
            return Err("invalid employment contract");
        }
        if e.paid > e.earned {
            return Err("employment paid exceeds earned");
        }
        if e.settled.is_some() != (e.attempt > 0)
            || e.settled
                .as_ref()
                .is_some_and(|s| s.op != pay_op(id, e.attempt) || u64::from(s.amount) > e.earned)
        {
            return Err("employment receipt out of order");
        }
        if e.treasury == (Endpoint::VillagerCarry { uuid: e.worker }) {
            return Err("treasury cannot be the worker's own pocket");
        }
    }
    for (id, q) in &state.quotes {
        sequence(id).map_err(|_| "invalid quote id")?;
        if q.seller == q.buyer || q.goods.count <= 0 || q.price == 0 {
            return Err("invalid trade quote");
        }
    }
    for (id, m) in &state.mandates {
        sequence(id).map_err(|_| "invalid escrow id")?;
        if m.shipment.is_empty() || m.price == 0 {
            return Err("invalid escrow mandate");
        }
        if m.escrow == m.payer || m.escrow == m.payee {
            return Err("escrow must hold money apart from payer and payee");
        }
    }
    Ok(())
}

/// True while payroll accrual or a money escrow needs future ticks.
/// Fully-paid stopped debt and settled escrows retire instead of ticking.
pub(crate) fn active(state: &State) -> bool {
    state
        .employments
        .values()
        .any(|e| e.active || e.paid < e.earned)
        || state
            .mandates
            .values()
            .any(|m| m.state == EscrowState::Escrowed)
}

fn emerald_id(state: &TickState) -> Result<u32, Failure> {
    let name =
        mc_data::Identifier::parse("minecraft:emerald").map_err(|_| Failure::InvalidRequest)?;
    state
        .registry
        .items
        .id_of(&name)
        .ok_or(Failure::InvalidRequest)
}

fn plain_emeralds(state: &TickState, slots: &[ItemStack]) -> Result<(ItemStack, u32), Failure> {
    let template = ItemStack::new(emerald_id(state)?, 1);
    let mut total = 0u32;
    for slot in slots {
        if slot.is_empty() || !mc_data::inventory_semantics_26_1_2::can_stack(&template, slot) {
            continue;
        }
        total = total
            .checked_add(slot.count.max(0) as u32)
            .ok_or(Failure::Capacity)?;
    }
    Ok((template, total))
}

/// Remove `need` identity-matching units; stock shortfall changes nothing.
fn take_units(slots: &mut [ItemStack], template: &ItemStack, mut need: u32) -> Result<(), Failure> {
    for slot in slots.iter_mut() {
        if need == 0 {
            break;
        }
        if slot.is_empty() || !mc_data::inventory_semantics_26_1_2::can_stack(template, slot) {
            continue;
        }
        let take = (slot.count.max(0) as u32).min(need);
        slot.count -= take as i32;
        need -= take;
        if slot.count <= 0 {
            *slot = ItemStack::EMPTY;
        }
    }
    if need > 0 {
        return Err(Failure::InsufficientItems);
    }
    Ok(())
}

/// Merge into identity-matching stacks first, then empty slots. Overflow changes nothing.
fn add_units(
    state: &TickState,
    slots: &mut [ItemStack],
    template: &ItemStack,
    mut need: u32,
) -> Result<(), Failure> {
    let maximum = state.registry.item_rules().max_stack(template);
    for slot in slots.iter_mut() {
        if need == 0 {
            break;
        }
        if !mc_data::inventory_semantics_26_1_2::can_stack(template, slot) {
            continue;
        }
        let room = (maximum - slot.count.max(0)).max(0) as u32;
        let put = room.min(need);
        slot.count += put as i32;
        need -= put;
    }
    for slot in slots.iter_mut() {
        if need == 0 {
            break;
        }
        if !slot.is_empty() {
            continue;
        }
        let put = (maximum.max(1) as u32).min(need);
        let mut stack = template.clone();
        stack.count = put as i32;
        *slot = stack;
        need -= put;
    }
    if need > 0 {
        return Err(Failure::Capacity);
    }
    Ok(())
}

fn projected_accrual(employment: &Employment, tick: u64) -> Option<(u64, u64)> {
    if !employment.active || tick < employment.next_due {
        return Some((employment.earned, employment.next_due));
    }
    let periods = (tick - employment.next_due)
        .checked_div(employment.period_ticks)?
        .checked_add(1)?;
    let earned = periods
        .checked_mul(u64::from(employment.wage))?
        .checked_add(employment.earned)?;
    let due = periods
        .checked_mul(employment.period_ticks)?
        .checked_add(employment.next_due)?;
    Some((earned, due))
}

fn accrue(employment: &mut Employment, tick: u64) -> bool {
    let Some((earned, due)) = projected_accrual(employment, tick) else {
        return false;
    };
    let changed = earned != employment.earned || due != employment.next_due;
    employment.earned = earned;
    employment.next_due = due;
    changed
}

fn authorize(
    world: &mut TickState,
    settlement: SettlementId,
    endpoint: &Endpoint,
) -> Result<(), Failure> {
    economy::authorize_endpoint(world, settlement, endpoint)
}

fn fresh(
    world: &TickState,
    endpoint: &Endpoint,
    fence: &ScriptInventoryFence,
) -> Result<super::inventory::Image, Failure> {
    let image = load(world, endpoint)?;
    if image.fence != *fence {
        return Err(Failure::StaleRevision);
    }
    Ok(image)
}

struct EmploymentTerms {
    worker: Uuid,
    treasury: Endpoint,
    wage: u32,
    period: u64,
    budget: u64,
}

struct QuoteTerms {
    seller: Endpoint,
    buyer: Endpoint,
    goods: ItemStack,
    price: u32,
    seller_fence: ScriptInventoryFence,
    buyer_fence: ScriptInventoryFence,
}

struct EscrowTerms {
    shipment: String,
    payer: Endpoint,
    escrow: Endpoint,
    payee: Endpoint,
    price: u32,
    payer_fence: ScriptInventoryFence,
}

fn employ(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    contract: String,
    terms: EmploymentTerms,
) -> Result<bool, Failure> {
    let EmploymentTerms {
        worker,
        treasury,
        wage,
        period,
        budget,
    } = terms;
    let seq = sequence(&contract)?;
    if wage == 0 || period == 0 || budget == 0 {
        return Err(Failure::InvalidRequest);
    }
    if world.tick.checked_add(period).is_none() {
        return Err(Failure::InvalidRequest);
    }
    if state.employments.contains_key(&contract) || seq <= state.ended_employment {
        return Err(Failure::OperationConflict);
    }
    if treasury == (Endpoint::VillagerCarry { uuid: worker }) {
        return Err(Failure::InvalidRequest);
    }
    authorize(world, settlement, &treasury)?;
    authorize(world, settlement, &Endpoint::VillagerCarry { uuid: worker })?;
    let next_due = world.tick.saturating_add(period);
    state.employments.insert(
        contract,
        Employment {
            worker,
            treasury,
            wage,
            period_ticks: period,
            next_due,
            budget,
            earned: 0,
            paid: 0,
            active: true,
            attempt: 0,
            settled: None,
        },
    );
    Ok(true)
}

fn stop_employment(state: &mut State, contract: &str, tick: u64) -> Result<bool, Failure> {
    let employment = state
        .employments
        .get_mut(contract)
        .ok_or(Failure::NotFound)?;
    if !employment.active {
        return Ok(false);
    }
    accrue(employment, tick);
    // Stopping ends accrual; earned-but-unpaid debt is preserved personal property.
    employment.active = false;
    Ok(true)
}

/// Fully-paid stopped contracts retire: the hiring slot is freed and the
/// ended watermark refuses replays without retaining the record.
fn dismiss_employment(state: &mut State, contract: &str) -> Result<bool, Failure> {
    let seq = sequence(contract)?;
    let employment = state.employments.get(contract).ok_or(Failure::NotFound)?;
    if employment.active || employment.paid != employment.earned {
        return Err(Failure::Blocked);
    }
    state.employments.remove(contract);
    retire_employment(state, seq);
    Ok(true)
}

/// Both autonomous and explicit payments use this same native atomic owner.
fn transfer_money(
    world: &mut TickState,
    settlement: SettlementId,
    source: &Endpoint,
    destination: &Endpoint,
    amount: u32,
    observed: Option<(ScriptInventoryFence, ScriptInventoryFence)>,
) -> Result<(), Failure> {
    authorize(world, settlement, source)?;
    authorize(world, settlement, destination)?;
    let source_image = load(world, source)?;
    let destination_image = load(world, destination)?;
    if observed.is_some_and(|(source, destination)| {
        source_image.fence != source || destination_image.fence != destination
    }) {
        return Err(Failure::StaleRevision);
    }
    let (emerald, held) = plain_emeralds(world, &source_image.slots)?;
    if held < amount {
        return Err(Failure::InsufficientItems);
    }
    let mut source_slots = source_image.slots.clone();
    take_units(&mut source_slots, &emerald, amount)?;
    let mut destination_slots = destination_image.slots.clone();
    add_units(world, &mut destination_slots, &emerald, amount)?;
    commit(
        world,
        vec![
            Change {
                before: source_image,
                after: source_slots,
            },
            Change {
                before: destination_image,
                after: destination_slots,
            },
        ],
    )?;
    Ok(())
}

fn settle_payroll(
    world: &mut TickState,
    settlement: SettlementId,
    employment: &mut Employment,
    attempt: u64,
    op: String,
    observed: Option<(ScriptInventoryFence, ScriptInventoryFence)>,
) -> Result<(), Failure> {
    let (earned, due) = projected_accrual(employment, world.tick).ok_or(Failure::Capacity)?;
    let unpaid = earned
        .checked_sub(employment.paid)
        .ok_or(Failure::InvalidRequest)?;
    if unpaid > employment.budget {
        return Err(Failure::InsufficientItems);
    }
    let amount = u32::try_from(unpaid).map_err(|_| Failure::Capacity)?;
    if amount == 0 {
        return Err(Failure::InvalidRequest);
    }
    transfer_money(
        world,
        settlement,
        &employment.treasury,
        &Endpoint::VillagerCarry {
            uuid: employment.worker,
        },
        amount,
        observed,
    )?;
    employment.earned = earned;
    employment.next_due = due;
    employment.paid += u64::from(amount);
    employment.budget -= u64::from(amount);
    employment.attempt = attempt;
    employment.settled = Some(SettledPayment { op, amount });
    Ok(())
}

fn pay(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    contract: String,
    treasury_fence: ScriptInventoryFence,
    worker_fence: ScriptInventoryFence,
    op: String,
) -> Result<bool, Failure> {
    let employment = state
        .employments
        .get_mut(&contract)
        .ok_or(Failure::NotFound)?;
    if employment
        .settled
        .as_ref()
        .is_some_and(|settled| settled.op == op)
    {
        return Ok(false);
    }
    let attempt = employment.attempt.checked_add(1).ok_or(Failure::Capacity)?;
    let (owner, requested) = op.rsplit_once('#').ok_or(Failure::OperationConflict)?;
    if owner != contract || sequence(requested).ok() != Some(attempt) {
        return Err(Failure::OperationConflict);
    }
    settle_payroll(
        world,
        settlement,
        employment,
        attempt,
        op,
        Some((treasury_fence, worker_fence)),
    )?;
    Ok(true)
}

fn quote(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    id: String,
    terms: QuoteTerms,
) -> Result<bool, Failure> {
    let QuoteTerms {
        seller,
        buyer,
        goods,
        price,
        seller_fence,
        buyer_fence,
    } = terms;
    let seq = sequence(&id)?;
    if seller == buyer || goods.count <= 0 || price == 0 {
        return Err(Failure::InvalidRequest);
    }
    if goods.item_id == 0 || world.registry.items.name_of(goods.item_id).is_none() {
        return Err(Failure::InvalidRequest);
    }
    if state.quotes.contains_key(&id) || seq <= state.completed_quote {
        return Err(Failure::OperationConflict);
    }
    authorize(world, settlement, &seller)?;
    authorize(world, settlement, &buyer)?;
    // Both sides prove a fresh observation; the fences bind the accepted exchange.
    fresh(world, &seller, &seller_fence)?;
    fresh(world, &buyer, &buyer_fence)?;
    state.quotes.insert(
        id,
        Quote {
            seller,
            buyer,
            goods,
            price,
            seller_fence,
            buyer_fence,
        },
    );
    Ok(true)
}

/// A committed quote retires with its watermark: no second exchange, ever.
fn accept_quote(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    id: String,
    seller_fence: ScriptInventoryFence,
    buyer_fence: ScriptInventoryFence,
) -> Result<bool, Failure> {
    let quoted = state.quotes.get(&id).ok_or(Failure::NotFound)?;
    // Acceptance cites the quoted observation; anything else is a conflicting replay.
    if quoted.seller_fence != seller_fence || quoted.buyer_fence != buyer_fence {
        return Err(Failure::OperationConflict);
    }
    authorize(world, settlement, &quoted.seller)?;
    authorize(world, settlement, &quoted.buyer)?;
    let seller_image = fresh(world, &quoted.seller, &seller_fence)?;
    let buyer_image = fresh(world, &quoted.buyer, &buyer_fence)?;
    let goods = quoted.goods.clone();
    let price = quoted.price;
    let need = goods.count.max(0) as u32;
    let mut seller_slots = seller_image.slots.clone();
    take_units(&mut seller_slots, &goods, need)?;
    let (emerald, held) = plain_emeralds(world, &buyer_image.slots)?;
    if held < price {
        return Err(Failure::InsufficientItems);
    }
    let mut buyer_slots = buyer_image.slots.clone();
    take_units(&mut buyer_slots, &emerald, price)?;
    let mut seller_pay = seller_slots.clone();
    add_units(world, &mut seller_pay, &emerald, price)?;
    let mut buyer_goods = buyer_slots.clone();
    let mut unit = goods.clone();
    unit.count = 1;
    add_units(world, &mut buyer_goods, &unit, need)?;
    commit(
        world,
        vec![
            Change {
                before: seller_image,
                after: seller_pay,
            },
            Change {
                before: buyer_image,
                after: buyer_goods,
            },
        ],
    )?;
    let seq = sequence(&id)?;
    state.quotes.remove(&id);
    retire_quote(state, seq);
    Ok(true)
}

/// A cancelled quote retires with its watermark: the id never trades again.
fn cancel_quote(state: &mut State, id: &str) -> Result<bool, Failure> {
    let seq = sequence(id)?;
    if !state.quotes.contains_key(id) {
        return Err(Failure::NotFound);
    }
    state.quotes.remove(id);
    retire_quote(state, seq);
    Ok(true)
}

fn escrow_reserve(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    id: String,
    terms: EscrowTerms,
) -> Result<bool, Failure> {
    let EscrowTerms {
        shipment,
        payer,
        escrow,
        payee,
        price,
        payer_fence,
    } = terms;
    let seq = sequence(&id)?;
    if shipment.is_empty() || price == 0 {
        return Err(Failure::InvalidRequest);
    }
    if escrow == payer || escrow == payee {
        return Err(Failure::InvalidRequest);
    }
    if state.mandates.contains_key(&id) || seq <= state.settled_mandate {
        return Err(Failure::OperationConflict);
    }
    authorize(world, settlement, &payer)?;
    authorize(world, settlement, &escrow)?;
    authorize(world, settlement, &payee)?;
    let payer_image = fresh(world, &payer, &payer_fence)?;
    let escrow_image = load(world, &escrow)?;
    let (emerald, held) = plain_emeralds(world, &payer_image.slots)?;
    if held < price {
        return Err(Failure::InsufficientItems);
    }
    // Only money moves: payer to escrow. Commodity travels with the caravan.
    let mut payer_slots = payer_image.slots.clone();
    take_units(&mut payer_slots, &emerald, price)?;
    let mut escrow_slots = escrow_image.slots.clone();
    add_units(world, &mut escrow_slots, &emerald, price)?;
    commit(
        world,
        vec![
            Change {
                before: payer_image,
                after: payer_slots,
            },
            Change {
                before: escrow_image,
                after: escrow_slots,
            },
        ],
    )?;
    state.mandates.insert(
        id,
        EscrowMandate {
            shipment,
            payer,
            escrow,
            payee,
            price,
            state: EscrowState::Escrowed,
        },
    );
    Ok(true)
}

fn delivered(
    world: &TickState,
    settlement: SettlementId,
    shipment: &str,
    mandate: &str,
) -> Result<bool, Failure> {
    let record = world.settlements.get(settlement).ok_or(Failure::NotFound)?;
    Ok(super::caravan::delivered(&record.economy.caravan, shipment)
        && record
            .economy
            .caravan
            .shipments
            .get(shipment)
            .is_some_and(|shipment| shipment.paid_mandate.as_deref() == Some(mandate)))
}

/// A paid escrow retires with its watermark; a looted/emptied escrow is Lost.
fn escrow_payout(
    world: &mut TickState,
    settlement: SettlementId,
    state: &mut State,
    id: String,
    op: String,
    escrow_fence: ScriptInventoryFence,
    payee_fence: ScriptInventoryFence,
) -> Result<bool, Failure> {
    let seq = sequence(&id)?;
    let mandate = state.mandates.get(&id).ok_or(Failure::NotFound)?;
    if op != payout_op(&id) {
        return Err(Failure::OperationConflict);
    }
    settle_mandate(
        world,
        settlement,
        &id,
        mandate,
        Some((escrow_fence, payee_fence)),
    )?;
    state.mandates.remove(&id);
    retire_mandate(state, seq);
    Ok(true)
}

fn settle_mandate(
    world: &mut TickState,
    settlement: SettlementId,
    id: &str,
    mandate: &EscrowMandate,
    observed: Option<(ScriptInventoryFence, ScriptInventoryFence)>,
) -> Result<(), Failure> {
    if mandate.state == EscrowState::Lost {
        return Err(Failure::OperationConflict);
    }
    if !delivered(world, settlement, &mandate.shipment, id)? {
        return Err(Failure::Blocked);
    }
    transfer_money(
        world,
        settlement,
        &mandate.escrow,
        &mandate.payee,
        mandate.price,
        observed,
    )
}

pub(crate) fn execute(
    world: &mut TickState,
    settlement: SettlementId,
    owner: &str,
    state: &mut State,
    request: Request,
) -> Result<bool, Failure> {
    if world
        .settlements
        .get(settlement)
        .ok_or(Failure::NotFound)?
        .owner
        .as_deref()
        != Some(owner)
    {
        return Err(Failure::Forbidden);
    }
    // Member/container/player authorization is re-checked inside every mutating helper.
    match request {
        Request::Employ {
            contract,
            worker,
            treasury,
            wage,
            period,
            budget,
        } => employ(
            world,
            settlement,
            state,
            contract,
            EmploymentTerms {
                worker,
                treasury,
                wage,
                period,
                budget,
            },
        ),
        Request::StopEmployment { contract } => stop_employment(state, &contract, world.tick),
        Request::DismissEmployment { contract } => dismiss_employment(state, &contract),
        Request::Pay {
            contract,
            treasury_fence,
            worker_fence,
            op,
        } => pay(
            world,
            settlement,
            state,
            contract,
            treasury_fence,
            worker_fence,
            op,
        ),
        Request::Quote {
            id,
            seller,
            buyer,
            goods,
            price,
            seller_fence,
            buyer_fence,
        } => quote(
            world,
            settlement,
            state,
            id,
            QuoteTerms {
                seller,
                buyer,
                goods,
                price,
                seller_fence,
                buyer_fence,
            },
        ),
        Request::AcceptQuote {
            id,
            seller_fence,
            buyer_fence,
        } => accept_quote(world, settlement, state, id, seller_fence, buyer_fence),
        Request::CancelQuote { id } => cancel_quote(state, &id),
        Request::EscrowReserve {
            id,
            shipment,
            payer,
            escrow,
            payee,
            price,
            payer_fence,
        } => escrow_reserve(
            world,
            settlement,
            state,
            id,
            EscrowTerms {
                shipment,
                payer,
                escrow,
                payee,
                price,
                payer_fence,
            },
        ),
        Request::EscrowPayout {
            id,
            op,
            escrow_fence,
            payee_fence,
        } => escrow_payout(world, settlement, state, id, op, escrow_fence, payee_fence),
    }
}

/// The simulation owns due wages and delivery-gated escrow payout. Commands
/// may request the same physical effects, but are never required to drive them.
/// Existing next_due is also the retry cadence for stopped, still-owed wages.
pub(crate) fn tick(world: &mut TickState, settlement: SettlementId, state: &mut State) -> bool {
    let now = world.tick;
    let mut changed = state.payroll_cursor.run(
        &mut state.employments,
        |employment| {
            (employment.active || employment.paid < employment.earned) && employment.next_due <= now
        },
        |id, employment| {
            let mut changed = accrue(employment, now);
            if !employment.active {
                let periods = (now - employment.next_due) / employment.period_ticks + 1;
                if let Some(due) = periods
                    .checked_mul(employment.period_ticks)
                    .and_then(|skip| employment.next_due.checked_add(skip))
                {
                    employment.next_due = due;
                    changed = true;
                }
            }
            let owed = employment.earned - employment.paid;
            if owed > 0
                && owed <= employment.budget
                && let Some(attempt) = employment.attempt.checked_add(1)
            {
                changed |= settle_payroll(
                    world,
                    settlement,
                    employment,
                    attempt,
                    pay_op(id, attempt),
                    None,
                )
                .is_ok();
            }
            changed.into()
        },
    );
    let mut retired = state.settled_mandate;
    changed |= state.escrow_cursor.run(
        &mut state.mandates,
        |mandate| mandate.state == EscrowState::Escrowed,
        |id, mandate| {
            if delivered(world, settlement, &mandate.shipment, id) == Ok(true) {
                match settle_mandate(world, settlement, id, mandate, None) {
                    Ok(()) => {
                        retired = retired.max(sequence(id).expect("validated escrow identity"));
                        return economy::Work::Remove;
                    }
                    Err(Failure::InsufficientItems) => {
                        mandate.state = EscrowState::Lost;
                        return economy::Work::Changed;
                    }
                    Err(_) => return economy::Work::Unchanged,
                }
            }
            // Unreadable/unloaded native stock is unknown, not theft evidence.
            if let Ok(image) = load(world, &mandate.escrow)
                && let Ok((_, held)) = plain_emeralds(world, &image.slots)
                && held < mandate.price
            {
                mandate.state = EscrowState::Lost;
                return economy::Work::Changed;
            }
            economy::Work::Unchanged
        },
    );
    state.settled_mandate = retired;
    changed
}

#[cfg(test)]
#[path = "commerce_tests.rs"]
mod tests;
