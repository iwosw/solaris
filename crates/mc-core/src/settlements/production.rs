//! Native settlement production: one honest job per living member body.
//!
//! Admission checks the live town owner, the live district certificate (whose
//! generic owner may differ from the town owner), the live workstation block
//! and the startup-native [`WorkRecipe`](mc_data::recipes::work::WorkRecipe).
//! Progress is delayed and causal: at most one due portion per job per tick,
//! applied atomically through the parent inventory bridge. Bodies walk via
//! [`target`]; this module never teleports and never fabricates catch-up.

use std::collections::BTreeMap;

use mc_data::recipes::work::{WorkRecipe, WorkRecipeAlternative};
use mc_data::{Identifier, ItemStack};
use mc_script::{ScriptOperationFailure as Failure, ScriptSettlementBuildingRef};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{SettlementId, inventory};
use crate::boundary::BlockPos;
use crate::tick::TickState;

/// Manual-hold reason. Auto pauses use native reasons below; only this exact
/// value makes [`target`] hide the job and [`active`] skip it.
const MANUAL: &str = "paused";
/// Retry delay for paused jobs: causal, never backfills missed portions.
const RETRY_TICKS: u64 = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolWear {
    pub item: String,
    pub wear: u32,
    pub broke: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub job: String,
    pub sequence: u32,
    pub recipe: String,
    pub recipe_revision: String,
    pub inputs: Vec<ItemStack>,
    pub outputs: Vec<ItemStack>,
    pub tool_wear: Vec<ToolWear>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Job {
    pub id: String,
    pub owner: String,
    pub worker: Uuid,
    pub recipe: String,
    pub recipe_revision: String,
    pub duration: u64,
    pub count: u32,
    pub done: u32,
    pub station: [i32; 3],
    pub stand_at: [i32; 3],
    pub building: ScriptSettlementBuildingRef,
    pub dimension: String,
    pub accepted_tick: u64,
    pub next_due: u64,
    pub paused: Option<String>,
    pub cancelled: bool,
    pub receipts: Vec<Receipt>,
}

/// Policy only: named job map plus admission/completion counters for the
/// parent WAL. No inventory mirrors, no JSON fragments.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    pub jobs: BTreeMap<String, Job>,
    pub sequence: u64,
    pub completed: u64,
    pub cancelled: u64,
    #[serde(skip)]
    cursor: super::economy::WorkCursor,
}

/// Private admission parameters mirroring the flat `Request::Start` fields.
struct Start {
    job: String,
    worker: Uuid,
    recipe_name: String,
    count: u32,
    station: [i32; 3],
    stand_at: [i32; 3],
    building: ScriptSettlementBuildingRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    Start {
        job: String,
        worker: Uuid,
        recipe: String,
        count: u32,
        station: [i32; 3],
        stand_at: [i32; 3],
        building: ScriptSettlementBuildingRef,
    },
    Pause {
        job: String,
    },
    Resume {
        job: String,
    },
    Cancel {
        job: String,
    },
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn recipe_of<'a>(world: &'a TickState, name: &str) -> Result<&'a WorkRecipe, Failure> {
    let id = Identifier::parse(name).map_err(|_| Failure::NotFound)?;
    world
        .registry
        .item_facts
        .work_recipe(&id)
        .ok_or(Failure::NotFound)
}

/// One atomic portion plan against worker carry + equipment copies.
struct PortionPlan {
    carry_before: inventory::Image,
    equipment_before: inventory::Image,
    inputs: Vec<ItemStack>,
    outputs: Vec<ItemStack>,
    wear: Vec<ToolWear>,
    carry_after: Vec<ItemStack>,
    equipment_after: Vec<ItemStack>,
}

/// Live worker membership: exact native body, alive, bound to this town.
fn worker_slot(world: &TickState, settlement: SettlementId, worker: Uuid) -> Result<(), Failure> {
    let slot = world
        .entities
        .slot_of_uuid(worker)
        .ok_or(Failure::NotFound)?;
    let row = world.entities.row(slot);
    if !world.entities.body(slot).alive {
        return Err(Failure::NotFound);
    }
    match row.villager.as_ref().map(|v| v.settlement) {
        Some(Some(id)) if id == settlement => Ok(()),
        Some(_) => Err(Failure::Forbidden),
        None => Err(Failure::NotFound),
    }
}

