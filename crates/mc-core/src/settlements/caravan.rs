//! Native physical caravan shipments: real source pickup into the exact living
//! carrier's native carry slots, native brain walking between finite waypoints,
//! arrival only from the real body pose, unload only of the actually carried
//! manifest. Money escrow lives in commerce; this module moves commodities only.
//!
//! Policy only: the immutable contract plus receipts serialize. Native stock,
//! bodies and poses never do. Every mutation is atomic through the shared
//! [`super::inventory`] bridge: all fences and capacity validate before any
//! native write, so a refusal never half-moves cargo.

use std::collections::BTreeMap;

use mc_data::ItemStack;
use mc_data::inventory_semantics_26_1_2::can_stack;
use mc_script::{
    ScriptInventoryFence, ScriptOperationFailure as Failure, ScriptSettlementBuildingRef,
    ScriptWorksiteInteractionPoint,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::SettlementId;
use super::inventory::{Change, Endpoint, commit, load};
use crate::boundary::BlockPos;
use crate::tick::TickState;

/// Delivered journals persist until the controller retires them; only active
/// shipments count against the per-tick work budget. No lifetime policy cap:
/// receipts and replay identity are never evicted by a count.
/// Finite route cells per shipment.
const MAX_WAYPOINTS: usize = 16;
/// Native admission bound per route segment, in cells.
const MAX_SEGMENT_CELLS: i64 = 128;
/// Largest single shipment; real carry capacity (`<=8` native slots) rejects
/// more at pickup plan time.
const MAX_QUANTITY: u32 = 4096;
/// Total route budget, in ticks.
const MAX_ROUTE_TICKS: u64 = 72_000;
/// Native villager carry slots.
const CARRY_SLOTS: usize = 8;
/// One native atomic transfer admits at most sixteen portions.
const MAX_DEPOSITS: usize = 16;
/// Terminal block reasons stay short enough for any durable value.
pub(crate) const MAX_BLOCKED_BYTES: usize = 192;

/// A certified native container: the building certificate bound in the live
/// district plus the exact native container cell that must match one of its
/// certified container points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContainerRef {
    pub building: ScriptSettlementBuildingRef,
    pub dimension: String,
    pub position: [i32; 3],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Status {
    Loading,
    Transit,
    Arrived,
    Blocked,
    Delivered,
    Lost,
    Cancelled,
}

/// Exact-once tombstone for one shipment effect: one load and one unload ever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpReceipt {
    pub op: String,
    pub epoch: u64,
}

/// One exact lot of the manifest inside a native carry slot. `count` never
/// exceeds one native stack: `plan_pickup` caps every lot at `max_stack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CarryLot {
    pub slot: u8,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Shipment {
    /// Immutable contract, fixed at admission and never mutated after.
    pub id: String,
    pub owner: String,
    pub settlement: SettlementId,
    pub source: ContainerRef,
    pub destination: ContainerRef,
    pub carrier: Uuid,
    /// Component-exact cargo identity; `count` always equals `quantity`.
    pub manifest: ItemStack,
    pub quantity: u32,
    pub waypoints: Vec<[i32; 3]>,
    /// Admission tick: the immutable route epoch in every receipt op id.
    pub epoch: u64,
    pub max_ticks: u64,
    pub paid_mandate: Option<String>,
    /// Fences captured at admission; any unrelated stock change afterwards
    /// blocks instead of moving wrong cargo.
    pub source_fence: ScriptInventoryFence,
    pub destination_fence: ScriptInventoryFence,
    pub carrier_fence: ScriptInventoryFence,
    /// Mutable progress below.
    pub status: Status,
    pub blocked: Option<String>,
    pub cursor: usize,
    pub ticks: u64,
    pub attempt: u64,
    pub loaded: bool,
    pub carry: Vec<CarryLot>,
    /// Expected carrier fence after our own pickup; the only fence we advance.
    pub carrier_live_fence: Option<ScriptInventoryFence>,
    pub load: Option<OpReceipt>,
    pub unload: Option<OpReceipt>,
    pub delivery: Option<OpReceipt>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    #[serde(default)]
    pub shipments: BTreeMap<String, Shipment>,
    #[serde(skip)]
    cursor: super::economy::WorkCursor,
}

