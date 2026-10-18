//! Native district plans; readiness is a live observation, never stored capacity.
use std::collections::{BTreeMap, BTreeSet};

use mc_script::{
    ScriptOperationFailure as Failure, ScriptSettlementBuildingRef,
    ScriptSettlementDistrictSnapshot, ScriptWorksiteInteractionPoint as Point,
};
use serde::{Deserialize, Serialize};

use super::SettlementId;
use crate::tick::TickState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistrictPlan {
    pub owner: String,
    pub branch: String,
    pub level: u8,
    pub buildings: Vec<ScriptSettlementBuildingRef>,
}

pub(super) fn validate(plan: &DistrictPlan) -> Result<(), &'static str> {
    if plan.owner.is_empty() || plan.owner.len() > 128 {
        return Err("invalid district building owner");
    }
    ScriptSettlementDistrictSnapshot::validate_plan(&plan.branch, plan.level, &plan.buildings)
        .map_err(|_| "invalid native district plan")
}

fn institutions(branch: &str, level: u8) -> &'static [&'static str] {
    match (branch, level) {
        ("estate", 1) => &["solaris:manor_hall", "solaris:farm", "solaris:warehouse"],
        ("estate", 2) => &[
            "solaris:manor_hall",
            "solaris:farm",
            "solaris:warehouse",
            "solaris:town_hall",
        ],
        ("fortress", 1) => &[
            "solaris:stone_tower",
            "solaris:stone_wall",
            "solaris:warehouse",
        ],
        ("fortress", 2) => &[
            "solaris:stone_tower",
            "solaris:stone_wall",
            "solaris:warehouse",
            "solaris:keep",
        ],
        ("town", 1) => &["solaris:market", "solaris:smithy", "solaris:warehouse"],
        ("town", 2) => &[
            "solaris:market",
            "solaris:smithy",
            "solaris:warehouse",
            "solaris:town_hall",
        ],
        _ => &[],
    }
}

fn validate_purpose(purpose: &str, points: &[Point]) -> Result<(), Failure> {
    if !points.iter().any(|p| matches!(p, Point::Entrance { .. })) {
        return Err(Failure::InvalidRequest);
    }
    if matches!(
        purpose,
        "solaris:workshop" | "solaris:smithy" | "solaris:sawmill"
    ) && !points
        .iter()
        .any(|p| matches!(p, Point::Workstation { .. }))
    {
        return Err(Failure::InvalidRequest);
    }
    if matches!(purpose, "solaris:warehouse" | "solaris:granary")
        && !points.iter().any(|p| matches!(p, Point::Container { .. }))
    {
        return Err(Failure::InvalidRequest);
    }
    Ok(())
}

fn within(anchor: [i32; 3], radius: u32, bounds: mc_script::ScriptWorldBounds) -> bool {
    [bounds.min[0], bounds.max[0]].into_iter().all(|x| {
        [bounds.min[1], bounds.max[1]].into_iter().all(|y| {
            [bounds.min[2], bounds.max[2]].into_iter().all(|z| {
                [x, y, z]
                    .into_iter()
                    .zip(anchor)
                    .map(|(value, anchor)| (i64::from(value) - i64::from(anchor)).pow(2))
                    .sum::<i64>()
                    <= i64::from(radius).pow(2)
            })
        })
    })
}

/// Bind existing native certificates. No entity, block, POI or inventory writes.
pub(crate) fn plan(
    state: &mut TickState,
    id: SettlementId,
    expected_revision: u64,
    proposal: DistrictPlan,
) -> Result<(), Failure> {
    validate(&proposal).map_err(|_| Failure::InvalidRequest)?;
    let record = state.settlements.get(id).ok_or(Failure::NotFound)?;
    if record.revision != expected_revision {
        return Err(Failure::StaleRevision);
    }
    if record
        .district
        .as_ref()
        .is_some_and(|old| old.branch != "none" && old.branch != proposal.branch)
    {
        return Err(Failure::InvalidRequest);
    }
    let revision = record
        .revision
        .checked_add(1)
        .filter(|n| *n <= mc_script::MAX_SCRIPT_WORLD_TIME)
        .ok_or(Failure::Capacity)?;
    let facts = crate::plugin_structures::building_facts(
        state,
        &proposal.owner,
        proposal
            .buildings
            .iter()
            .map(|building| (building.id.as_str(), building.kind == "worksite")),
    )?;
    for (building, facts) in proposal.buildings.iter().zip(facts) {
        if state
            .settlements
            .records()
            .filter(|other| other.id != id)
            .any(|other| {
                other.district.as_ref().is_some_and(|district| {
                    district.owner == proposal.owner
                        && district
                            .buildings
                            .iter()
                            .any(|bound| bound.kind == building.kind && bound.id == building.id)
                })
            })
        {
            return Err(Failure::InvalidRequest);
        }
        if facts.dimension != state.dimension.name()
            || !within(record.anchor, record.radius, facts.footprint)
            || facts
                .blueprint
                .as_ref()
                .is_some_and(|purpose| purpose != &building.purpose)
        {
            return Err(Failure::InvalidRequest);
        }
        validate_purpose(&building.purpose, &facts.points)?;
    }
    let record = state.settlements.get_mut(id).expect("validated settlement");
    record.district = Some(proposal);
    record.revision = revision;
    Ok(())
}