fn worker_pos(world: &TickState, worker: Uuid) -> Option<[f64; 3]> {
    let slot = world.entities.slot_of_uuid(worker)?;
    world.entities.row(slot).villager.as_ref()?;
    Some(world.entities.pos(slot))
}

/// Player-built certificate (`kind == "building"`): the live record's
/// production building (`Workshop`/`Mine`/`LumberCamp`) bounds are the
/// footprint; the station is checked against the live block by [`station_live`].
fn native_building_facts(
    world: &TickState,
    settlement: SettlementId,
    reference: &ScriptSettlementBuildingRef,
) -> Result<crate::plugin_structures::BuildingFacts, Failure> {
    let id = super::buildings::BuildingId(reference.id.parse().map_err(|_| Failure::NotFound)?);
    let building = world
        .settlements
        .get(settlement)
        .and_then(|record| record.buildings.get(&id))
        .ok_or(Failure::NotFound)?;
    if building.kind.profession().is_none()
        || reference.purpose.strip_prefix("solaris:") != Some(building.kind.name())
    {
        return Err(Failure::NotFound);
    }
    Ok(crate::plugin_structures::BuildingFacts {
        dimension: world.dimension.name().to_owned(),
        footprint: building.bounds,
        points: Vec::new(),
        blueprint: None,
    })
}

/// Live certificate + intact native building. A plugin-ledger certificate must
/// be in the live district plan; its generic owner (`DistrictPlan.owner`)
/// queries the native ledger, never the town owner.
fn building_facts(
    world: &mut TickState,
    settlement: SettlementId,
    reference: &ScriptSettlementBuildingRef,
) -> Result<crate::plugin_structures::BuildingFacts, Failure> {
    reference.validate().map_err(|_| Failure::InvalidRequest)?;
    if reference.kind == "building" {
        return native_building_facts(world, settlement, reference);
    }
    let record = world.settlements.get(settlement).ok_or(Failure::NotFound)?;
    let district = record.district.as_ref().ok_or(Failure::NotFound)?;
    if !district.buildings.iter().any(|bound| {
        bound.kind == reference.kind
            && bound.id == reference.id
            && bound.purpose == reference.purpose
    }) {
        return Err(Failure::NotFound);
    }
    let mut facts = crate::plugin_structures::building_facts(
        world,
        &district.owner,
        std::iter::once((reference.id.as_str(), reference.kind == "worksite")),
    )?;
    let facts = facts.pop().ok_or(Failure::NotFound)?;
    if facts.dimension != world.dimension.name()
        || !crate::plugin_structures::check_building(world, &facts)?
    {
        return Err(Failure::Blocked);
    }
    Ok(facts)
}

fn footprint_contains(facts: &crate::plugin_structures::BuildingFacts, at: [i32; 3]) -> bool {
    (0..3)
        .all(|axis| facts.footprint.min[axis] <= at[axis] && at[axis] <= facts.footprint.max[axis])
}

/// The requested station must be live recipe station inside the certified
/// footprint; the stand post must sit next to it.
fn station_live(
    world: &TickState,
    recipe: &WorkRecipe,
    facts: &crate::plugin_structures::BuildingFacts,
    station: [i32; 3],
    stand_at: [i32; 3],
) -> Result<(), Failure> {
    if facts.dimension != world.dimension.name() {
        return Err(Failure::Unloaded);
    }
    if !footprint_contains(facts, station) {
        return Err(Failure::Blocked);
    }
    let near: i64 = station
        .into_iter()
        .zip(stand_at)
        .map(|(a, b)| (i64::from(a) - i64::from(b)).pow(2))
        .sum();
    if near > 64 {
        return Err(Failure::InvalidRequest);
    }
    let native = world
        .world
        .block_at(station[0], station[1], station[2])
        .ok_or(Failure::Unloaded)?;
    let block = world
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(native))
        .ok_or(Failure::Blocked)?;
    if block.block.id != recipe.station
        || (recipe.station.path() == "campfire"
            && block
                .properties
                .iter()
                .any(|(key, value)| key == "lit" && value == "false"))
    {
        return Err(Failure::Blocked);
    }
    Ok(())
}