#[derive(Debug, Clone)]
pub(crate) struct Open {
    pub(crate) id: String,
    pub(crate) source: ContainerRef,
    pub(crate) destination: ContainerRef,
    pub(crate) carrier: Uuid,
    pub(crate) manifest: ItemStack,
    pub(crate) quantity: u32,
    pub(crate) waypoints: Vec<[i32; 3]>,
    pub(crate) max_ticks: u64,
    pub(crate) paid_mandate: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Request {
    Open(Box<Open>),
    Pickup { id: String },
    Unload { id: String },
    Block { id: String, reason: String },
    Cancel { id: String },
}

fn terminal(status: Status) -> bool {
    matches!(
        status,
        Status::Blocked | Status::Delivered | Status::Lost | Status::Cancelled
    )
}

/// True while any shipment still awaits real work. Terminal journals never
/// re-enter the tick loop.
pub(crate) fn active(state: &State) -> bool {
    state.shipments.values().any(|s| {
        matches!(
            s.status,
            Status::Loading | Status::Transit | Status::Arrived
        )
    })
}

/// Commerce payout signal: true only for a genuine committed delivery.
/// Lost, Blocked and Cancelled shipments never report delivered.
pub(crate) fn delivered(state: &State, id: &str) -> bool {
    state
        .shipments
        .get(id)
        .is_some_and(|s| s.status == Status::Delivered && s.delivery.is_some() && !s.loaded)
}

/// Native brain routing target for one carrier: the next real stance it must
/// occupy. Nothing synthetic: the body walks, the policy only names cells.
#[cfg(test)]
pub(crate) fn target(state: &State, worker: Uuid) -> Option<BlockPos> {
    state
        .shipments
        .values()
        .filter(|shipment| shipment.carrier == worker)
        .find_map(target_for_shipment)
}

pub(super) fn target_for_shipment(shipment: &Shipment) -> Option<BlockPos> {
    if !matches!(
        shipment.status,
        Status::Loading | Status::Transit | Status::Arrived
    ) {
        return None;
    }
    let cell = match shipment.status {
        Status::Loading => shipment.waypoints.first().copied()?,
        Status::Transit => shipment
            .waypoints
            .get(shipment.cursor)
            .or_else(|| shipment.waypoints.last())
            .copied()?,
        _ => shipment.waypoints.last().copied()?,
    };
    Some(BlockPos {
        x: cell[0],
        y: cell[1],
        z: cell[2],
    })
}

fn beside(a: [i32; 3], b: [i32; 3]) -> bool {
    a[1] == b[1]
        && (i64::from(a[0]) - i64::from(b[0])).abs() + (i64::from(a[2]) - i64::from(b[2])).abs()
            == 1
}

fn validate_route(
    waypoints: &[[i32; 3]],
    source: [i32; 3],
    destination: [i32; 3],
) -> Result<(), &'static str> {
    if !(2..=MAX_WAYPOINTS).contains(&waypoints.len()) {
        return Err("route needs 2..=16 finite physical waypoints");
    }
    for pair in waypoints.windows(2) {
        if pair[0] == pair[1] {
            return Err("adjacent route waypoints must differ");
        }
        for (from, to) in pair[0].iter().zip(pair[1].iter()) {
            if (i64::from(*from) - i64::from(*to)).abs() > MAX_SEGMENT_CELLS {
                return Err("adjacent route segments must fit native 128-cell admission");
            }
        }
    }
    if !beside(waypoints[0], source) || !beside(waypoints[waypoints.len() - 1], destination) {
        return Err("physical source and destination stances required");
    }
    Ok(())
}

