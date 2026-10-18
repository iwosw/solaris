//! Household policy records; membership and birth provenance live on villagers.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::SettlementId;
use crate::boundary::EntityId;
use crate::tick::TickState;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HouseholdOrigin {
    #[default]
    Adoption,
    Migration,
    Split,
    Refugee,
}

impl HouseholdOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Adoption => "adoption",
            Self::Migration => "migration",
            Self::Split => "split",
            Self::Refugee => "refugee",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Household {
    pub id: String,
    pub revision: u64,
    pub origin: HouseholdOrigin,
    /// Social home anchor, not an extra bed claim. Each member owns its bed.
    pub home: Option<[i32; 3]>,
    pub needs: Option<String>,
    pub policy: Option<String>,
}

pub(super) fn validate_record(record: &super::SettlementRecord) -> Result<(), &'static str> {
    for (id, household) in &record.households {
        let (founder, sequence) = id.split_once('-').ok_or("invalid household identity")?;
        let founder: u64 = founder.parse().map_err(|_| "invalid household identity")?;
        let sequence: u64 = sequence.parse().map_err(|_| "invalid household identity")?;
        if founder == 0
            || sequence == 0
            || id != &format!("{founder}-{sequence}")
            || id != &household.id
            || household.revision == 0
            || (founder == record.id.0 && sequence > record.household_sequence)
        {
            return Err("invalid household identity or revision");
        }
        if [&household.needs, &household.policy]
            .into_iter()
            .flatten()
            .any(|value| value.is_empty() || value.len() > 128)
        {
            return Err("invalid household policy reference");
        }
    }
    Ok(())
}

pub(crate) fn members(
    state: &TickState,
    settlement: SettlementId,
    household: &str,
) -> Vec<EntityId> {
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            let v = row.villager.as_ref()?;
            (body.alive
                && v.settlement == Some(settlement)
                && v.household.as_deref() == Some(household))
            .then_some(body.id)
        })
        .collect()
}

pub(crate) fn form(
    state: &mut TickState,
    settlement: SettlementId,
    members: &[EntityId],
    origin: HouseholdOrigin,
) -> Result<String, &'static str> {
    let record = state
        .settlements
        .get(settlement)
        .ok_or("settlement not found")?;
    if members.is_empty() {
        return Err("household needs at least one member");
    }
    let mut distinct = BTreeSet::new();
    let mut home = None;
    for &entity in members {
        if !distinct.insert(entity) {
            return Err("duplicate household member");
        }
        let slot = state.entities.slot_of(entity).ok_or("villager not found")?;
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        let v = row
            .villager
            .as_ref()
            .filter(|v| body.alive && !v.wandering && v.conversion_time.is_none())
            .ok_or("a living resident villager is required")?;
        if v.settlement != Some(settlement) {
            return Err("villager belongs to another settlement");
        }
        if v.household.is_some() {
            return Err("villager already belongs to a household");
        }
        home = home.or(v.home.map(|p| [p.x, p.y, p.z]));
    }
    let sequence = record
        .household_sequence
        .checked_add(1)
        .ok_or("household identity exhausted")?;
    let revision = record
        .revision
        .checked_add(1)
        .ok_or("settlement revision exhausted")?;
    let id = format!("{}-{sequence}", settlement.0);
    let record = state
        .settlements
        .get_mut(settlement)
        .expect("validated settlement");
    record.household_sequence = sequence;
    record.revision = revision;
    record.households.insert(
        id.clone(),
        Household {
            id: id.clone(),
            revision: 1,
            origin,
            home,
            needs: None,
            policy: None,
        },
    );
    for &entity in members {
        let slot = state.entities.slot_of(entity).expect("validated member");
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .household = Some(id.clone());
    }
    state.settlements.set_household_size(&id, members.len());
    super::refresh_counts(state, members.iter().copied());
    Ok(id)
}

/// Remove a social binding without removing the resident from its settlement.
/// Empty households retire; their monotonically allocated identity never returns.
pub(crate) fn detach(state: &mut TickState, entity: EntityId) {
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return;
    };
    let Some((settlement, household)) = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .and_then(|v| Some((v.settlement?, v.household.clone()?)))
    else {
        return;
    };
    if state.entities.body(slot).alive
        && state
            .settlements
            .get(settlement)
            .is_some_and(|record| record.households.contains_key(&household))
    {
        let size = state.settlements.household_size(&household);
        state
            .settlements
            .set_household_size(&household, size.saturating_sub(1));
    }
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .household = None;
    super::refresh_counts(state, [entity]);
    if let Some(record) = state.settlements.get_mut(settlement) {
        record.revision = record.revision.saturating_add(1);
        if let Some(family) = record.households.get_mut(&household) {
            family.revision = family.revision.saturating_add(1);
        }
    }
    // Sibling dimensions may be temporarily outside the tick owner. Retire only
    // when the shared commit tail can observe every living member.
    state.settlements.queue_household_change(household);
}