fn resource<'a>(world: &'a TickState, stack: &ItemStack) -> Option<&'a str> {
    world
        .registry
        .items
        .name_of(stack.item_id)
        .map(Identifier::as_str)
}

fn accepts(
    world: &TickState,
    stack: &ItemStack,
    input: &mc_data::recipes::work::WorkRecipeInput,
) -> bool {
    if stack.is_empty()
        || input
            .components
            .as_ref()
            .is_some_and(|components| !components.matches(stack))
    {
        return false;
    }
    let identity = resource(world, stack);
    let item = input
        .item
        .as_ref()
        .is_some_and(|name| identity == Some(name.as_str()));
    let tag = input.tag.as_ref().is_some_and(|name| {
        world
            .registry
            .tags
            .contains_raw_id("minecraft:item", name.as_str(), stack.item_id as i32)
    });
    item || tag
        || input
            .alternatives
            .iter()
            .any(|alternative| match alternative {
                WorkRecipeAlternative::Item(name) => identity == Some(name.as_str()),
                WorkRecipeAlternative::Tag(name) => world.registry.tags.contains_raw_id(
                    "minecraft:item",
                    name.as_str(),
                    stack.item_id as i32,
                ),
            })
}

fn maximum_damage(world: &TickState, stack: &ItemStack, name: &str) -> Option<u32> {
    stack
        .extended()
        .max_damage
        .or_else(|| {
            world
                .registry
                .item_facts
                .get(world.registry.items.name_of(stack.item_id)?)
                .and_then(|facts| facts.max_damage)
        })
        .or_else(|| {
            mc_data::item_semantics_26_1_2::max_tool_damage_for_path(
                name.strip_prefix("minecraft:").unwrap_or(name),
            )
            .map(|value| value as u32)
        })
        .filter(|maximum| *maximum > 0 && *maximum <= i32::MAX as u32)
}

enum Shortage {
    Input,
    Tool,
    Storage,
}

