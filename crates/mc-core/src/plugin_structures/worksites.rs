use super::*;
use mc_script::ScriptWorksiteInteractionPoint as Point;

pub(super) fn facts(state: &TickState, at: [i32; 3]) -> Result<ScriptWorldBlockFacts, Failure> {
    let Some(id) = state.world.block_at(at[0], at[1], at[2]) else {
        return Ok(ScriptWorldBlockFacts::new(
            at,
            None,
            Vec::new(),
            false,
            false,
            false,
            false,
        ));
    };
    let definition = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .ok_or(Failure::InvalidRequest)?;
    let name = definition.block.id.as_str();
    let mut properties: Vec<_> = definition
        .properties
        .iter()
        .map(|(k, v)| ScriptWorldBlockProperty::new(k.clone(), v.clone()))
        .collect();
    properties.sort_by(|a, b| a.key.cmp(&b.key));
    let standable =
        crate::plugin_npcs::standable(state, state.dimension.name(), at).unwrap_or(false);
    Ok(ScriptWorldBlockFacts::new(
        at,
        Some(name.to_owned()),
        properties,
        standable,
        inventories::container_name(name)
            && state.block_entities.contains_key(&(at[0], at[1], at[2])),
        name.ends_with("_door"),
        name.ends_with("_bed"),
    ))
}
fn workstation(name: &str) -> bool {
    matches!(
        name,
        "minecraft:crafting_table"
            | "minecraft:furnace"
            | "minecraft:blast_furnace"
            | "minecraft:smoker"
            | "minecraft:campfire"
            | "minecraft:soul_campfire"
            | "minecraft:brewing_stand"
            | "minecraft:stonecutter"
            | "minecraft:smithing_table"
            | "minecraft:loom"
            | "minecraft:grindstone"
            | "minecraft:anvil"
    )
}
pub(super) fn planned_points(
    state: &TickState,
    instance: &BlueprintInstance,
) -> Result<Vec<Point>, Failure> {
    let mut cells = BTreeMap::new();
    for cell in instance.placed_blocks() {
        cells.insert(cell.pos, cell.state);
    }
    for stage in instance.blueprint().stages() {
        for cell in instance.placed_stage_blocks(stage) {
            cells.insert(cell.pos, cell.state);
        }
    }
    let mut points = BTreeMap::new();
    for (at, id) in cells {
        let definition = state
            .registry
            .blocks
            .by_id(id)
            .ok_or(Failure::InvalidRequest)?;
        let name = definition.block.id.as_str();
        let prop = |key: &str| {
            definition
                .properties
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        if name.ends_with("_door") && prop("half") == Some("lower") {
            points.insert(at, Point::Entrance { at });
        } else if inventories::container_name(name) {
            points.insert(at, Point::Container { at });
        } else if workstation(name) {
            points.insert(
                at,
                Point::Workstation {
                    at,
                    capability: name.to_owned(),
                },
            );
        } else if (name.ends_with("_bed") && prop("part") == Some("head"))
            || name == "minecraft:bell"
        {
            points.insert(
                at,
                Point::Block {
                    at,
                    block: name.to_owned(),
                },
            );
        }
    }
    for (at, _) in instance.placed_street_connections() {
        points.entry(at).or_insert(Point::Entrance { at });
    }
    if points.len() > MAX_WORKSITE_POINTS {
        return Err(Failure::Capacity);
    }
    Ok(points.into_values().collect())
}
fn adjacent(at: [i32; 3]) -> [[i32; 3]; 4] {
    [
        [at[0] - 1, at[1], at[2]],
        [at[0] + 1, at[1], at[2]],
        [at[0], at[1], at[2] - 1],
        [at[0], at[1], at[2] + 1],
    ]
}
pub(super) fn check(
    state: &mut TickState,
    dimension: &str,
    points: &[Point],
) -> Result<Vec<ScriptWorksitePointFailure>, Failure> {
    let mut blocked = BTreeMap::new();
    let mut entrances = Vec::new();
    for point in points {
        let at = point.at();
        let target = world(state, dimension)?;
        let facts = facts(target, at)?;
        let reason = match point {
            Point::Entrance { .. } => {
                if facts.block.is_none() {
                    Some(ScriptWorksiteFailure::Unloaded)
                } else if facts.standable {
                    entrances.push(at);
                    None
                } else if facts.door {
                    // Native villagers can open ordinary wood doors: query the
                    // actual adjacent stand positions through the shared planner.
                    let definition = target
                        .world
                        .block_at(at[0], at[1], at[2])
                        .and_then(|id| target.registry.blocks.by_id(mc_world::BlockStateId(id)))
                        .expect("loaded door");
                    if definition.block.id.as_str() == "minecraft:iron_door" {
                        Some(ScriptWorksiteFailure::BlockedEntrance)
                    } else {
                        let nearby: Vec<_> = adjacent(at)
                            .into_iter()
                            .filter(|p| {
                                crate::plugin_npcs::standable(state, dimension, *p) == Some(true)
                            })
                            .collect();
                        if nearby.is_empty() {
                            Some(ScriptWorksiteFailure::BlockedEntrance)
                        } else {
                            entrances.extend(nearby);
                            None
                        }
                    }
                } else {
                    Some(ScriptWorksiteFailure::BlockedEntrance)
                }
            }
            Point::Block { block, .. } => {
                if facts.block.is_none() {
                    Some(ScriptWorksiteFailure::Unloaded)
                } else if facts.block.as_ref() != Some(block) {
                    Some(ScriptWorksiteFailure::MissingBlock)
                } else {
                    None
                }
            }
            Point::Workstation { capability, .. } => {
                if facts.block.is_none() {
                    Some(ScriptWorksiteFailure::Unloaded)
                } else if facts.block.as_ref() != Some(capability)
                    || (capability.ends_with("campfire")
                        && facts
                            .properties
                            .iter()
                            .any(|p| p.key == "lit" && p.value == "false"))
                {
                    Some(ScriptWorksiteFailure::MissingWorkstation)
                } else {
                    None
                }
            }
            Point::Container { .. } => {
                if facts.block.is_none() {
                    Some(ScriptWorksiteFailure::Unloaded)
                } else if !facts.container {
                    Some(ScriptWorksiteFailure::MissingContainer)
                } else {
                    None
                }
            }
            _ => return Err(Failure::InvalidRequest),
        };
        if let Some(reason) = reason {
            blocked.insert(at, reason);
        }
    }
    for point in points {
        if matches!(point, Point::Entrance { .. }) || blocked.contains_key(&point.at()) {
            continue;
        }
        let candidates: Vec<_> = adjacent(point.at())
            .into_iter()
            .filter(|at| crate::plugin_npcs::standable(state, dimension, *at) == Some(true))
            .collect();
        if candidates.is_empty() {
            blocked.insert(point.at(), ScriptWorksiteFailure::Unwalkable);
            continue;
        }
        if entrances.is_empty() {
            continue;
        }
        let mut reachable = false;
        let mut unloaded = false;
        for entrance in &entrances {
            for candidate in &candidates {
                match crate::plugin_npcs::reachable(state, dimension, *entrance, *candidate) {
                    Ok(true) => {
                        reachable = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(Failure::Unloaded) => unloaded = true,
                    Err(error) => return Err(error),
                }
            }
            if reachable {
                break;
            }
        }
        if !reachable {
            blocked.insert(
                point.at(),
                if unloaded {
                    ScriptWorksiteFailure::Unloaded
                } else {
                    ScriptWorksiteFailure::Unwalkable
                },
            );
        }
    }
    Ok(blocked
        .into_iter()
        .map(|(at, reason)| ScriptWorksitePointFailure::new(at, reason))
        .collect())
}