/// Recovery has already rebuilt each dimension's local projections. Summing
/// those maps counts visitors without scanning or persisting another roster.
pub(super) fn rebuild_sizes(state: &mut TickState) {
    let sizes = {
        let mut sizes = std::collections::BTreeMap::new();
        for world in
            std::iter::once(&*state).chain(state.other_dimensions.values().map(|world| &**world))
        {
            for record in world.settlements.records() {
                for household in record.households.keys() {
                    let size = std::iter::once(&*state)
                        .chain(state.other_dimensions.values().map(|world| &**world))
                        .map(|world| world.settlements.household_count(record.id, household))
                        .sum();
                    sizes.insert(household.clone(), size);
                }
            }
        }
        sizes
    };
    state.settlements.rebuild_household_sizes(&sizes);
    for world in state.other_dimensions.values_mut() {
        world.settlements.rebuild_household_sizes(&sizes);
    }
}

/// Reconcile only explicitly changed households once all dimension owners are
/// restored. Idle ticks never inspect the population or produce household WAL.
pub(super) fn finish(state: &mut TickState) {
    let mut pending = state.settlements.take_household_changes();
    for world in state.other_dimensions.values_mut() {
        pending.extend(world.settlements.take_household_changes());
    }
    for household in pending {
        let owner = std::iter::once(&*state)
            .chain(state.other_dimensions.values().map(|world| &**world))
            .flat_map(|world| world.settlements.records())
            .find_map(|record| {
                record
                    .households
                    .contains_key(&household)
                    .then_some(record.id)
            });
        let Some(owner) = owner else { continue };
        let size = std::iter::once(&*state)
            .chain(state.other_dimensions.values().map(|world| &**world))
            .map(|world| {
                world
                    .entities
                    .slots()
                    .filter(|&slot| {
                        let body = world.entities.body(slot);
                        let row = world.entities.row(slot);
                        body.alive
                            && row
                                .villager
                                .as_ref()
                                .is_some_and(|v| v.household.as_deref() == Some(household.as_str()))
                    })
                    .count()
            })
            .sum();
        reconcile(state, owner, &household, size);
        for world in state.other_dimensions.values_mut() {
            reconcile(world, owner, &household, size);
        }
    }
    // Reconciliation only rewrites these same households on the single owner;
    // discard its derived notifications rather than rescan them next tick.
    state.settlements.take_household_changes();
    for world in state.other_dimensions.values_mut() {
        world.settlements.take_household_changes();
    }
}

fn reconcile(world: &mut TickState, owner: SettlementId, household: &str, size: usize) {
    let mut moved = Vec::new();
    for index in 0..world.entities.len() {
        let slot = crate::entities::EntitySlot(index as u32);
        let body = world.entities.body(slot);
        let row = world.entities.row(slot);
        let Some(v) = row
            .villager
            .as_ref()
            .filter(|v| v.household.as_deref() == Some(household))
        else {
            continue;
        };
        if !body.alive {
            world
                .entities
                .row_mut(slot)
                .villager
                .as_mut()
                .unwrap()
                .household = None;
        } else if v.settlement != Some(owner) {
            let id = body.id;
            world
                .entities
                .row_mut(slot)
                .villager
                .as_mut()
                .unwrap()
                .settlement = Some(owner);
            moved.push(id);
        }
    }
    if !moved.is_empty() {
        super::allocator::release_members(world, &moved);
        super::refresh_counts(world, moved);
    }
    if world
        .settlements
        .get(owner)
        .is_some_and(|record| record.households.contains_key(household))
    {
        if size == 0 || world.settlements.household_size(household) != size {
            let record = world.settlements.get_mut(owner).unwrap();
            record.revision = record.revision.saturating_add(1);
            if size > 0 {
                let family = record.households.get_mut(household).unwrap();
                family.revision = family.revision.saturating_add(1);
            } else {
                record.households.remove(household);
            }
        }
        world.settlements.set_household_size(household, size);
    }
}