#[allow(clippy::too_many_lines)]
fn plan_portion(
    world: &TickState,
    worker: Uuid,
    recipe: &WorkRecipe,
) -> Result<PortionPlan, Shortage> {
    let carry_image = inventory::load(world, &inventory::Endpoint::VillagerCarry { uuid: worker })
        .map_err(|_| Shortage::Input)?;
    let equipment_image = inventory::load(
        world,
        &inventory::Endpoint::VillagerEquipment { uuid: worker },
    )
    .map_err(|_| Shortage::Input)?;
    let mut carry = carry_image.slots.clone();
    let mut equipment = equipment_image.slots.clone();
    let mut inputs = Vec::new();
    for input in &recipe.inputs {
        let mut remaining = input.count;
        for slots in [&mut carry, &mut equipment] {
            for stack in slots.iter_mut() {
                if remaining == 0 {
                    break;
                }
                if !accepts(world, stack, input) {
                    continue;
                }
                let take = remaining.min(stack.count.max(0) as u32);
                let mut taken = stack.clone();
                taken.count = take as i32;
                inputs.push(taken);
                stack.count -= take as i32;
                if stack.is_empty() {
                    *stack = ItemStack::EMPTY;
                }
                remaining -= take;
            }
        }
        if remaining != 0 {
            return Err(Shortage::Input);
        }
    }
    let mut wear = Vec::new();
    for tool in &recipe.tools {
        let mut placed = None;
        for (slots, equipment_side) in [(&mut equipment, true), (&mut carry, false)] {
            if let Some(index) = slots.iter().position(|stack| {
                !stack.is_empty()
                    && resource(world, stack) == Some(tool.item.as_str())
                    && tool
                        .components
                        .as_ref()
                        .is_none_or(|components| components.matches(stack))
            }) {
                placed = Some((index, equipment_side));
                break;
            }
        }
        let Some((index, equipment_side)) = placed else {
            return Err(Shortage::Tool);
        };
        let slots = if equipment_side {
            &mut equipment
        } else {
            &mut carry
        };
        let name = resource(world, &slots[index]).ok_or(Shortage::Tool)?;
        let maximum = maximum_damage(world, &slots[index], name).ok_or(Shortage::Tool)?;
        let damage = (slots[index].damage.unwrap_or(0).max(0) as u32).saturating_add(tool.wear);
        let broke = damage >= maximum;
        if broke {
            slots[index] = ItemStack::EMPTY;
        } else {
            slots[index].damage = Some(damage as i32);
        }
        wear.push(ToolWear {
            item: name.to_owned(),
            wear: tool.wear,
            broke,
        });
    }
    let mut outputs = Vec::new();
    for output in recipe.outputs.iter().chain(&recipe.remainders) {
        outputs.push(
            output
                .to_stack(&world.registry.items, &world.registry.item_facts)
                .ok_or(Shortage::Input)?,
        );
    }
    for stack in &outputs {
        let mut rest = stack.clone();
        for slot in &mut carry {
            if rest.is_empty() {
                break;
            }
            let maximum = world.registry.item_rules().max_stack(&rest).min(99);
            if slot.is_empty() {
                let take = rest.count.min(maximum);
                *slot = rest.clone();
                slot.count = take;
                rest.count -= take;
            } else if mc_data::inventory_semantics_26_1_2::can_stack(slot, &rest) {
                let take = rest.count.min((maximum - slot.count).max(0));
                slot.count += take;
                rest.count -= take;
            }
        }
        if !rest.is_empty() {
            return Err(Shortage::Storage);
        }
    }
    Ok(PortionPlan {
        carry_before: carry_image,
        equipment_before: equipment_image,
        inputs,
        outputs,
        wear,
        carry_after: carry,
        equipment_after: equipment,
    })
}

fn pause_reason(failure: Failure) -> &'static str {
    match failure {
        Failure::Unloaded => "unloaded",
        Failure::Blocked => "missing station",
        Failure::NotFound => "missing station",
        Failure::Capacity => "no storage",
        Failure::InsufficientItems => "missing input",
        _ => "blocked route",
    }
}

fn admit(
    world: &mut TickState,
    settlement: SettlementId,
    owner: &str,
    state: &mut State,
    start: Start,
) -> Result<bool, Failure> {
    let Start {
        job,
        worker,
        recipe_name,
        count,
        station,
        stand_at,
        building,
    } = start;
    if !valid_id(&job) || count == 0 || worker.is_nil() {
        return Err(Failure::InvalidRequest);
    }
    let record = world.settlements.get(settlement).ok_or(Failure::NotFound)?;
    if record.owner != owner {
        return Err(Failure::Forbidden);
    }
    worker_slot(world, settlement, worker)?;
    if let Some(existing) = state.jobs.get(&job) {
        let same = existing.worker == worker
            && existing.recipe == recipe_name
            && existing.count == count
            && existing.station == station
            && existing.stand_at == stand_at
            && existing.building.kind == building.kind
            && existing.building.id == building.id
            && existing.building.purpose == building.purpose;
        return if same {
            Ok(false)
        } else {
            Err(Failure::OperationConflict)
        };
    }
    if state
        .jobs
        .values()
        .any(|other| !other.cancelled && other.done < other.count && other.worker == worker)
    {
        return Err(Failure::Busy);
    }
    let facts = building_facts(world, settlement, &building)?;
    let recipe = recipe_of(world, &recipe_name)?;
    station_live(world, recipe, &facts, station, stand_at)?;
    let duration = recipe.duration_ticks.max(1);
    let tick = world.tick;
    state.jobs.insert(
        job.clone(),
        Job {
            id: job,
            owner: owner.to_owned(),
            worker,
            recipe: recipe_name,
            recipe_revision: recipe.revision.clone(),
            duration,
            count,
            done: 0,
            station,
            stand_at,
            building,
            dimension: facts.dimension,
            accepted_tick: tick,
            next_due: tick.saturating_add(duration),
            paused: None,
            cancelled: false,
            receipts: Vec::new(),
        },
    );
    state.sequence = state.sequence.saturating_add(1);
    Ok(true)
}