/// One query pass over native columns. A shared/invalid/unloaded bed is not housing.
pub(crate) fn population(state: &TickState, id: SettlementId) -> (u32, u32) {
    let mut population = 0;
    let mut homes = BTreeMap::new();
    for slot in state.entities.slots() {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if !body.alive {
            continue;
        }
        let Some(v) = row.villager.as_ref() else {
            continue;
        };
        let member = v.settlement == Some(id);
        if member {
            population += 1;
        }
        if v.home_dimension != state.dimension.name() {
            continue;
        }
        if let Some(home) = v.home {
            match homes.entry((home.x, home.y, home.z)) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.insert(false);
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(member && super::allocator::bed_has_baby_space(state, home));
                }
            }
        }
    }
    (
        population,
        homes.into_values().filter(|valid| *valid).count() as u32,
    )
}

pub(crate) fn observe(
    state: &mut TickState,
    id: SettlementId,
    anchor: [i32; 3],
    radius: u32,
    plan: Option<&DistrictPlan>,
) -> Option<ScriptSettlementDistrictSnapshot> {
    let plan = plan?;
    let mut observation = ScriptSettlementDistrictSnapshot::new(
        plan.branch.clone(),
        plan.level,
        plan.buildings.clone(),
    );
    (observation.population, observation.housed) = population(state, id);
    observation.observed_tick = state.tick;
    observation.world_revision = state.world.revision_now();
    let mut working = BTreeSet::new();
    let mut containers = BTreeSet::new();
    let inspection =
        (|| -> Result<(), Failure> {
            let facts = crate::plugin_structures::building_facts(
                state,
                &plan.owner,
                plan.buildings
                    .iter()
                    .map(|building| (building.id.as_str(), building.kind == "worksite")),
            )?;
            for (building, facts) in plan.buildings.iter().zip(facts) {
                if facts.dimension != state.dimension.name()
                    || !within(anchor, radius, facts.footprint)
                    || facts
                        .blueprint
                        .as_ref()
                        .is_some_and(|purpose| purpose != &building.purpose)
                {
                    return Err(Failure::InvalidRequest);
                }
                validate_purpose(&building.purpose, &facts.points)?;
                if !crate::plugin_structures::check_building(state, &facts)? {
                    return Err(Failure::Blocked);
                }
                working.insert(building.purpose.clone());
                for point in &facts.points {
                    if let Point::Container { at } = point {
                        let position = mc_script::ScriptBlockPosition::new(at[0], at[1], at[2]);
                        for cell in crate::plugin_inventories::container_cells(state, &position)? {
                            if !containers.insert((cell.x, cell.y, cell.z)) {
                                continue;
                            }
                            for stack in &state.block_entities[&(cell.x, cell.y, cell.z)].slots {
                                if stack.count > 0
                                    && state.registry.items.name_of(stack.item_id).is_some_and(
                                        |name| {
                                            matches!(
                                                name.as_str(),
                                                "minecraft:bread"
                                                    | "minecraft:carrot"
                                                    | "minecraft:potato"
                                                    | "minecraft:beetroot"
                                            )
                                        },
                                    )
                                {
                                    observation.food += stack.count as u64;
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        })();
    observation.working = working.into_iter().collect();
    observation.reason = if let Err(failure) = inspection {
        Some(format!(
            "Native district building unavailable: {failure:?}."
        ))
    } else if observation.population == 0 {
        Some("No actual native residents.".into())
    } else if observation.housed < observation.population {
        Some("Actual native housing unavailable or insufficient.".into())
    } else if observation.food < u64::from(observation.population) {
        Some("Real local food stock insufficient.".into())
    } else {
        let stage = if observation.population >= 64 {
            "developed"
        } else if observation.population >= 24 {
            "village"
        } else {
            "hamlet"
        };
        if plan.branch != "none" && stage != "developed" {
            Some("Developed population prerequisites not met; no people are created to meet a range.".into())
        } else if !institutions(&plan.branch, plan.level)
            .iter()
            .all(|purpose| observation.working.iter().any(|working| working == purpose))
        {
            Some("Normal working district building/economic institution missing.".into())
        } else {
            observation.stage = Some(stage.into());
            observation.branch = plan.branch.clone();
            observation.level = plan.level;
            None
        }
    };
    Some(observation)
}

#[cfg(test)]
#[path = "district_tests.rs"]
mod tests;
