//! Native economic policy; real stock and motion remain with their native owners.
use mc_script::{ScriptOperationFailure as Failure, ScriptSettlementBuildingRef};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{SettlementId, caravan, commerce, inventory, production};
use crate::{boundary::BlockPos, tick::TickState};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    #[serde(default)]
    pub production: production::State,
    #[serde(default)]
    pub commerce: commerce::State,
    #[serde(default)]
    pub caravan: caravan::State,
    /// Derived routing lookup: never serialized or compared as policy. Native
    /// brains must not scan every historical job/shipment for every body.
    #[serde(skip)]
    routes: BTreeMap<uuid::Uuid, (RouteKind, [i32; 3], bool)>,
    #[serde(skip)]
    route_generation: bool,
}

/// Ephemeral fair scheduling over authoritative contract maps. The two key
/// buffers retain their allocations; neither cursors nor equality participate
/// in journal/checkpoint policy images.
pub(super) enum Work {
    Unchanged,
    Changed,
    Remove,
}

impl From<bool> for Work {
    fn from(changed: bool) -> Self {
        if changed {
            Self::Changed
        } else {
            Self::Unchanged
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct WorkCursor {
    after: String,
    next: String,
}

impl Clone for WorkCursor {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for WorkCursor {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl WorkCursor {
    pub(super) fn run<T>(
        &mut self,
        records: &mut BTreeMap<String, T>,
        eligible: impl Fn(&T) -> bool,
        mut apply: impl FnMut(&str, &mut T) -> Work,
    ) -> bool {
        let mut changed = false;
        let mut visited = 0;
        let mut visit = |tail: bool| {
            records.retain(|id, record| {
                if visited == 16 || (id.as_str() > self.after.as_str()) != tail || !eligible(record)
                {
                    return true;
                }
                self.next.clone_from(id);
                visited += 1;
                match apply(id, record) {
                    Work::Unchanged => true,
                    Work::Changed => {
                        changed = true;
                        true
                    }
                    Work::Remove => {
                        changed = true;
                        false
                    }
                }
            });
        };
        visit(true);
        visit(false);
        if visited != 0 {
            std::mem::swap(&mut self.after, &mut self.next);
        }
        changed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteKind {
    Production,
    Caravan,
}

impl Clone for State {
    fn clone(&self) -> Self {
        // Journal/checkpoint images copy authority, not potentially thousands
        // of derived routing entries. Registry::apply_image rebuilds those.
        Self {
            production: self.production.clone(),
            commerce: self.commerce.clone(),
            caravan: self.caravan.clone(),
            ..Self::default()
        }
    }
}

impl PartialEq for State {
    fn eq(&self, other: &Self) -> bool {
        self.production == other.production
            && self.commerce == other.commerce
            && self.caravan == other.caravan
    }
}

impl State {
    pub(super) fn refresh_routes(&mut self) {
        let generation = !self.route_generation;
        let production = self.production.jobs.values().filter_map(|job| {
            production::target_for_job(job).map(|at| (job.worker, RouteKind::Production, at))
        });
        let caravans = self.caravan.shipments.values().filter_map(|shipment| {
            caravan::target_for_shipment(shipment)
                .map(|at| (shipment.carrier, RouteKind::Caravan, at))
        });
        for (worker, kind, at) in production.chain(caravans) {
            self.routes
                .insert(worker, (kind, [at.x, at.y, at.z], generation));
        }
        self.routes.retain(|_, (_, _, seen)| *seen == generation);
        self.route_generation = generation;
    }

    fn assigned(&self, worker: uuid::Uuid, kind: RouteKind) -> bool {
        self.routes
            .get(&worker)
            .is_some_and(|(assigned, _, _)| *assigned == kind)
    }
}

pub(crate) enum Request {
    Production(production::Request),
    Commerce(commerce::Request),
    Caravan(caravan::Request),
}

pub(super) fn validate(state: &State) -> Result<(), &'static str> {
    production::validate(&state.production)?;
    commerce::validate(&state.commerce)?;
    caravan::validate(&state.caravan)
}

pub(super) fn active(state: &State) -> bool {
    production::active(&state.production)
        || commerce::active(&state.commerce)
        || caravan::active(&state.caravan)
}

fn next_revision(world: &TickState, id: SettlementId) -> Result<u64, Failure> {
    world
        .settlements
        .get(id)
        .ok_or(Failure::NotFound)?
        .revision
        .checked_add(1)
        .filter(|n| *n <= mc_script::MAX_SCRIPT_WORLD_TIME)
        .ok_or(Failure::Capacity)
}

/// Caller authenticates the native town owner before admission. Extract only
/// the selected domain, so commerce can inspect live caravan delivery evidence.
pub(crate) fn execute(
    world: &mut TickState,
    id: SettlementId,
    request: Request,
) -> Result<(), Failure> {
    let revision = next_revision(world, id)?;
    let record = world.settlements.get(id).expect("validated settlement");
    let worker_busy = match &request {
        Request::Production(production::Request::Start { worker, .. }) => {
            record.economy.assigned(*worker, RouteKind::Caravan)
        }
        Request::Production(production::Request::Resume { job }) => record
            .economy
            .production
            .jobs
            .get(job)
            .is_some_and(|job| record.economy.assigned(job.worker, RouteKind::Caravan)),
        Request::Caravan(caravan::Request::Open(open)) => {
            record.economy.assigned(open.carrier, RouteKind::Production)
        }
        _ => false,
    };
    if worker_busy {
        return Err(Failure::Blocked);
    }
    let owner = record.owner.clone().ok_or(Failure::Forbidden)?;
    let result = match request {
        Request::Production(request) => {
            let mut state = std::mem::take(&mut world.settlements.economy_for_tick(id).production);
            let result = production::execute(world, id, &owner, &mut state, request);
            world.settlements.economy_for_tick(id).production = state;
            result
        }
        Request::Commerce(request) => {
            let mut state = std::mem::take(&mut world.settlements.economy_for_tick(id).commerce);
            let result = commerce::execute(world, id, &owner, &mut state, request);
            world.settlements.economy_for_tick(id).commerce = state;
            result
        }
        Request::Caravan(request) => {
            let mut state = std::mem::take(&mut world.settlements.economy_for_tick(id).caravan);
            let result = caravan::execute(world, id, &owner, &mut state, request);
            world.settlements.economy_for_tick(id).caravan = state;
            result
        }
    };
    let changed = result?;
    world.settlements.finish_economy(id, revision, changed);
    Ok(())
}

/// The derived active ID index excludes idle towns/terminal journals. Domain
/// state is moved, not cloned; only a genuinely changed image touches the WAL.
pub(super) fn tick(world: &mut TickState) {
    let mut after = None;
    while let Some(id) = world.settlements.next_economy(after) {
        after = Some(id);
        let Ok(revision) = next_revision(world, id) else {
            continue;
        };
        let mut production = std::mem::take(&mut world.settlements.economy_for_tick(id).production);
        let mut changed = production::tick(world, id, &mut production);
        world.settlements.economy_for_tick(id).production = production;
        let mut caravan = std::mem::take(&mut world.settlements.economy_for_tick(id).caravan);
        changed |= caravan::tick(world, id, &mut caravan);
        world.settlements.economy_for_tick(id).caravan = caravan;
        let mut commerce = std::mem::take(&mut world.settlements.economy_for_tick(id).commerce);
        changed |= commerce::tick(world, id, &mut commerce);
        world.settlements.economy_for_tick(id).commerce = commerce;
        world.settlements.finish_economy(id, revision, changed);
    }
}

pub(crate) fn target(
    world: &TickState,
    settlement: Option<SettlementId>,
    worker: uuid::Uuid,
) -> Option<BlockPos> {
    let economy = &world.settlements.get(settlement?)?.economy;
    economy.routes.get(&worker).map(|(_, at, _)| BlockPos {
        x: at[0],
        y: at[1],
        z: at[2],
    })
}

/// Certificate identity comes from the live district, not a caller-supplied
/// plugin owner or a saved readiness/stock projection.
fn certificate(
    world: &TickState,
    id: SettlementId,
    reference: &ScriptSettlementBuildingRef,
) -> Result<crate::plugin_structures::BuildingFacts, Failure> {
    let plan = world
        .settlements
        .get(id)
        .and_then(|record| record.district.as_ref())
        .ok_or(Failure::Forbidden)?;
    if !plan.buildings.contains(reference) {
        return Err(Failure::Forbidden);
    }
    let facts = crate::plugin_structures::building_facts(
        world,
        &plan.owner,
        std::iter::once((reference.id.as_str(), reference.kind == "worksite")),
    )?
    .pop()
    .ok_or(Failure::NotFound)?;
    if facts.dimension != world.dimension.name()
        || facts
            .blueprint
            .as_ref()
            .is_some_and(|purpose| purpose != &reference.purpose)
    {
        return Err(Failure::Blocked);
    }
    Ok(facts)
}

pub(super) fn building(
    world: &mut TickState,
    id: SettlementId,
    reference: &ScriptSettlementBuildingRef,
) -> Result<crate::plugin_structures::BuildingFacts, Failure> {
    let facts = certificate(world, id, reference)?;
    if !crate::plugin_structures::check_building(world, &facts)? {
        return Err(Failure::Blocked);
    }
    Ok(facts)
}

/// Native authorization boundary shared by payroll, quotes and shipments.
/// Operators do not acquire arbitrary foreign players' inventories merely by
/// owning a settlement; player consent here is the native owner's exact UUID.
pub(super) fn authorize_endpoint(
    world: &mut TickState,
    id: SettlementId,
    endpoint: &inventory::Endpoint,
) -> Result<(), Failure> {
    let record = world.settlements.get(id).ok_or(Failure::NotFound)?;
    match endpoint {
        inventory::Endpoint::VillagerCarry { uuid }
        | inventory::Endpoint::VillagerEquipment { uuid } => {
            let slot = world
                .entities
                .slot_of_uuid(*uuid)
                .ok_or(Failure::NotFound)?;
            let body = world.entities.body(slot);
            let row = world.entities.row(slot);
            if !body.alive
                || row.villager.as_ref().is_none_or(|v| {
                    v.settlement != Some(id) || v.wandering || v.conversion_time.is_some()
                })
            {
                return Err(Failure::Forbidden);
            }
        }
        inventory::Endpoint::Player { uuid } => {
            if record
                .owner
                .as_deref()
                .and_then(|owner| owner.parse::<uuid::Uuid>().ok())
                != Some(*uuid)
            {
                return Err(Failure::Forbidden);
            }
            inventory::load(world, endpoint)?;
        }
        inventory::Endpoint::Container {
            dimension,
            position,
        } => {
            if dimension != world.dimension.name() {
                return Err(Failure::Forbidden);
            }
            let count = record
                .district
                .as_ref()
                .ok_or(Failure::Forbidden)?
                .buildings
                .len();
            for index in 0..count {
                let reference = &world
                    .settlements
                    .get(id)
                    .expect("native settlement")
                    .district
                    .as_ref()
                    .expect("native district")
                    .buildings[index];
                let facts = certificate(world, id, reference)?;
                for point in &facts.points {
                    if let mc_script::ScriptWorksiteInteractionPoint::Container { at } = point {
                        let at = mc_script::ScriptBlockPosition::new(at[0], at[1], at[2]);
                        if crate::plugin_inventories::container_cells(world, &at)?
                            .iter()
                            .any(|cell| [cell.x, cell.y, cell.z] == *position)
                        {
                            if !crate::plugin_structures::check_building(world, &facts)? {
                                return Err(Failure::Blocked);
                            }
                            return Ok(());
                        }
                    }
                }
            }
            return Err(Failure::Forbidden);
        }
    }
    Ok(())
}