fn validate_shipment(shipment: &Shipment) -> Result<(), &'static str> {
    if shipment.id.is_empty() || shipment.id.len() > mc_script::MAX_SCRIPT_ID_BYTES {
        return Err("invalid shipment identity");
    }
    if shipment.owner.is_empty() || shipment.owner.len() > 128 {
        return Err("invalid shipment owner");
    }
    if shipment.settlement.0 == 0 {
        return Err("invalid shipment settlement");
    }
    if shipment.quantity == 0 || shipment.quantity > MAX_QUANTITY {
        return Err("shipment quantity must fit one native admission");
    }
    if shipment.manifest.is_empty()
        || u64::from(shipment.quantity) > u64::from(i32::MAX as u32)
        || shipment.manifest.count != shipment.quantity as i32
    {
        return Err("component-exact manifest must cite the exact quantity");
    }
    validate_route(
        &shipment.waypoints,
        shipment.source.position,
        shipment.destination.position,
    )?;
    if shipment.source.position == shipment.destination.position
        && shipment.source.building == shipment.destination.building
    {
        return Err("source and destination must differ");
    }
    if shipment.max_ticks == 0 || shipment.max_ticks > MAX_ROUTE_TICKS {
        return Err("finite route tick budget required");
    }
    if shipment
        .paid_mandate
        .as_ref()
        .is_some_and(|id| super::commerce::sequence(id).is_err())
    {
        return Err("invalid paid mandate reference");
    }
    for fence in [
        &shipment.source_fence,
        &shipment.destination_fence,
        &shipment.carrier_fence,
    ]
    .into_iter()
    .chain(shipment.carrier_live_fence.as_ref())
    {
        fence
            .validate()
            .map_err(|_| "invalid native inventory fence")?;
    }
    if shipment.epoch > mc_script::MAX_SCRIPT_WORLD_TIME {
        return Err("invalid route epoch");
    }
    if shipment.cursor > shipment.waypoints.len() {
        return Err("shipment cursor beyond its finite route");
    }
    let mut carried = 0_u32;
    let mut occupied = 0_u8;
    for lot in &shipment.carry {
        if lot.count == 0 || usize::from(lot.slot) >= CARRY_SLOTS || occupied & (1 << lot.slot) != 0
        {
            return Err("invalid carried lots");
        }
        occupied |= 1 << lot.slot;
        carried = carried
            .checked_add(lot.count)
            .ok_or("invalid carried lots")?;
    }
    if shipment.loaded {
        if shipment.load.is_none() || carried != shipment.quantity {
            return Err("loaded cargo must cite its exact receipted lots");
        }
    } else if !shipment.carry.is_empty() {
        return Err("unloaded shipment still cites held cargo");
    }
    if shipment.unload.is_some() && shipment.load.is_none() {
        return Err("unload without load is impossible");
    }
    if shipment.delivery.is_some()
        && (shipment.status != Status::Delivered
            || shipment.unload.is_none()
            || !shipment.carry.is_empty())
    {
        return Err("delivery is a terminal empty-handed receipt only");
    }
    for receipt in [&shipment.load, &shipment.unload, &shipment.delivery]
        .into_iter()
        .flatten()
    {
        if receipt.op.is_empty() || receipt.op.len() > 128 {
            return Err("invalid shipment receipt");
        }
        if receipt.epoch > mc_script::MAX_SCRIPT_WORLD_TIME {
            return Err("invalid shipment receipt epoch");
        }
    }
    match shipment.status {
        Status::Loading => {
            if shipment.loaded
                || shipment.load.is_some()
                || !shipment.carry.is_empty()
                || shipment.cursor != 0
                || shipment.delivery.is_some()
            {
                return Err("loading shipment holds no cargo yet");
            }
        }
        Status::Transit => {
            if !shipment.loaded
                || shipment.load.is_none()
                || shipment.delivery.is_some()
                || shipment.cursor == 0
                || shipment.cursor >= shipment.waypoints.len()
            {
                return Err("transit shipment carries cargo along its route");
            }
        }
        Status::Arrived => {
            if !shipment.loaded
                || shipment.load.is_none()
                || shipment.delivery.is_some()
                || shipment.cursor != shipment.waypoints.len()
            {
                return Err("arrived shipment carries cargo at its destination");
            }
        }
        Status::Blocked => {
            if shipment.blocked.as_ref().is_none_or(String::is_empty) {
                return Err("blocked shipment cites its reason");
            }
            if shipment.delivery.is_some() {
                return Err("blocked shipment never delivers");
            }
        }
        Status::Delivered => {
            if shipment.delivery.is_none()
                || shipment.unload.is_none()
                || !shipment.carry.is_empty()
                || shipment.loaded
            {
                return Err("delivery is a terminal empty-handed receipt only");
            }
        }
        Status::Lost | Status::Cancelled => {
            if shipment.delivery.is_some() {
                return Err("only a delivery delivers");
            }
        }
    }
    Ok(())
}