pub(crate) fn execute(
    world: &mut TickState,
    settlement: SettlementId,
    owner: &str,
    state: &mut State,
    request: Request,
) -> Result<bool, Failure> {
    match request {
        Request::Start {
            job,
            worker,
            recipe: recipe_name,
            count,
            station,
            stand_at,
            building,
        } => admit(
            world,
            settlement,
            owner,
            state,
            Start {
                job,
                worker,
                recipe_name,
                count,
                station,
                stand_at,
                building,
            },
        ),
        Request::Pause { job } => {
            let Some(entry) = state.jobs.get_mut(&job) else {
                return Err(Failure::NotFound);
            };
            if entry.cancelled
                || entry.done >= entry.count
                || entry.paused.as_deref() == Some(MANUAL)
            {
                return Ok(false);
            }
            entry.paused = Some(MANUAL.to_owned());
            Ok(true)
        }
        Request::Resume { job } => {
            let tick = world.tick;
            let Some(entry) = state.jobs.get_mut(&job) else {
                return Err(Failure::NotFound);
            };
            if entry.cancelled {
                return Err(Failure::InvalidRequest);
            }
            if entry.done >= entry.count || entry.paused.is_none() {
                return Ok(false);
            }
            entry.paused = None;
            entry.next_due = tick;
            Ok(true)
        }
        Request::Cancel { job } => {
            let Some(entry) = state.jobs.get_mut(&job) else {
                return Err(Failure::NotFound);
            };
            if entry.cancelled || entry.done >= entry.count {
                return Ok(false);
            }
            entry.cancelled = true;
            entry.paused = None;
            state.cancelled = state.cancelled.saturating_add(1);
            Ok(true)
        }
    }
}

fn reached(world: &TickState, job: &Job) -> bool {
    let Some(pos) = worker_pos(world, job.worker) else {
        return false;
    };
    let centre = [
        f64::from(job.stand_at[0]) + 0.5,
        f64::from(job.stand_at[1]),
        f64::from(job.stand_at[2]) + 0.5,
    ];
    pos.iter()
        .zip(centre)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        <= 6.25
}

fn park(job: &mut Job, reason: &str, next_due: u64) -> bool {
    let changed = job.paused.as_deref() != Some(reason) || job.next_due != next_due;
    if job.paused.as_deref() != Some(reason) {
        job.paused = Some(reason.to_owned());
    }
    job.next_due = next_due;
    changed
}

fn tick_job(
    world: &mut TickState,
    settlement: SettlementId,
    job: &mut Job,
) -> Result<bool, Failure> {
    // Resolve mutable physical evidence before borrowing the native recipe.
    let facts = building_facts(world, settlement, &job.building)?;
    let recipe = recipe_of(world, &job.recipe)?;
    let metadata_changed =
        job.recipe_revision != recipe.revision || job.duration != recipe.duration_ticks.max(1);
    if job.recipe_revision != recipe.revision {
        job.recipe_revision.clone_from(&recipe.revision);
    }
    job.duration = recipe.duration_ticks.max(1);
    let retry = world.tick.saturating_add(RETRY_TICKS);
    if let Err(reason) = station_live(world, recipe, &facts, job.station, job.stand_at) {
        return Ok(park(job, pause_reason(reason), retry) | metadata_changed);
    }
    if !reached(world, job) {
        return Ok(park(job, "awaiting reach", retry) | metadata_changed);
    }
    let plan = match plan_portion(world, job.worker, recipe) {
        Ok(plan) => plan,
        Err(shortage) => {
            let reason = match shortage {
                Shortage::Input => "missing input",
                Shortage::Tool => "missing tool",
                Shortage::Storage => "no storage",
            };
            return Ok(park(job, reason, retry) | metadata_changed);
        }
    };
    let changes = vec![
        inventory::Change {
            before: plan.carry_before,
            after: plan.carry_after,
        },
        inventory::Change {
            before: plan.equipment_before,
            after: plan.equipment_after,
        },
    ];
    if let Err(failure) = inventory::commit(world, changes) {
        return Ok(park(job, pause_reason(failure), retry) | metadata_changed);
    }
    job.done += 1;
    job.receipts.push(Receipt {
        job: job.id.clone(),
        sequence: job.done,
        recipe: job.recipe.clone(),
        recipe_revision: job.recipe_revision.clone(),
        inputs: plan.inputs,
        outputs: plan.outputs,
        tool_wear: plan.wear,
    });
    job.paused = None;
    job.next_due = world.tick.saturating_add(job.duration);
    Ok(true)
}

