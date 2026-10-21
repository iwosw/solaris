//! Native settlements, households, districts and physical economic policy.
//! Native military uses resident equipment and the shared melee commit path.
mod registry;
pub use registry::{
    RecordImage, Registry, SettlementId, SettlementRecord, valid_name, validate_record,
};

pub(crate) mod allocator;
pub(crate) mod army;
pub(crate) mod autonomy;
pub mod blueprints;
pub(crate) mod building_commands;
pub mod buildings;
mod caravan;
pub(crate) mod commands;
mod commerce;
pub mod construction;
pub(crate) mod construction_crafting;
pub(crate) mod economy;
pub(crate) mod economy_commands;
pub(crate) mod economy_customization;
pub mod presentation;
mod production;
pub(crate) use crate::plugin_inventories::native as inventory;
pub mod districts;
pub mod households;
pub(crate) mod persistence;
pub mod startup;
pub(crate) mod ui;
pub(crate) mod ui_text;
pub(crate) fn tick(state: &mut TickState, changes: &mut crate::boundary::ChangeSet) -> u64 {
    let elapsed = allocator::tick(state, changes);
    construction::tick(state);
    army::tick(state);
    elapsed
}

use crate::boundary::EntityId;
use crate::tick::TickState;

/// Bind the exact native entity, never create a replacement NPC or roster row.
pub(crate) fn bind(
    state: &mut TickState,
    id: SettlementId,
    entity: EntityId,
) -> Result<(), &'static str> {
    let record = state.settlements.get(id).ok_or("settlement not found")?;
    let slot = state.entities.slot_of(entity).ok_or("villager not found")?;
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let villager = row
        .villager
        .as_ref()
        .filter(|villager| body.alive && !villager.wandering && villager.conversion_time.is_none())
        .ok_or("a living resident villager is required")?;
    if villager.settlement.is_some_and(|existing| existing != id) {
        return Err("villager already belongs to another settlement");
    }
    let pos = state.entities.pos(slot);
    let distance: f64 = pos
        .iter()
        .zip(record.anchor)
        .map(|(coordinate, anchor)| (coordinate - f64::from(anchor)).powi(2))
        .sum();
    if distance > f64::from(record.radius).powi(2) {
        return Err("villager is outside the settlement");
    }
    if villager.settlement == Some(id) {
        return Ok(());
    }
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .expect("validated villager")
        .settlement = Some(id);
    refresh_counts(state, [entity]);
    Ok(())
}

pub(crate) fn unbind(state: &mut TickState, entity: EntityId) -> Result<(), &'static str> {
    let slot = state.entities.slot_of(entity).ok_or("villager not found")?;
    let villager = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .ok_or("villager not found")?;
    if villager.settlement.is_none() {
        return Ok(());
    }
    if villager.soldier.is_some() {
        return Err("Dismiss soldier before unbinding");
    }
    households::detach(state, entity);
    let v = state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .expect("validated villager");
    let profession = v.data.profession;
    construction::detach(v);
    v.settlement = None;
    if v.data.profession != profession {
        crate::villager::sync_appearance(state, slot);
    }
    refresh_counts(state, [entity]);
    Ok(())
}

/// Clear membership and record in the same tick's entity and settlement WAL.
pub(crate) fn remove(state: &mut TickState, id: SettlementId) -> bool {
    if !construction::remove(state, id) {
        return false;
    }
    if !state.settlements.remove(id) {
        return false;
    }
    fn clear(world: &mut TickState, id: SettlementId) {
        for index in 0..world.entities.len() {
            let slot = crate::entities::EntitySlot(index as u32);
            if world
                .entities
                .row(slot)
                .villager
                .as_ref()
                .is_some_and(|v| v.settlement == Some(id))
            {
                let v = world
                    .entities
                    .row_mut(slot)
                    .villager
                    .as_mut()
                    .expect("matched villager");
                let profession = v.data.profession;
                construction::detach(v);
                v.settlement = None;
                v.household = None;
                v.soldier = None;
                if v.data.profession == crate::villager::VillagerProfession::Soldier {
                    v.data.profession = crate::villager::VillagerProfession::None;
                }
                if v.data.profession != profession {
                    crate::villager::sync_appearance(world, slot);
                }
            }
        }
    }
    clear(state, id);
    for world in state.other_dimensions.values_mut() {
        clear(world, id);
    }
    true
}

/// Retain authenticated names for offline presentation without changing the
/// UUID used for ownership checks. No string copies or dirty idle-tick images.
pub(crate) fn refresh_owner_names(state: &mut TickState) {
    let roster = std::sync::Arc::clone(&state.dimension_globals.roster);
    let refresh = |world: &mut TickState| {
        let connections = &world.connections;
        world.settlements.refresh_owner_names(|uuid| {
            connections
                .all_connections()
                .find_map(|connection| {
                    (connections.uuid_of(connection) == Some(uuid))
                        .then(|| {
                            connections
                                .entity_of(connection)
                                .and_then(|entity| connections.name_of(entity))
                        })
                        .flatten()
                })
                .or_else(|| {
                    roster
                        .iter()
                        .find(|player| player.uuid == uuid)
                        .map(|player| player.name.as_str())
                })
        });
    };
    for world in state.other_dimensions.values_mut() {
        refresh(world);
    }
    refresh(state);
}