pub(crate) fn validate(state: &State) -> Result<(), &'static str> {
    for (id, shipment) in &state.shipments {
        if id != &shipment.id {
            return Err("invalid shipment map identity");
        }
        validate_shipment(shipment)?;
    }
    Ok(())
}

enum Fault {
    Gone,
    Away,
}

/// The exact live body cell of the carrier, or why there is none. Dead and
/// removed carriers are both gone: loss, never a refund. A living carrier
/// outside the settlement is away: blocked, cargo stays where it is.
fn live_cell(
    world: &TickState,
    settlement: SettlementId,
    carrier: Uuid,
) -> Result<[i32; 3], Fault> {
    let slot = world.entities.slot_of_uuid(carrier).ok_or(Fault::Gone)?;
    let body = world.entities.body(slot);
    let row = world.entities.row(slot);
    if !body.alive {
        return Err(Fault::Gone);
    }
    if row.villager.as_ref().is_none_or(|v| {
        v.settlement != Some(settlement) || v.wandering || v.conversion_time.is_some()
    }) {
        return Err(Fault::Away);
    }
    let pos = world.entities.pos(slot);
    Ok([
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    ])
}

fn container_endpoint(reference: &ContainerRef) -> Endpoint {
    Endpoint::Container {
        dimension: reference.dimension.clone(),
        position: reference.position,
    }
}

