//! Generic package blueprints, native terrain observations and staged construction.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use mc_script::{ScriptOperationFailure as Failure, *};
use mc_worldgen::{Blueprint, BlueprintCatalog, BlueprintInstance, QuarterTurn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::boundary::BlockPos;
use crate::plugin_inventories::{self as inventories, world, world_mut};
use crate::tick::TickState;

#[path = "plugin_structures/features.rs"]
mod features;
#[path = "plugin_structures/worksites.rs"]
mod worksites;

const OWNER: &str = "@native-structures";
const KEY: &str = "ledger";
pub(crate) type Catalogs = BTreeMap<String, BlueprintCatalog>;

/// Only package roots discovered by the strict host loader reach this function.
pub(crate) fn load_catalog(
    roots: &BTreeMap<String, PathBuf>,
    registry: &mc_world::BlockRegistry,
) -> Result<Catalogs, crate::config::CoreError> {
    let mut catalogs = BTreeMap::new();
    let mut identities = BTreeSet::new();
    for (owner, root) in roots {
        let dir = root.join("structures");
        let metadata = match std::fs::symlink_metadata(&dir) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(crate::config::CoreError::Plugin(format!(
                    "structure catalog {}: {error}",
                    dir.display()
                )));
            }
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(crate::config::CoreError::Plugin(format!(
                "structure catalog {} is not a native directory",
                dir.display()
            )));
        }
        let mut files = Vec::new();
        let mut bytes = 0_usize;
        let entries = std::fs::read_dir(&dir).map_err(|error| {
            crate::config::CoreError::Plugin(format!(
                "structure catalog {}: {error}",
                dir.display()
            ))
        })?;
        for entry in entries {
            let entry =
                entry.map_err(|error| crate::config::CoreError::Plugin(error.to_string()))?;
            let kind = entry
                .file_type()
                .map_err(|error| crate::config::CoreError::Plugin(error.to_string()))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| crate::config::CoreError::Plugin("non-UTF8 structure file".into()))?;
            if !kind.is_file() || kind.is_symlink() || !name.ends_with(".toml") {
                return Err(crate::config::CoreError::Plugin(format!(
                    "invalid structure catalog entry {name}"
                )));
            }
            if files.len() >= mc_worldgen::blueprint::MAX_BLUEPRINTS {
                return Err(crate::config::CoreError::Plugin(
                    "too many blueprint files".into(),
                ));
            }
            let metadata = entry
                .metadata()
                .map_err(|error| crate::config::CoreError::Plugin(error.to_string()))?;
            let size = usize::try_from(metadata.len())
                .map_err(|_| crate::config::CoreError::Plugin("blueprint size overflow".into()))?;
            bytes = bytes
                .checked_add(size)
                .filter(|n| *n <= mc_worldgen::blueprint::MAX_CATALOG_DECODED_BYTES)
                .ok_or_else(|| {
                    crate::config::CoreError::Plugin(
                        "blueprint catalog exceeds decoded byte budget".into(),
                    )
                })?;
            let text = std::fs::read_to_string(entry.path()).map_err(|error| {
                crate::config::CoreError::Plugin(format!("structure {name}: {error}"))
            })?;
            files.push((name, text));
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let namespace = catalog_namespace(&files).unwrap_or_else(|| owner.clone());
        let catalog = BlueprintCatalog::from_files(registry, &namespace, &files)
            .map_err(|error| crate::config::CoreError::Plugin(error.to_string()))?;
        for blueprint in catalog.blueprints() {
            if !identities.insert(blueprint.id().to_owned()) {
                return Err(crate::config::CoreError::Plugin(format!(
                    "duplicate blueprint {}",
                    blueprint.id()
                )));
            }
        }
        catalogs.insert(owner.clone(), catalog);
    }
    Ok(catalogs)
}
// Match the deployed catalog contract: one authored namespace per package,
// independent of its plugin id; mixed namespaces fail catalog validation.
fn catalog_namespace(files: &[(String, String)]) -> Option<String> {
    files.iter().find_map(|(_, text)| {
        text.lines().find_map(|line| {
            let rest = line
                .trim()
                .strip_prefix("id")?
                .trim_start()
                .strip_prefix('=')?
                .trim_start()
                .strip_prefix('"')?;
            let (id, _) = rest.split_once('"')?;
            id.split_once(':')
                .map(|(namespace, _)| namespace.to_owned())
        })
    })
}