/// Dimension ticks temporarily take sibling worlds out of the owner. Resolve
/// deletion against all restored worlds before the shared tick WAL is captured.
pub(crate) fn finish(state: &mut TickState) {
    refresh_owner_names(state);
    households::finish(state);
    let removed: Vec<_> = std::iter::once(&*state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .flat_map(|world| world.settlements.removed_ids())
        .collect();
    if removed.is_empty() {
        return;
    }
    fn clear(world: &mut TickState, removed: &[SettlementId]) {
        for index in 0..world.entities.len() {
            let slot = crate::entities::EntitySlot(index as u32);
            if world
                .entities
                .row(slot)
                .villager
                .as_ref()
                .and_then(|v| v.settlement)
                .is_some_and(|id| removed.contains(&id))
            {
                let v = world
                    .entities
                    .row_mut(slot)
                    .villager
                    .as_mut()
                    .expect("matched villager");
                let profession = v.data.profession;
                construction::detach(v);
                v.settlement = None;
                v.household = None;
                v.soldier = None;
                if v.data.profession == crate::villager::VillagerProfession::Soldier {
                    v.data.profession = crate::villager::VillagerProfession::None;
                }
                if v.data.profession != profession {
                    crate::villager::sync_appearance(world, slot);
                }
            }
        }
        rebuild_counts(world);
    }
    clear(state, &removed);
    for world in state.other_dimensions.values_mut() {
        clear(world, &removed);
    }
}

pub(crate) fn member_count(state: &TickState, id: SettlementId) -> usize {
    state.settlements.member_count(id)
}

/// Counts are occupancy projections, not a second membership authority.
/// Absent/dead members do not occupy local settlement housing.
pub(crate) fn rebuild_counts(state: &mut TickState) {
    state.settlements.clear_counts();
    for slot in state.entities.slots() {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if body.alive
            && let Some(id) = row.villager.as_ref().and_then(|v| v.settlement)
        {
            if state.settlements.get(id).is_some() {
                state.settlements.update_resident(body.id, Some(id));
            }
            state.settlements.update_household(
                body.id,
                row.villager
                    .as_ref()
                    .and_then(|v| v.household.as_deref())
                    .map(|family| (id, family)),
                false,
            );
        }
    }
    state.settlements.grow_radii();
}

/// Incremental twin of [`rebuild_counts`]: refresh only the entities the
/// parent tick already knows changed (moved/died/removed residents included).
/// Changes each id's cached share when its current living membership differs.
/// Counting stays exactly the old alive scan (conversion/wandering keep
/// their share); only command-time bind validation filters those.
pub(crate) fn refresh_counts(state: &mut TickState, changed: impl IntoIterator<Item = EntityId>) {
    for entity in changed {
        let membership = state.entities.slot_of_any(entity).and_then(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            body.alive
                .then(|| row.villager.as_ref().and_then(|v| v.settlement))
                .flatten()
                .filter(|&id| state.settlements.get(id).is_some())
        });
        state.settlements.update_resident(entity, membership);
        let household = state.entities.slot_of_any(entity).and_then(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            body.alive
                .then(|| {
                    let v = row.villager.as_ref()?;
                    Some((v.settlement?, v.household.as_deref()?))
                })
                .flatten()
        });
        state.settlements.update_household(entity, household, true);
    }
    state.settlements.grow_radii();
}

/// Run only after all dimension registries and entity columns have hydrated.
pub(crate) fn validate_memberships(state: &TickState) -> Result<(), crate::CoreError> {
    let mut ids = std::collections::BTreeSet::new();
    let mut families = std::collections::BTreeMap::new();
    for world in std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        for record in world.settlements.records() {
            validate_record(record).map_err(|error| crate::CoreError::Storage(error.into()))?;
            if !ids.insert(record.id) {
                return Err(crate::CoreError::Storage(format!(
                    "duplicate global settlement identity {}",
                    record.id.0
                )));
            }
            for household in record.households.keys() {
                if families.insert(household.as_str(), record.id).is_some() {
                    return Err(crate::CoreError::Storage(format!(
                        "duplicate global household identity {household}"
                    )));
                }
            }
        }
    }
    for world in std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        for slot in world.entities.slots() {
            let body = world.entities.body(slot);
            let row = world.entities.row(slot);
            if let Some(id) = row.villager.as_ref().and_then(|v| v.settlement)
                && !ids.contains(&id)
            {
                return Err(crate::CoreError::Storage(format!(
                    "{}: villager {} references missing settlement {}",
                    world.dimension.name(),
                    body.id.0,
                    id.0
                )));
            }
            if let Some(v) = row.villager.as_ref()
                && let Some(household) = v.household.as_deref()
                && (v.settlement.is_none() || families.get(household).copied() != v.settlement)
            {
                return Err(crate::CoreError::Storage(format!(
                    "villager {} references missing household {household}",
                    body.id.0
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "binding_tests.rs"]
mod binding_tests;