/// The requested native cell must be one of the named certificate's actual
/// certified container cells: no caller-supplied arbitrary cert, no saved
/// readiness projection.
fn certified_container(
    world: &TickState,
    facts: &crate::plugin_structures::BuildingFacts,
    position: [i32; 3],
) -> Result<bool, Failure> {
    for point in &facts.points {
        if let ScriptWorksiteInteractionPoint::Container { at } = point {
            let anchor = mc_script::ScriptBlockPosition::new(at[0], at[1], at[2]);
            for cell in crate::plugin_inventories::container_cells(world, &anchor)? {
                if [cell.x, cell.y, cell.z] == position {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn block(shipment: &mut Shipment, reason: &str) {
    let mut reason = reason.to_owned();
    if reason.len() > MAX_BLOCKED_BYTES {
        let mut end = MAX_BLOCKED_BYTES;
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason.truncate(end);
    }
    shipment.status = Status::Blocked;
    shipment.blocked = Some(reason);
}

fn lose(shipment: &mut Shipment, reason: &str) {
    let mut reason = reason.to_owned();
    if reason.len() > MAX_BLOCKED_BYTES {
        let mut end = MAX_BLOCKED_BYTES;
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason.truncate(end);
    }
    shipment.status = Status::Lost;
    shipment.blocked = Some(reason);
}

fn receipt_op(shipment: &Shipment, effect: &str) -> Result<(u64, String), Failure> {
    let attempt = shipment
        .attempt
        .checked_add(1)
        .filter(|n| *n <= mc_script::MAX_SCRIPT_WORLD_TIME)
        .ok_or(Failure::Capacity)?;
    Ok((
        attempt,
        format!("cv-{}-{effect}-{}-{attempt}", shipment.id, shipment.epoch),
    ))
}

/// Exact carrier lots and native inventory images for one atomic source pickup.
struct PickupPlan {
    lots: Vec<CarryLot>,
    new_source: Vec<ItemStack>,
    new_carry: Vec<ItemStack>,
}

/// One source slot holding the whole exact quantity; lots split across the
/// carrier's empty or component-compatible native slots. Real capacity beyond
/// `8 x stack` refuses here, naturally.
fn plan_pickup(
    source: &[ItemStack],
    carry: &[ItemStack],
    manifest: &ItemStack,
    quantity: u32,
    max_stack: i32,
) -> Option<PickupPlan> {
    if carry.len() != CARRY_SLOTS || max_stack <= 0 {
        return None;
    }
    let need = u64::from(quantity);
    let source_slot = source.iter().position(|stack| {
        !stack.is_empty()
            && can_stack(stack, manifest)
            && u64::from(stack.count.max(0) as u32) >= need
    })?;
    let mut lots = Vec::new();
    let mut remaining = quantity;
    for (index, slot) in carry.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        let room = if slot.is_empty() {
            max_stack
        } else if can_stack(slot, manifest) {
            max_stack - slot.count
        } else {
            0
        };
        let take = remaining.min(room.max(0) as u32);
        if take == 0 {
            continue;
        }
        lots.push(CarryLot {
            slot: u8::try_from(index).ok()?,
            count: take,
        });
        remaining -= take;
    }
    if remaining != 0 {
        return None;
    }
    let mut new_source = source.to_vec();
    new_source[source_slot].count -= quantity as i32;
    if new_source[source_slot].count <= 0 {
        new_source[source_slot] = ItemStack::EMPTY;
    }
    let mut new_carry = carry.to_vec();
    for lot in &lots {
        let slot = &mut new_carry[usize::from(lot.slot)];
        if slot.is_empty() {
            *slot = manifest.clone();
            slot.count = lot.count as i32;
        } else {
            slot.count = slot.count.saturating_add(lot.count as i32);
        }
    }
    Some(PickupPlan {
        lots,
        new_source,
        new_carry,
    })
}

/// The carried lots must be exactly present; destination room across at most
/// sixteen portions. Anything else refuses with cargo untouched.
fn plan_unload(
    carry: &[ItemStack],
    lots: &[CarryLot],
    destination: &[ItemStack],
    manifest: &ItemStack,
    quantity: u32,
    max_stack: i32,
) -> Option<(Vec<ItemStack>, Vec<ItemStack>)> {
    if max_stack <= 0 || lots.iter().map(|lot| lot.count).sum::<u32>() != quantity {
        return None;
    }
    for lot in lots {
        if lot.count == 0 {
            return None;
        }
        let slot = carry.get(usize::from(lot.slot))?;
        if slot.count < lot.count as i32 || !can_stack(slot, manifest) {
            return None;
        }
    }
    let mut deposits: Vec<(usize, u32)> = Vec::new();
    let mut remaining = quantity;
    for (index, slot) in destination.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        if deposits.len() >= MAX_DEPOSITS {
            break;
        }
        let room = if slot.is_empty() {
            max_stack
        } else if can_stack(slot, manifest) {
            max_stack - slot.count
        } else {
            0
        };
        let take = remaining.min(room.max(0) as u32);
        if take == 0 {
            continue;
        }
        deposits.push((index, take));
        remaining -= take;
    }
    if remaining != 0 {
        return None;
    }
    let mut new_carry = carry.to_vec();
    for lot in lots {
        let slot = &mut new_carry[usize::from(lot.slot)];
        slot.count -= lot.count as i32;
        if slot.is_empty() {
            *slot = ItemStack::EMPTY;
        }
    }
    let mut new_destination = destination.to_vec();
    for (index, count) in deposits {
        let slot = &mut new_destination[index];
        if slot.is_empty() {
            *slot = manifest.clone();
            slot.count = count as i32;
        } else {
            slot.count = slot.count.saturating_add(count as i32);
        }
    }
    Some((new_carry, new_destination))
}

fn carry_endpoint(carrier: Uuid) -> Endpoint {
    Endpoint::VillagerCarry { uuid: carrier }
}

/// Actual source pickup: live member body at the source stance, admission
/// fences unchanged, exact manifest present, real carry room. Atomic: the
/// bridge validates both images before either native write.
fn try_pickup(
    world: &mut TickState,
    settlement: SettlementId,
    shipment: &mut Shipment,
) -> Result<(), Failure> {
    let source_endpoint = container_endpoint(&shipment.source);
    let carry_endpoint = carry_endpoint(shipment.carrier);
    let facts = super::economy::building(world, settlement, &shipment.source.building)?;
    if !certified_container(world, &facts, shipment.source.position)? {
        return Err(Failure::Forbidden);
    }
    super::economy::authorize_endpoint(world, settlement, &carry_endpoint)?;
    let source_live = load(world, &source_endpoint)?;
    let carry_live = load(world, &carry_endpoint)?;
    if source_live.fence != shipment.source_fence || carry_live.fence != shipment.carrier_fence {
        return Err(Failure::StaleRevision);
    }
    if !source_live.slots.iter().any(|stack| {
        !stack.is_empty()
            && can_stack(stack, &shipment.manifest)
            && u64::from(stack.count.max(0) as u32) >= u64::from(shipment.quantity)
    }) {
        return Err(Failure::InsufficientItems);
    }
    let max = world
        .registry
        .item_rules()
        .max_stack(&shipment.manifest)
        .min(99);
    let Some(plan) = plan_pickup(
        &source_live.slots,
        &carry_live.slots,
        &shipment.manifest,
        shipment.quantity,
        max,
    ) else {
        return Err(Failure::Capacity);
    };
    let PickupPlan {
        lots,
        new_source,
        new_carry,
    } = plan;
    let (attempt, op) = receipt_op(shipment, "load")?;
    let fences = commit(
        world,
        vec![
            Change {
                before: source_live,
                after: new_source,
            },
            Change {
                before: carry_live,
                after: new_carry,
            },
        ],
    )?;
    shipment.attempt = attempt;
    shipment.carry = lots;
    shipment.carrier_live_fence = Some(fences[1].clone());
    shipment.load = Some(OpReceipt {
        op,
        epoch: world.tick,
    });
    shipment.loaded = true;
    shipment.status = Status::Transit;
    shipment.cursor = 1;
    Ok(())
}

/// Genuine unload: arrived, live body at the destination stance, carried lots
/// exactly present, both fences ours, real destination room. Atomic.
fn try_unload(
    world: &mut TickState,
    settlement: SettlementId,
    shipment: &mut Shipment,
) -> Result<(), Failure> {
    let destination_endpoint = container_endpoint(&shipment.destination);
    let carry_endpoint = carry_endpoint(shipment.carrier);
    let facts = super::economy::building(world, settlement, &shipment.destination.building)?;
    if !certified_container(world, &facts, shipment.destination.position)? {
        return Err(Failure::Forbidden);
    }
    super::economy::authorize_endpoint(world, settlement, &carry_endpoint)?;
    let destination_live = load(world, &destination_endpoint)?;
    let carry_live = load(world, &carry_endpoint)?;
    if destination_live.fence != shipment.destination_fence
        || Some(&carry_live.fence) != shipment.carrier_live_fence.as_ref()
    {
        return Err(Failure::StaleRevision);
    }
    let max = world.registry.item_rules().max_stack(&shipment.manifest);
    let Some((new_carry, new_destination)) = plan_unload(
        &carry_live.slots,
        &shipment.carry,
        &destination_live.slots,
        &shipment.manifest,
        shipment.quantity,
        max,
    ) else {
        return Err(Failure::Capacity);
    };
    let (attempt, op) = receipt_op(shipment, "unload")?;
    commit(
        world,
        vec![
            Change {
                before: carry_live,
                after: new_carry,
            },
            Change {
                before: destination_live,
                after: new_destination,
            },
        ],
    )?;
    shipment.attempt = attempt;
    let epoch = world.tick;
    shipment.carry.clear();
    shipment.carrier_live_fence = None;
    shipment.unload = Some(OpReceipt {
        op: op.clone(),
        epoch,
    });
    shipment.delivery = Some(OpReceipt { op, epoch });
    shipment.loaded = false;
    shipment.status = Status::Delivered;
    shipment.blocked = None;
    Ok(())
}

fn step(world: &mut TickState, settlement: SettlementId, shipment: &mut Shipment) -> bool {
    if terminal(shipment.status) {
        return false;
    }
    // Deadlines use the native clock, not calls or offscreen movement. An idle
    // step leaves all durable fields untouched.
    if world.tick.saturating_sub(shipment.epoch) > shipment.max_ticks {
        block(shipment, "finite route tick budget exhausted");
        return true;
    }
    let cell = match live_cell(world, settlement, shipment.carrier) {
        Ok(cell) => cell,
        Err(Fault::Gone) => {
            lose(
                shipment,
                "native carrier gone; whatever native inventory actually holds stays",
            );
            return true;
        }
        Err(Fault::Away) => {
            block(
                shipment,
                "carrier left the settlement; cargo stays where it physically is",
            );
            return true;
        }
    };
    let result = match shipment.status {
        Status::Loading if cell == shipment.waypoints[0] => try_pickup(world, settlement, shipment),
        Status::Transit if shipment.waypoints.get(shipment.cursor) == Some(&cell) => {
            shipment.cursor += 1;
            shipment.ticks = shipment.ticks.saturating_add(1);
            if shipment.cursor < shipment.waypoints.len() {
                return true;
            }
            shipment.status = Status::Arrived;
            // Arrival is itself a durable change even if native unloading is
            // temporarily unavailable. A later tick retries the actual write.
            if let Err(failure) = try_unload(world, settlement, shipment)
                && matches!(
                    failure,
                    Failure::StaleRevision | Failure::Capacity | Failure::InsufficientItems
                )
            {
                block(shipment, failure.as_str());
            }
            return true;
        }
        Status::Arrived if shipment.waypoints.last() == Some(&cell) => {
            try_unload(world, settlement, shipment)
        }
        _ => return false,
    };
    match result {
        Ok(()) => {
            shipment.ticks = shipment.ticks.saturating_add(1);
            true
        }
        Err(
            failure @ (Failure::StaleRevision | Failure::Capacity | Failure::InsufficientItems),
        ) => {
            block(shipment, failure.as_str());
            true
        }
        Err(_) => false,
    }
}

/// At most sixteen active contracts per town/tick, rotated fairly without
/// copying keys or retaining cursor state in the WAL. Idle scans change no
/// policy; deadline expiry and actual body/stock transitions do.
pub(crate) fn tick(world: &mut TickState, settlement: SettlementId, state: &mut State) -> bool {
    state.cursor.run(
        &mut state.shipments,
        |shipment| !terminal(shipment.status),
        |_, shipment| step(world, settlement, shipment).into(),
    )
}

/// Controller executes as the native settlement owner; the caller authenticates
/// that first. Returns true iff policy actually changed: idempotent replays
/// return false, errors leave policy unchanged.
pub(crate) fn execute(
    world: &mut TickState,
    settlement: SettlementId,
    owner: &str,
    state: &mut State,
    request: Request,
) -> Result<bool, Failure> {
    let record = world.settlements.get(settlement).ok_or(Failure::NotFound)?;
    if record.owner != owner {
        return Err(Failure::Forbidden);
    }
    match request {
        Request::Open(open) => {
            let Open {
                id,
                source,
                destination,
                carrier,
                manifest,
                quantity,
                waypoints,
                max_ticks,
                paid_mandate,
            } = *open;
            if let Some(existing) = state.shipments.get(&id) {
                let same = existing.source == source
                    && existing.destination == destination
                    && existing.carrier == carrier
                    && existing.manifest == manifest
                    && existing.quantity == quantity
                    && existing.waypoints == waypoints
                    && existing.max_ticks == max_ticks
                    && existing.paid_mandate == paid_mandate;
                return if same {
                    Ok(false)
                } else {
                    Err(Failure::OperationConflict)
                };
            }
            if id.is_empty() || id.len() > mc_script::MAX_SCRIPT_ID_BYTES {
                return Err(Failure::InvalidRequest);
            }
            if quantity == 0 || quantity > MAX_QUANTITY {
                return Err(Failure::InvalidRequest);
            }
            if manifest.is_empty()
                || u64::from(quantity) > u64::from(i32::MAX as u32)
                || manifest.count != quantity as i32
                || world.registry.items.name_of(manifest.item_id).is_none()
            {
                return Err(Failure::InvalidRequest);
            }
            if max_ticks == 0 || max_ticks > MAX_ROUTE_TICKS {
                return Err(Failure::InvalidRequest);
            }
            if paid_mandate
                .as_ref()
                .is_some_and(|id| super::commerce::sequence(id).is_err())
            {
                return Err(Failure::InvalidRequest);
            }
            validate_route(&waypoints, source.position, destination.position)
                .map_err(|_| Failure::InvalidRequest)?;
            if source.position == destination.position && source.building == destination.building {
                return Err(Failure::InvalidRequest);
            }
            if source.dimension != world.dimension.name()
                || destination.dimension != world.dimension.name()
            {
                return Err(Failure::Forbidden);
            }
            if world.entities.slot_of_uuid(carrier).is_none_or(|slot| {
                world.entities.row(slot).villager.as_ref().is_none_or(|v| {
                    v.settlement != Some(settlement) || v.wandering || v.conversion_time.is_some()
                })
            }) {
                return Err(Failure::Forbidden);
            }
            if state.shipments.values().any(|s| {
                s.carrier == carrier
                    && matches!(
                        s.status,
                        Status::Loading | Status::Transit | Status::Arrived
                    )
            }) {
                return Err(Failure::OperationConflict);
            }
            for reference in [&source, &destination] {
                let facts = super::economy::building(world, settlement, &reference.building)?;
                if !certified_container(world, &facts, reference.position)? {
                    return Err(Failure::Forbidden);
                }
            }
            super::economy::authorize_endpoint(world, settlement, &carry_endpoint(carrier))?;
            let source_live = load(world, &container_endpoint(&source))?;
            let destination_live = load(world, &container_endpoint(&destination))?;
            let carry_live = load(world, &carry_endpoint(carrier))?;
            if !source_live.slots.iter().any(|stack| {
                !stack.is_empty()
                    && can_stack(stack, &manifest)
                    && u64::from(stack.count.max(0) as u32) >= u64::from(quantity)
            }) {
                return Err(Failure::InsufficientItems);
            }
            state.shipments.insert(
                id.clone(),
                Shipment {
                    id,
                    owner: owner.to_owned(),
                    settlement,
                    source,
                    destination,
                    carrier,
                    manifest,
                    quantity,
                    waypoints,
                    epoch: world.tick,
                    max_ticks,
                    paid_mandate,
                    source_fence: source_live.fence,
                    destination_fence: destination_live.fence,
                    carrier_fence: carry_live.fence,
                    status: Status::Loading,
                    blocked: None,
                    cursor: 0,
                    ticks: 0,
                    attempt: 0,
                    loaded: false,
                    carry: Vec::new(),
                    carrier_live_fence: None,
                    load: None,
                    unload: None,
                    delivery: None,
                },
            );
            Ok(true)
        }
        Request::Pickup { id } => {
            let (carrier, stance) = {
                let shipment = state.shipments.get(&id).ok_or(Failure::NotFound)?;
                if shipment.loaded {
                    return Ok(false);
                }
                if terminal(shipment.status) {
                    return Err(Failure::OperationConflict);
                }
                (shipment.carrier, shipment.waypoints[0])
            };
            match live_cell(world, settlement, carrier) {
                Ok(cell) => {
                    if cell != stance {
                        return Err(Failure::Blocked);
                    }
                }
                Err(Fault::Gone) => return Err(Failure::NotFound),
                Err(Fault::Away) => return Err(Failure::Forbidden),
            }
            let shipment = state.shipments.get_mut(&id).expect("admitted shipment");
            try_pickup(world, settlement, shipment)?;
            Ok(true)
        }
        Request::Unload { id } => {
            let (carrier, stance) = {
                let shipment = state.shipments.get(&id).ok_or(Failure::NotFound)?;
                if shipment.status == Status::Delivered {
                    return Ok(false);
                }
                if terminal(shipment.status) {
                    return Err(Failure::OperationConflict);
                }
                if shipment.status != Status::Arrived {
                    return Err(Failure::Blocked);
                }
                (
                    shipment.carrier,
                    shipment.waypoints[shipment.waypoints.len() - 1],
                )
            };
            match live_cell(world, settlement, carrier) {
                Ok(cell) => {
                    if cell != stance {
                        return Err(Failure::Blocked);
                    }
                }
                Err(Fault::Gone) => return Err(Failure::NotFound),
                Err(Fault::Away) => return Err(Failure::Forbidden),
            }
            let shipment = state.shipments.get_mut(&id).expect("admitted shipment");
            try_unload(world, settlement, shipment)?;
            Ok(true)
        }
        Request::Block { id, reason } => {
            let shipment = state.shipments.get(&id).ok_or(Failure::NotFound)?;
            if shipment.status == Status::Blocked {
                return Ok(false);
            }
            if terminal(shipment.status) {
                return Err(Failure::OperationConflict);
            }
            block(
                state.shipments.get_mut(&id).expect("admitted shipment"),
                &reason,
            );
            Ok(true)
        }
        Request::Cancel { id } => {
            let shipment = state.shipments.get(&id).ok_or(Failure::NotFound)?;
            if shipment.status == Status::Cancelled {
                return Ok(false);
            }
            if terminal(shipment.status) {
                return Err(Failure::OperationConflict);
            }
            state
                .shipments
                .get_mut(&id)
                .expect("admitted shipment")
                .status = Status::Cancelled;
            Ok(true)
        }
    }
}

#[cfg(test)]
#[path = "caravan_tests.rs"]
mod tests;