#[derive(Default, Serialize, Deserialize)]
struct Ledger {
    revision: u64,
    surveys: BTreeMap<String, Survey>,
    structures: BTreeMap<String, Structure>,
    worksites: BTreeMap<String, Worksite>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Survey {
    owner: String,
    snapshot: ScriptSurveySnapshot,
    image: String,
    expires: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Cell {
    at: [i32; 3],
    state: u32,
    resource: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Structure {
    owner: String,
    snapshot: ScriptStructureSnapshot,
    blueprint_hash: String,
    image: String,
    cells: Vec<Vec<Cell>>,
    receipts: Vec<ScriptStructureReceipt>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Worksite {
    owner: String,
    dimension: String,
    snapshot: ScriptWorksiteSnapshot,
}

/// Existing native building authority, projected without exposing its ledger.
pub(crate) struct BuildingFacts {
    pub dimension: String,
    pub footprint: ScriptWorldBounds,
    pub points: Vec<ScriptWorksiteInteractionPoint>,
    pub blueprint: Option<String>,
}

pub(crate) fn building_facts<'a>(
    state: &TickState,
    owner: &str,
    references: impl Iterator<Item = (&'a str, bool)>,
) -> Result<Vec<BuildingFacts>, Failure> {
    let mut ledger = ledger(state)?;
    references
        .map(|(id, worksite)| {
            if worksite {
                let record = ledger.worksites.remove(id).ok_or(Failure::NotFound)?;
                if record.owner != owner {
                    return Err(Failure::Forbidden);
                }
                Ok(BuildingFacts {
                    dimension: record.dimension,
                    footprint: record.snapshot.footprint,
                    points: record.snapshot.interaction_points,
                    blueprint: None,
                })
            } else {
                let record = ledger.structures.remove(id).ok_or(Failure::NotFound)?;
                if record.owner != owner {
                    return Err(Failure::Forbidden);
                }
                if record.snapshot.state != ScriptStructureState::Committed {
                    return Err(Failure::Blocked);
                }
                let footprint = bounds_of_structure(&record)?;
                Ok(BuildingFacts {
                    dimension: record.snapshot.dimension,
                    footprint,
                    points: record.snapshot.interaction_points,
                    blueprint: Some(record.snapshot.blueprint_id),
                })
            }
        })
        .collect()
}

pub(crate) fn check_building(
    state: &mut TickState,
    facts: &BuildingFacts,
) -> Result<bool, Failure> {
    worksites::check(state, &facts.dimension, &facts.points).map(|blocked| blocked.is_empty())
}
fn ledger(state: &TickState) -> Result<Ledger, Failure> {
    state
        .plugins
        .get_domain(OWNER, KEY)
        .map(|value| Ledger::deserialize(value).map_err(|_| Failure::RuntimeUnavailable))
        .unwrap_or_else(|| Ok(Ledger::default()))
}
fn save(state: &mut TickState, ledger: &Ledger) {
    state.plugins.set_domain(
        OWNER,
        KEY,
        serde_json::to_value(ledger).expect("native structure ledger serialization"),
    );
}
fn next(value: u64) -> Result<u64, Failure> {
    value
        .checked_add(1)
        .filter(|v| *v <= MAX_SCRIPT_WORLD_TIME)
        .ok_or(Failure::Capacity)
}
fn bounds(origin: [i32; 3], size: [i32; 3]) -> Result<ScriptWorldBounds, Failure> {
    let mut max = origin;
    for axis in 0..3 {
        max[axis] = origin[axis]
            .checked_add(size[axis] - 1)
            .ok_or(Failure::InvalidRequest)?;
    }
    ScriptWorldBounds::new(origin, max).map_err(|_| Failure::InvalidRequest)
}
fn overlaps(a: ScriptWorldBounds, b: ScriptWorldBounds) -> bool {
    (0..3).all(|i| a.min[i] <= b.max[i] && b.min[i] <= a.max[i])
}
fn contains(a: ScriptWorldBounds, b: ScriptWorldBounds) -> bool {
    (0..3).all(|i| a.min[i] <= b.min[i] && b.max[i] <= a.max[i])
}
fn fingerprint(state: &TickState, bounds: ScriptWorldBounds) -> Result<String, Failure> {
    let mut hash = Sha256::new();
    for z in bounds.min[2]..=bounds.max[2] {
        for x in bounds.min[0]..=bounds.max[0] {
            for y in bounds.min[1]..=bounds.max[1] {
                hash.update(
                    state
                        .world
                        .block_at(x, y, z)
                        .ok_or(Failure::Unloaded)?
                        .to_le_bytes(),
                );
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn validate_survey(
    state: &TickState,
    owner: &str,
    ledger: &Ledger,
    reference: &str,
    dimension: &str,
    bounds: ScriptWorldBounds,
    revision: u64,
) -> Result<(), Failure> {
    let survey = ledger.surveys.get(reference).ok_or(Failure::NotFound)?;
    if survey.owner != owner {
        return Err(Failure::Forbidden);
    }
    if survey.snapshot.dimension != dimension
        || survey.snapshot.world_revision != revision
        || survey.expires < state.plugins.revision()
    {
        return Err(Failure::StaleRevision);
    }
    if !contains(survey.snapshot.bounds, bounds) {
        return Err(Failure::InvalidRequest);
    }
    if fingerprint(world(state, dimension)?, survey.snapshot.bounds)? != survey.image {
        return Err(Failure::StaleRevision);
    }
    Ok(())
}
fn materials(counts: &BTreeMap<String, u64>) -> Vec<ScriptStructureMaterial> {
    counts
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(name, n)| ScriptStructureMaterial::new(name.clone(), *n))
        .collect()
}
fn resource_plan(stages: &[ScriptStructureStagePlan]) -> ScriptInventoryResourcePlan {
    ScriptInventoryResourcePlan::new(
        stages
            .iter()
            .map(|stage| {
                ScriptInventoryWorkPortion::new(
                    stage.work_units,
                    stage
                        .materials
                        .iter()
                        .map(|m| ScriptInventoryMaterial::new(m.resource.clone(), m.quantity))
                        .collect(),
                )
            })
            .collect(),
    )
}
fn planned_cells(
    state: &TickState,
    instance: &BlueprintInstance,
) -> Result<(Vec<ScriptStructureStagePlan>, Vec<Vec<Cell>>), Failure> {
    let blueprint = instance.blueprint();
    let mut plans = Vec::new();
    let mut batches = Vec::new();
    let mut resources = BTreeSet::new();
    for index in 0..blueprint.stages().len().max(1) {
        let (name, placed) = match blueprint.stages().get(index) {
            Some(stage) => (stage.id.clone(), instance.placed_stage_blocks(stage)),
            None => ("body".to_owned(), instance.placed_blocks()),
        };
        let mut cells = BTreeMap::new();
        for block in placed {
            let definition = state
                .registry
                .blocks
                .by_id(block.state)
                .ok_or(Failure::InvalidRequest)?;
            let name = definition.block.id.as_str();
            // Air authored to clear geometry costs no item. Every physical block
            // must have a real native item; no invented block-to-material fallback.
            let resource = if matches!(
                name,
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            ) {
                None
            } else {
                if state.registry.items.id_of(&definition.block.id).is_none() {
                    return Err(Failure::InvalidRequest);
                }
                Some(name.to_owned())
            };
            cells.insert(
                block.pos,
                Cell {
                    at: block.pos,
                    state: block.state.0,
                    resource,
                },
            );
        }
        if cells.is_empty() {
            return Err(Failure::InvalidRequest);
        }
        let cells: Vec<_> = cells.into_values().collect();
        let mut counts = BTreeMap::new();
        for cell in &cells {
            if let Some(resource) = &cell.resource {
                *counts.entry(resource.clone()).or_insert(0) += 1_u64;
                resources.insert(resource.clone());
            }
        }
        if resources.len() > MAX_STRUCTURE_RESOURCE_TYPES {
            return Err(Failure::Capacity);
        }
        plans.push(ScriptStructureStagePlan::new(
            name,
            cells.len() as u32,
            cells.len() as u64,
            materials(&counts),
        ));
        batches.push(cells);
    }
    Ok((plans, batches))
}
fn blueprint(state: &TickState, owner: &str, id: &str) -> Result<Arc<Blueprint>, Failure> {
    state
        .plugin_runtime
        .as_ref()
        .and_then(|runtime| runtime.blueprint(owner, id))
        .cloned()
        .ok_or(Failure::NotFound)
}
fn structure<'a>(ledger: &'a Ledger, owner: &str, id: &str) -> Result<&'a Structure, Failure> {
    let record = ledger.structures.get(id).ok_or(Failure::NotFound)?;
    if record.owner != owner {
        return Err(Failure::Forbidden);
    }
    Ok(record)
}
fn take_structure(ledger: &mut Ledger, owner: &str, id: &str) -> Result<Structure, Failure> {
    let record = ledger.structures.remove(id).ok_or(Failure::NotFound)?;
    if record.owner != owner {
        return Err(Failure::Forbidden);
    }
    Ok(record)
}

fn advance_cells<'a>(
    record: &'a Structure,
    stage: &str,
    reservation_ref: &str,
    expected_revision: u64,
    work_units: u64,
) -> Result<&'a [Cell], Failure> {
    if record.snapshot.revision != expected_revision {
        return Err(Failure::StaleRevision);
    }
    if !matches!(
        record.snapshot.state,
        ScriptStructureState::Prepared | ScriptStructureState::Running
    ) {
        return Err(Failure::Blocked);
    }
    let index = record.snapshot.current_stage_index as usize;
    let planned = record.snapshot.stages.get(index).ok_or(Failure::Blocked)?;
    if planned.stage != stage {
        return Err(Failure::InvalidRequest);
    }
    if record
        .snapshot
        .reservation_ref
        .as_ref()
        .is_some_and(|id| id != reservation_ref)
    {
        return Err(Failure::Blocked);
    }
    let offset = record.snapshot.completed_work_units as usize;
    let count = work_units
        .min(planned.work_units - record.snapshot.completed_work_units)
        .min(MAX_WORLD_COMMIT_PORTION as u64) as usize;
    if count == 0 {
        return Err(Failure::InvalidRequest);
    }
    Ok(&record.cells[index][offset..offset + count])
}

/// Frozen physical portion and material custody for one server-owned build.
#[derive(Debug, PartialEq)]
pub(crate) struct PreparedBuild {
    pub(crate) context: mc_script::precommit::HookContext,
    reservation_revision: u64,
    reservation: ScriptInventoryReservationSnapshot,
    inventory_fence: ScriptInventoryFence,
}

/// Read-only preview of the only structure operation that writes blocks.
/// Repeating this preview at commit fences the exact edits and material custody.
/// A changed footprint still takes the existing pause-only path, without a hook.
pub(crate) fn prepare_build(
    state: &TickState,
    owner: &str,
    request: &ScriptOperationRequest,
) -> Result<Option<PreparedBuild>, Failure> {
    let ScriptOperation::WorldStructure { operation } = request.operation() else {
        return Ok(None);
    };
    prepare_structure_build(state, owner, operation)
}

pub(crate) fn prepare_structure_build(
    state: &TickState,
    owner: &str,
    operation: &ScriptWorldStructureOperation,
) -> Result<Option<PreparedBuild>, Failure> {
    use mc_script::precommit::{BuildContext, BuildEdit, HookActor, HookContext};
    let ScriptWorldStructureOperation::AdvanceStructure {
        structure_id,
        stage,
        reservation_ref,
        expected_revision,
        work_units,
        ..
    } = operation
    else {
        return Ok(None);
    };
    operation.validate().map_err(|_| Failure::InvalidRequest)?;
    let ledger = ledger(state)?;
    let record = structure(&ledger, owner, structure_id)?;
    let portion = advance_cells(
        record,
        stage,
        reservation_ref,
        *expected_revision,
        *work_units,
    )?;
    if blueprint(state, owner, &record.snapshot.blueprint_id)?.content_hash()
        != record.blueprint_hash
    {
        return Err(Failure::StaleRevision);
    }
    let bounds = bounds_of_structure(record)?;
    if !crate::plugin_player::allows_plugin_bounds(
        state,
        owner,
        &record.snapshot.dimension,
        bounds.min,
        bounds.max,
    ) {
        return Err(Failure::Forbidden);
    }
    let target = world(state, &record.snapshot.dimension)?;
    if fingerprint(target, bounds)? != record.image {
        return Ok(None);
    }
    let mut edits = Vec::with_capacity(portion.len());
    for cell in portion {
        let previous = target
            .world
            .block_at(cell.at[0], cell.at[1], cell.at[2])
            .ok_or(Failure::Unloaded)?;
        if target
            .block_entities
            .get(&(cell.at[0], cell.at[1], cell.at[2]))
            .is_some_and(|block| block.slots.iter().any(|stack| !stack.is_empty()))
        {
            return Err(Failure::Blocked);
        }
        edits.push(BuildEdit::new(
            cell.at[0], cell.at[1], cell.at[2], previous, cell.state,
        ));
    }
    let reservation = inventories::reservation(state, owner, reservation_ref)?;
    if reservation.snapshot.resource_plan_hash != record.snapshot.resource_plan_hash
        || reservation.snapshot.released
        || reservation
            .snapshot
            .bound_to
            .as_deref()
            .is_some_and(|id| id != structure_id)
    {
        return Err(Failure::Blocked);
    }
    let inventory_fence = inventories::load(state, owner, &reservation.snapshot.endpoint)?.fence;
    let context = HookContext::Build(
        BuildContext::try_new(
            HookActor::Plugin(owner.to_owned()),
            record.snapshot.dimension.clone(),
            edits,
        )
        .map_err(|_| Failure::InvalidRequest)?,
    );
    Ok(Some(PreparedBuild {
        context,
        reservation_revision: reservation.revision,
        reservation: reservation.snapshot,
        inventory_fence,
    }))
}
fn survey(
    state: &mut TickState,
    owner: &str,
    dimension: &str,
    bounds: ScriptWorldBounds,
) -> Result<(u64, ScriptWorldStructureResult), Failure> {
    let mut ledger = ledger(state)?;
    let target = world(state, dimension)?;
    let image = fingerprint(target, bounds)?;
    let revision = next(ledger.revision)?;
    let mut usable = 0_u32;
    let mut water = 0_u32;
    let mut biomes = BTreeSet::new();
    let mut resources = BTreeSet::new();
    for z in bounds.min[2]..=bounds.max[2] {
        for x in bounds.min[0]..=bounds.max[0] {
            let mut surface = None;
            for y in (bounds.min[1]..=bounds.max[1]).rev() {
                let id = target.world.block_at(x, y, z).ok_or(Failure::Unloaded)?;
                let definition = target
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(id))
                    .ok_or(Failure::InvalidRequest)?;
                let name = definition.block.id.as_str();
                if name.contains("_ore") {
                    resources.insert(name.to_owned());
                }
                if name.ends_with("_log") {
                    resources.insert("minecraft:wood".to_owned());
                }
                if surface.is_none()
                    && !matches!(
                        name,
                        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                    )
                {
                    surface = Some((y, id, name));
                }
            }
            if let Some((y, id, name)) = surface {
                if name == "minecraft:water" {
                    water += 1;
                } else if !target.registry.collision_boxes[id as usize].is_empty() {
                    usable += 1;
                }
                if let Some(biome) = target.world.biome_at(x, y, z) {
                    biomes.insert(biome.as_str().to_owned());
                }
            }
        }
    }
    if biomes.len() > MAX_SURVEY_TAGS || resources.len() > MAX_SURVEY_TAGS {
        return Err(Failure::Capacity);
    }
    let existing = ledger
        .structures
        .values()
        .filter(|s| {
            s.snapshot.state != ScriptStructureState::Cancelled
                && s.snapshot.dimension == dimension
                && bounds_of_structure(s).is_ok_and(|b| overlaps(b, bounds))
        })
        .count() as u32;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let snapshot = ScriptSurveySnapshot::new(
        dimension.to_owned(),
        bounds,
        revision,
        target.world.revision_now(),
        ScriptChunkAvailability::Loaded,
        token.clone(),
        usable,
        water,
        existing,
        biomes.into_iter().collect(),
        resources.into_iter().collect(),
    );
    snapshot.validate().map_err(|_| Failure::InvalidRequest)?;
    ledger.revision = revision;
    let watermark = state.plugins.revision();
    ledger.surveys.retain(|_, s| s.expires >= watermark);
    ledger.surveys.insert(
        token,
        Survey {
            owner: owner.to_owned(),
            snapshot: snapshot.clone(),
            image,
            expires: watermark.saturating_add(64),
        },
    );
    save(state, &ledger);
    Ok((
        revision,
        ScriptWorldStructureResult::Survey {
            survey: Box::new(snapshot),
        },
    ))
}
fn bounds_of_structure(record: &Structure) -> Result<ScriptWorldBounds, Failure> {
    bounds(record.snapshot.origin, record.snapshot.reserved_footprint)
}

pub(crate) fn execute(
    state: &mut TickState,
    plugin_id: &str,
    request: &ScriptOperationRequest,
) -> ScriptOperationOutcome {
    let ScriptOperation::WorldStructure { operation } = request.operation() else {
        return ScriptOperationOutcome::rejected(Failure::InvalidRequest);
    };
    match execute_operation(state, plugin_id, operation) {
        Ok((revision, result)) => ScriptOperationOutcome::committed(
            revision,
            ScriptOperationPayload::WorldStructure {
                result: Box::new(result),
            },
        )
        .expect("validated native structure outcome"),
        Err(failure) => ScriptOperationOutcome::rejected(failure),
    }
}
fn execute_operation(
    state: &mut TickState,
    owner: &str,
    operation: &ScriptWorldStructureOperation,
) -> Result<(u64, ScriptWorldStructureResult), Failure> {
    operation.validate().map_err(|_| Failure::InvalidRequest)?;
    match operation {
        ScriptWorldStructureOperation::Survey { dimension, bounds } => {
            survey(state, owner, dimension, *bounds)
        }
        ScriptWorldStructureOperation::QueryBlocks {
            dimension,
            positions,
        } => {
            let target = world(state, dimension)?;
            let blocks = positions
                .iter()
                .map(|at| worksites::facts(target, *at))
                .collect::<Result<_, Failure>>()?;
            Ok((
                target.world.revision_now(),
                ScriptWorldStructureResult::Blocks {
                    query: Box::new(ScriptWorldBlockQuery::new(
                        target.world.revision_now(),
                        blocks,
                    )),
                },
            ))
        }
        ScriptWorldStructureOperation::ListFeatures { cursor, limit } => {
            features::list(state, cursor.as_deref(), *limit)
        }
        ScriptWorldStructureOperation::QueryFeature { feature_id } => {
            features::query(state, feature_id)
        }
        ScriptWorldStructureOperation::PrepareStructure {
            dimension,
            blueprint_id,
            anchor,
            rotation,
            survey_token,
            expected_world_revision,
            expected_blueprint_hash,
            ..
        } => {
            let mut ledger = ledger(state)?;
            let blueprint = blueprint(state, owner, blueprint_id)?;
            if expected_blueprint_hash
                .as_ref()
                .is_some_and(|hash| hash != blueprint.content_hash())
            {
                return Err(Failure::StaleRevision);
            }
            let turn = QuarterTurn::from_degrees(*rotation).ok_or(Failure::InvalidRequest)?;
            let size = blueprint.size();
            let extent = if matches!(turn, QuarterTurn::Cw90 | QuarterTurn::Cw270) {
                [size[2], size[1], size[0]]
            } else {
                size
            };
            let bounds = bounds(*anchor, extent)?;
            if !crate::plugin_player::allows_plugin_bounds(
                state, owner, dimension, bounds.min, bounds.max,
            ) {
                return Err(Failure::Forbidden);
            }
            validate_survey(
                state,
                owner,
                &ledger,
                survey_token,
                dimension,
                bounds,
                *expected_world_revision,
            )?;
            let target = world(state, dimension)?;
            let geometry = target.dimension.geometry();
            if bounds.min[1] < geometry.min_y() || bounds.max[1] >= geometry.max_y() {
                return Err(Failure::InvalidRequest);
            }
            if ledger
                .structures
                .values()
                .filter(|s| {
                    s.owner == owner
                        && matches!(
                            s.snapshot.state,
                            ScriptStructureState::Prepared
                                | ScriptStructureState::Running
                                | ScriptStructureState::Paused
                        )
                })
                .count()
                >= MAX_STRUCTURE_ACTIVE_PER_PLUGIN
            {
                return Err(Failure::Capacity);
            }
            if ledger.structures.values().any(|s| {
                s.snapshot.dimension == *dimension
                    && s.snapshot.state != ScriptStructureState::Cancelled
                    && bounds_of_structure(s).is_ok_and(|b| overlaps(b, bounds))
            }) {
                return Err(Failure::Blocked);
            }
            for z in bounds.min[2]..=bounds.max[2] {
                for x in bounds.min[0]..=bounds.max[0] {
                    for y in (anchor[1] + 1)..=bounds.max[1] {
                        let id = target.world.block_at(x, y, z).ok_or(Failure::Unloaded)?;
                        if !target.registry.collision_boxes[id as usize].is_empty() {
                            return Err(Failure::Blocked);
                        }
                    }
                }
            }
            let instance = BlueprintInstance::new(blueprint.clone(), turn, *anchor);
            let (stages, cells) = planned_cells(state, &instance)?;
            let plan = resource_plan(&stages);
            plan.validate().map_err(|_| Failure::InvalidRequest)?;
            let interaction_points = worksites::planned_points(state, &instance)?;
            let revision = next(ledger.revision)?;
            let id = uuid::Uuid::new_v4().simple().to_string();
            let remaining = stages.iter().flat_map(|s| s.materials.iter()).fold(
                BTreeMap::new(),
                |mut counts, m| {
                    *counts.entry(m.resource.clone()).or_insert(0) += m.quantity;
                    counts
                },
            );
            let snapshot = ScriptStructureSnapshot::new(
                id.clone(),
                blueprint_id.clone(),
                dimension.clone(),
                ScriptStructureState::Prepared,
                revision,
                *anchor,
                *rotation,
                extent,
                stages,
                0,
                0,
                inventories::resource_plan_hash(&plan),
                None,
                0,
                Vec::new(),
                materials(&remaining),
                None,
                interaction_points,
            );
            snapshot.validate().map_err(|_| Failure::InvalidRequest)?;
            let image = fingerprint(target, bounds)?;
            ledger.revision = revision;
            ledger.structures.insert(
                id,
                Structure {
                    owner: owner.to_owned(),
                    snapshot: snapshot.clone(),
                    blueprint_hash: blueprint.content_hash().to_owned(),
                    image,
                    cells,
                    receipts: Vec::new(),
                },
            );
            save(state, &ledger);
            Ok((
                revision,
                ScriptWorldStructureResult::Structure {
                    structure: Box::new(snapshot),
                },
            ))
        }
        ScriptWorldStructureOperation::Status { structure_id } => {
            let ledger = ledger(state)?;
            let record = structure(&ledger, owner, structure_id)?;
            Ok((
                record.snapshot.revision,
                ScriptWorldStructureResult::Structure {
                    structure: Box::new(record.snapshot.clone()),
                },
            ))
        }
        ScriptWorldStructureOperation::AdvanceStructure {
            structure_id,
            stage,
            reservation_ref,
            expected_revision,
            work_units,
            ..
        } => {
            let mut ledger = ledger(state)?;
            let mut record = take_structure(&mut ledger, owner, structure_id)?;
            let portion = advance_cells(
                &record,
                stage,
                reservation_ref,
                *expected_revision,
                *work_units,
            )?;
            let index = record.snapshot.current_stage_index as usize;
            let blueprint = blueprint(state, owner, &record.snapshot.blueprint_id)?;
            if blueprint.content_hash() != record.blueprint_hash {
                return Err(Failure::StaleRevision);
            }
            let bounds = bounds_of_structure(&record)?;
            if !crate::plugin_player::allows_plugin_bounds(
                state,
                owner,
                &record.snapshot.dimension,
                bounds.min,
                bounds.max,
            ) {
                return Err(Failure::Forbidden);
            }
            let target = world(state, &record.snapshot.dimension)?;
            if fingerprint(target, bounds)? != record.image {
                record.snapshot.state = ScriptStructureState::Paused;
                record.snapshot.pause_reason = Some("footprint_changed".to_owned());
                ledger.revision = next(ledger.revision)?;
                record.snapshot.revision = ledger.revision;
                let result = record.snapshot.clone();
                ledger.structures.insert(structure_id.clone(), record);
                save(state, &ledger);
                return Ok((
                    result.revision,
                    ScriptWorldStructureResult::Structure {
                        structure: Box::new(result),
                    },
                ));
            }
            let count = portion.len();
            let mut consumed = BTreeMap::new();
            for cell in portion {
                target
                    .world
                    .block_at(cell.at[0], cell.at[1], cell.at[2])
                    .ok_or(Failure::Unloaded)?;
                if target
                    .block_entities
                    .get(&(cell.at[0], cell.at[1], cell.at[2]))
                    .is_some_and(|s| s.slots.iter().any(|stack| !stack.is_empty()))
                {
                    return Err(Failure::Blocked);
                }
                if let Some(resource) = &cell.resource {
                    *consumed.entry(resource.clone()).or_insert(0) += 1_u64;
                }
            }
            let reservation = inventories::reservation(state, owner, reservation_ref)?;
            if reservation.snapshot.resource_plan_hash != record.snapshot.resource_plan_hash {
                return Err(Failure::Blocked);
            }
            let revision = next(ledger.revision)?;
            let sequence = next(record.snapshot.watermark)?;
            let receipt = ScriptStructureReceipt::new(
                structure_id.clone(),
                stage.clone(),
                sequence,
                count as u32,
                count as u64,
                materials(&consumed),
                revision,
            );
            receipt.validate().map_err(|_| Failure::InvalidRequest)?;
            // Debit and placement are preflighted on the same single tick owner.
            // Their queues and the durable receipt join the root tick WAL image.
            inventories::commit_reserved_debit(state, owner, reservation, &consumed, structure_id)?;
            let target = world_mut(state, &record.snapshot.dimension).expect("preflight dimension");
            for cell in portion {
                let pos = BlockPos {
                    x: cell.at[0],
                    y: cell.at[1],
                    z: cell.at[2],
                };
                crate::block_entity_defaults::accepted_placement(target, pos, cell.state);
                target
                    .world
                    .set_block(cell.at[0], cell.at[1], cell.at[2], cell.state);
                crate::block_entity_defaults::after_blocks(target, &[(pos, cell.state)]);
                target.plugin_block_writes.push((pos, cell.state));
            }
            record.image = fingerprint(target, bounds).expect("preflight native footprint");
            record.snapshot.state = ScriptStructureState::Running;
            record.snapshot.revision = revision;
            record.snapshot.watermark = sequence;
            record.snapshot.reservation_ref = Some(reservation_ref.clone());
            record.snapshot.completed_work_units += count as u64;
            let mut totals: BTreeMap<_, _> = record
                .snapshot
                .consumed
                .iter()
                .map(|m| (m.resource.clone(), m.quantity))
                .collect();
            for (name, n) in consumed {
                *totals.entry(name).or_insert(0) += n;
            }
            record.snapshot.consumed = materials(&totals);
            let current = inventories::reservation(state, owner, reservation_ref)?;
            record.snapshot.remaining = current
                .snapshot
                .quantities
                .iter()
                .filter(|q| q.remaining > 0)
                .map(|q| ScriptStructureMaterial::new(q.resource_id.clone(), q.remaining))
                .collect();
            if record.snapshot.completed_work_units == record.snapshot.stages[index].work_units {
                record.snapshot.current_stage_index += 1;
                record.snapshot.completed_work_units = 0;
                if record.snapshot.current_stage_index as usize == record.snapshot.stages.len() {
                    record.snapshot.state = ScriptStructureState::Committed;
                }
            }
            record.receipts.push(receipt.clone());
            ledger.revision = revision;
            ledger.structures.insert(structure_id.clone(), record);
            save(state, &ledger);
            Ok((
                revision,
                ScriptWorldStructureResult::Receipt {
                    receipt: Box::new(receipt),
                },
            ))
        }
        ScriptWorldStructureOperation::PauseStructure {
            structure_id,
            expected_revision,
            ..
        }
        | ScriptWorldStructureOperation::ResumeStructure {
            structure_id,
            expected_revision,
            ..
        }
        | ScriptWorldStructureOperation::CancelStructure {
            structure_id,
            expected_revision,
            ..
        } => {
            let mut ledger = ledger(state)?;
            let mut record = take_structure(&mut ledger, owner, structure_id)?;
            if record.snapshot.revision != *expected_revision {
                return Err(Failure::StaleRevision);
            }
            if matches!(
                record.snapshot.state,
                ScriptStructureState::Committed | ScriptStructureState::Cancelled
            ) {
                return Err(Failure::Blocked);
            }
            let revision = next(ledger.revision)?;
            match operation {
                ScriptWorldStructureOperation::PauseStructure { .. } => {
                    record.snapshot.state = ScriptStructureState::Paused;
                    record.snapshot.pause_reason = Some("requested".to_owned());
                }
                ScriptWorldStructureOperation::ResumeStructure { .. } => {
                    if record.snapshot.state != ScriptStructureState::Paused {
                        return Err(Failure::Blocked);
                    }
                    if fingerprint(
                        world(state, &record.snapshot.dimension)?,
                        bounds_of_structure(&record)?,
                    )? != record.image
                    {
                        return Err(Failure::Blocked);
                    }
                    record.snapshot.state = ScriptStructureState::Running;
                    record.snapshot.pause_reason = None;
                }
                _ => {
                    if let Some(reference) = &record.snapshot.reservation_ref {
                        let mut reservation = inventories::reservation(state, owner, reference)?;
                        for q in &mut reservation.snapshot.quantities {
                            q.returned += q.remaining;
                            q.remaining = 0;
                        }
                        reservation.snapshot.released = true;
                        reservation.snapshot.bound_to = None;
                        reservation.revision = next(reservation.revision)?;
                        inventories::save_reservation(state, reservation)?;
                    }
                    record.snapshot.state = ScriptStructureState::Cancelled;
                    record.snapshot.pause_reason = None;
                    record.snapshot.remaining.clear();
                }
            }
            ledger.revision = revision;
            record.snapshot.revision = ledger.revision;
            let result = record.snapshot.clone();
            ledger.structures.insert(structure_id.clone(), record);
            save(state, &ledger);
            Ok((
                result.revision,
                ScriptWorldStructureResult::Structure {
                    structure: Box::new(result),
                },
            ))
        }
        ScriptWorldStructureOperation::RegisterWorksite { request, .. } => {
            let mut ledger = ledger(state)?;
            let survey = ledger
                .surveys
                .get(&request.survey_token)
                .ok_or(Failure::NotFound)?;
            let dimension = survey.snapshot.dimension.clone();
            validate_survey(
                state,
                owner,
                &ledger,
                &request.survey_token,
                &dimension,
                request.footprint,
                request.expected_world_revision,
            )?;
            let blocked = worksites::check(state, &dimension, &request.interaction_points)?;
            if !blocked.is_empty() {
                return Err(Failure::Blocked);
            }
            let target = world(state, &dimension)?;
            ledger.revision = next(ledger.revision)?;
            let id = uuid::Uuid::new_v4().simple().to_string();
            let snapshot = ScriptWorksiteSnapshot::new(
                id.clone(),
                request.footprint,
                request.interaction_points.clone(),
                target.world.revision_now(),
                ledger.revision,
            );
            snapshot.validate().map_err(|_| Failure::InvalidRequest)?;
            ledger.worksites.insert(
                id,
                Worksite {
                    owner: owner.to_owned(),
                    dimension,
                    snapshot: snapshot.clone(),
                },
            );
            save(state, &ledger);
            Ok((
                snapshot.revision,
                ScriptWorldStructureResult::Worksite {
                    worksite: Box::new(snapshot),
                },
            ))
        }
        ScriptWorldStructureOperation::WorksiteStatus { worksite_id }
        | ScriptWorldStructureOperation::CheckWorksite { worksite_id } => {
            let ledger = ledger(state)?;
            let worksite = ledger.worksites.get(worksite_id).ok_or(Failure::NotFound)?;
            if worksite.owner != owner {
                return Err(Failure::Forbidden);
            }
            if matches!(
                operation,
                ScriptWorldStructureOperation::WorksiteStatus { .. }
            ) {
                return Ok((
                    worksite.snapshot.revision,
                    ScriptWorldStructureResult::Worksite {
                        worksite: Box::new(worksite.snapshot.clone()),
                    },
                ));
            }
            let blocked = worksites::check(
                state,
                &worksite.dimension,
                &worksite.snapshot.interaction_points,
            )?;
            let target = world(state, &worksite.dimension)?;
            let check = ScriptWorksiteCheck::new(
                worksite_id.clone(),
                target.world.revision_now(),
                blocked.is_empty(),
                blocked,
            );
            Ok((
                target.world.revision_now(),
                ScriptWorldStructureResult::WorksiteCheck {
                    check: Box::new(check),
                },
            ))
        }
        ScriptWorldStructureOperation::CheckStructure { structure_id } => {
            let ledger = ledger(state)?;
            let record = structure(&ledger, owner, structure_id)?;
            let blocked = worksites::check(
                state,
                &record.snapshot.dimension,
                &record.snapshot.interaction_points,
            )?;
            let target = world(state, &record.snapshot.dimension)?;
            let check = ScriptStructureCheck::new(
                structure_id.clone(),
                owner.to_owned(),
                target.world.revision_now(),
                blocked.is_empty(),
                blocked,
            );
            Ok((
                target.world.revision_now(),
                ScriptWorldStructureResult::StructureCheck {
                    check: Box::new(check),
                },
            ))
        }
        _ => Err(Failure::InvalidRequest),
    }
}

pub(crate) fn advance_npc_work(
    state: &mut TickState,
    owner: &str,
    handle: &str,
    structure_id: &str,
    stage: &str,
    expected_revision: u64,
    assignment_revision: u64,
) -> Result<Option<(u64, u64, Vec<ScriptItemChange>)>, Failure> {
    let slot = crate::plugin_npcs::owned_slot(state, owner, handle)?;
    let entity_uuid = state.entities.row(slot).uuid;
    let ledger = ledger(state)?;
    let record = structure(&ledger, owner, structure_id)?;
    let reference = if let Some(reference) = &record.snapshot.reservation_ref {
        reference.clone()
    } else {
        inventories::matching_reservation(
            state,
            owner,
            &record.snapshot.resource_plan_hash,
            structure_id,
        )?
    };
    let operation = ScriptWorldStructureOperation::AdvanceStructure {
        operation_id: format!("npc-build-{expected_revision}"),
        structure_id: structure_id.to_owned(),
        stage: stage.to_owned(),
        reservation_ref: reference,
        expected_revision,
        work_units: 1,
    };
    if !crate::plugin::admit_npc_structure_build(
        state,
        owner,
        handle,
        assignment_revision,
        entity_uuid,
        &operation,
    )? {
        return Ok(None);
    }
    let (revision, result) = execute_operation(state, owner, &operation)?;
    match result {
        ScriptWorldStructureResult::Receipt { receipt } => Ok(Some((
            receipt.work_units,
            revision,
            receipt
                .consumed
                .iter()
                .map(|material| {
                    ScriptItemChange::new(material.resource.clone(), -(material.quantity as i64))
                })
                .collect(),
        ))),
        _ => Err(Failure::Blocked),
    }
}
pub(crate) fn validate(state: &TickState) -> Result<(), crate::config::CoreError> {
    let malformed =
        || crate::config::CoreError::Recovery("invalid native plugin structure ledger".into());
    let ledger = ledger(state).map_err(|_| malformed())?;
    if ledger.revision > MAX_SCRIPT_WORLD_TIME {
        return Err(malformed());
    }
    for (token, survey) in &ledger.surveys {
        if token != &survey.snapshot.survey_token
            || survey.owner.is_empty()
            || !valid_hash(&survey.image)
            || survey.snapshot.revision > ledger.revision
        {
            return Err(malformed());
        }
        survey.snapshot.validate().map_err(|_| malformed())?;
    }
    for (id, record) in &ledger.structures {
        if id != &record.snapshot.structure_id
            || record.owner.is_empty()
            || !valid_hash(&record.image)
            || !valid_hash(&record.blueprint_hash)
            || record.snapshot.revision > ledger.revision
            || record.cells.len() != record.snapshot.stages.len()
        {
            return Err(malformed());
        }
        record.snapshot.validate().map_err(|_| malformed())?;
        let bounds = bounds_of_structure(record).map_err(|_| malformed())?;
        for (stage, cells) in record.snapshot.stages.iter().zip(&record.cells) {
            if cells.len() as u64 != stage.work_units || cells.len() as u32 != stage.block_count {
                return Err(malformed());
            }
            let mut positions = BTreeSet::new();
            let mut counts = BTreeMap::new();
            for cell in cells {
                if !(0..3).all(|i| (bounds.min[i]..=bounds.max[i]).contains(&cell.at[i]))
                    || !positions.insert(cell.at)
                    || state
                        .registry
                        .blocks
                        .by_id(mc_world::BlockStateId(cell.state))
                        .is_none()
                {
                    return Err(malformed());
                }
                if let Some(resource) = &cell.resource {
                    let definition = state
                        .registry
                        .blocks
                        .by_id(mc_world::BlockStateId(cell.state))
                        .expect("validated state");
                    if definition.block.id.as_str() != resource
                        || state.registry.items.id_of(&definition.block.id).is_none()
                    {
                        return Err(malformed());
                    }
                    *counts.entry(resource.clone()).or_insert(0) += 1;
                }
            }
            if materials(&counts) != stage.materials {
                return Err(malformed());
            }
        }
        let plan = resource_plan(&record.snapshot.stages);
        plan.validate().map_err(|_| malformed())?;
        if inventories::resource_plan_hash(&plan) != record.snapshot.resource_plan_hash {
            return Err(malformed());
        }
        if record.receipts.len() as u64 != record.snapshot.watermark {
            return Err(malformed());
        }
        for (index, receipt) in record.receipts.iter().enumerate() {
            receipt.validate().map_err(|_| malformed())?;
            if receipt.structure_id != *id
                || receipt.sequence != index as u64 + 1
                || receipt.revision > record.snapshot.revision
            {
                return Err(malformed());
            }
        }
        if let Some(reference) = &record.snapshot.reservation_ref {
            match inventories::reservation(state, &record.owner, reference) {
                Ok(reservation)
                    if reservation.snapshot.resource_plan_hash
                        == record.snapshot.resource_plan_hash => {}
                Err(Failure::NotFound)
                    if record.snapshot.state == ScriptStructureState::Cancelled
                        && !inventories::retains_reservation(state, reference) => {}
                _ => return Err(malformed()),
            }
        }
    }
    for (id, worksite) in &ledger.worksites {
        if id != &worksite.snapshot.worksite_id
            || worksite.owner.is_empty()
            || worksite.snapshot.revision > ledger.revision
            || crate::dimensions::Dimension::parse(&worksite.dimension).is_none()
        {
            return Err(malformed());
        }
        worksite.snapshot.validate().map_err(|_| malformed())?;
    }
    Ok(())
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
#[path = "plugin_structures_tests.rs"]
mod tests;