/// Move the same people and household identity. The allocator releases the old
/// memories; its normal budgeted path admission takes reachable destination POIs.
/// No entity is spawned, teleported, or placed under plugin custody.
pub(crate) fn relocate(
    state: &mut TickState,
    source: SettlementId,
    household: &str,
    destination: SettlementId,
) -> Result<(), &'static str> {
    if source == destination {
        return Err("relocation needs another settlement");
    }
    let from = state
        .settlements
        .get(source)
        .ok_or("settlement not found")?;
    let family = from
        .households
        .get(household)
        .ok_or("household not found")?;
    let to = state
        .settlements
        .get(destination)
        .ok_or("destination settlement not found")?;
    if to.households.contains_key(household) {
        return Err("destination household identity conflict");
    }
    let from_revision = from
        .revision
        .checked_add(1)
        .ok_or("settlement revision exhausted")?;
    let to_revision = to
        .revision
        .checked_add(1)
        .ok_or("settlement revision exhausted")?;
    let revision = family
        .revision
        .checked_add(1)
        .ok_or("household revision exhausted")?;
    let members = members(state, source, household);
    if members.is_empty() {
        return Err("household has no living members");
    }
    let mut family = state
        .settlements
        .get_mut(source)
        .unwrap()
        .households
        .remove(household)
        .unwrap();
    state.settlements.get_mut(source).unwrap().revision = from_revision;
    family.revision = revision;
    family.home = None;
    let to = state.settlements.get_mut(destination).unwrap();
    to.revision = to_revision;
    to.households.insert(household.to_owned(), family);
    for &entity in &members {
        let slot = state.entities.slot_of(entity).unwrap();
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .settlement = Some(destination);
    }
    state
        .settlements
        .queue_household_change(household.to_owned());
    super::allocator::release_members(state, &members);
    super::refresh_counts(state, members);
    Ok(())
}

/// Identity compatibility only. The native courtship branch must secure an
/// actual free bed with baby clearance before it consumes food or spawns.
/// Household membership has no authored population limit.
pub(crate) fn can_breed(state: &TickState, parents: [EntityId; 2]) -> bool {
    let Some(a) = state
        .entities
        .slot_of(parents[0])
        .and_then(|s| state.entities.row(s).villager.as_ref())
    else {
        return false;
    };
    let Some(b) = state
        .entities
        .slot_of(parents[1])
        .and_then(|s| state.entities.row(s).villager.as_ref())
    else {
        return false;
    };
    if a.settlement != b.settlement {
        return false;
    }
    let Some(settlement) = a.settlement else {
        return a.household.is_none() && b.household.is_none();
    };
    let Some(record) = state.settlements.get(settlement) else {
        return false;
    };
    match (a.household.as_deref(), b.household.as_deref()) {
        (Some(a), Some(b)) if a != b => false,
        (Some(id), _) | (_, Some(id)) => record.households.contains_key(id),
        (None, None) => true,
    }
}

/// The sole caller is the synchronous brain birth branch, immediately after
/// `can_breed` admission. No other writer or pair can interleave that branch;
/// it refreshes the derived counts before the next pair's admission.
pub(crate) fn inherit_birth(state: &mut TickState, child: EntityId, parents: [EntityId; 2]) {
    let first = state.entities.slot_of(parents[0]).expect("birth parent");
    let second = state.entities.slot_of(parents[1]).expect("birth parent");
    let a = state.entities.row(first).villager.as_ref().unwrap();
    let Some(settlement) = a.settlement else {
        return;
    };
    let household = a.household.clone().or_else(|| {
        state
            .entities
            .row(second)
            .villager
            .as_ref()
            .unwrap()
            .household
            .clone()
    });
    let added = if household.is_some() {
        1 + usize::from(a.household.is_none())
            + usize::from(
                state
                    .entities
                    .row(second)
                    .villager
                    .as_ref()
                    .unwrap()
                    .household
                    .is_none(),
            )
    } else {
        1
    };
    let household = household.unwrap_or_else(|| {
        form(state, settlement, &parents, HouseholdOrigin::Adoption)
            .expect("admitted birth household")
    });
    let size = state.settlements.household_size(&household) + added;
    for entity in parents.into_iter().chain([child]) {
        let slot = state.entities.slot_of(entity).unwrap();
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.settlement = Some(settlement);
        v.household = Some(household.clone());
    }
    let record = state.settlements.get_mut(settlement).unwrap();
    record.revision = record.revision.saturating_add(1);
    record.households.get_mut(&household).unwrap().revision += 1;
    super::refresh_counts(state, parents.into_iter().chain([child]));
    state.settlements.set_household_size(&household, size);
}

#[cfg(test)]
#[path = "household_tests.rs"]
mod tests;