pub(crate) fn tick(world: &mut TickState, settlement: SettlementId, state: &mut State) -> bool {
    let tick_now = world.tick;
    let mut completed = 0_u64;
    let changed = state.cursor.run(
        &mut state.jobs,
        |job| {
            !job.cancelled
                && job.done < job.count
                && job.paused.as_deref() != Some(MANUAL)
                && job.next_due <= tick_now
        },
        |_, job| {
            if worker_slot(world, settlement, job.worker).is_err() {
                return park(job, "missing worker", tick_now.saturating_add(RETRY_TICKS)).into();
            }
            match tick_job(world, settlement, job) {
                Ok(changed) => {
                    if job.done >= job.count {
                        completed += 1;
                    }
                    changed
                }
                Err(failure) => park(
                    job,
                    pause_reason(failure),
                    tick_now.saturating_add(RETRY_TICKS),
                ),
            }
            .into()
        },
    );
    state.completed = state.completed.saturating_add(completed);
    changed || completed > 0
}

/// Any nonterminal, noncancelled job needing tick. Auto-paused jobs that can
/// resume are included; only the manual hold is parked.
pub(crate) fn active(state: &State) -> bool {
    state
        .jobs
        .values()
        .any(|job| !job.cancelled && job.done < job.count && job.paused.as_deref() != Some(MANUAL))
}

/// Native brain routing target for one worker: its live stand post, hidden
/// while manually held or terminal.
#[cfg(test)]
pub(crate) fn target(state: &State, worker: Uuid) -> Option<BlockPos> {
    state
        .jobs
        .values()
        .filter(|job| job.worker == worker)
        .find_map(target_for_job)
}

pub(super) fn target_for_job(job: &Job) -> Option<BlockPos> {
    (!job.cancelled && job.done < job.count && job.paused.as_deref() != Some(MANUAL)).then_some(
        BlockPos {
            x: job.stand_at[0],
            y: job.stand_at[1],
            z: job.stand_at[2],
        },
    )
}

pub(crate) fn validate(state: &State) -> Result<(), &'static str> {
    if state.completed > state.sequence || state.cancelled > state.sequence {
        return Err("invalid production counters");
    }
    for (id, job) in &state.jobs {
        if !valid_id(id) || id != &job.id {
            return Err("invalid production job identity");
        }
        if job.owner.is_empty()
            || job.recipe.is_empty()
            || job.recipe_revision.is_empty()
            || job.dimension.is_empty()
            || job.count == 0
            || job.done > job.count
            || job.duration == 0
        {
            return Err("invalid production job");
        }
        if job.cancelled && job.done >= job.count {
            return Err("invalid cancelled production job");
        }
        if job.receipts.len() != job.done as usize {
            return Err("invalid production receipts");
        }
        for (index, receipt) in job.receipts.iter().enumerate() {
            if receipt.sequence != index as u32 + 1
                || receipt.job != job.id
                || receipt.recipe != job.recipe
                || receipt.recipe_revision.is_empty()
                || receipt.outputs.is_empty()
            {
                return Err("invalid production receipt");
            }
        }
        job.building
            .validate()
            .map_err(|_| "invalid production certificate")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "production_tests.rs"]
mod tests;
