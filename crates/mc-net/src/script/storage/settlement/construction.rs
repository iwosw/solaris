//! Staged construction lifecycle: prepare, advance, pause, resume and cancel.
//!
//! Split out of `settlement.rs` by CP-REFACTOR-040. The ledger authority, the
//! shared atomic `apply`, the world adapters and `commit_settlement` stay in the
//! parent module, which declares this child and dispatches into it.

use super::*;

impl crate::script::storage::InventoryRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_structure(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        site_id: &str,
        blueprint_id: &str,
        anchor: [i32; 3],
        rotation: u16,
        survey_token: &str,
        expected_site_revision: u64,
        expected_blueprint_hash: Option<&str>,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let Some(blueprint) = runtime.catalog().get(blueprint_id).cloned() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if expected_blueprint_hash.is_some_and(|hash| hash != blueprint.content_hash()) {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let Some(turn) = QuarterTurn::from_degrees(rotation) else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        let reserved_footprint = rotated_extent(blueprint.size(), turn);
        if reserved_footprint
            .iter()
            .any(|axis| *axis <= 0 || *axis > MAX_BLUEPRINT_FOOTPRINT_AXIS)
        {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let bounds = bounds_of(anchor, reserved_footprint);
        if bounds.min[1] < mc_world::MIN_Y || bounds.max[1] >= mc_world::MAX_Y {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        // Contract rule: a structure is only placed where nothing rises above
        // its base row. Terrain or vegetation above that row would be
        // overwritten (forbidden) or wall the interior in, so the placement is
        // refused instead of built on top of the terrain. Terrain level with
        // the base row is the ground the structure stands on.
        match world.max_opaque_y(bounds) {
            Ok(None) => return Ok(rejected(ScriptOperationFailure::Unloaded)),
            Ok(Some(surface)) if surface > anchor[1] => {
                return Ok(rejected(ScriptOperationFailure::Blocked));
            }
            Ok(Some(_)) => {}
            Err(failure) => return Ok(rejected(failure)),
        }
        if blueprint.street_connections().is_empty() {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if storage.settlements().active_structures(plugin_id).len()
            >= MAX_STRUCTURE_ACTIVE_PER_PLUGIN
        {
            return Ok(rejected(ScriptOperationFailure::Capacity));
        }
        let structure_site_id = match generated_village(runtime, site_id) {
            Ok(village) => {
                let bounds = match village_bounds(&village) {
                    Ok(bounds) => bounds,
                    Err(failure) => return Ok(rejected(failure)),
                };
                if !(bounds.min[0]..=bounds.max[0]).contains(&anchor[0])
                    || !(bounds.min[2]..=bounds.max[2]).contains(&anchor[2])
                {
                    return Ok(rejected(ScriptOperationFailure::NotFound));
                }
                site_id.to_owned()
            }
            Err(ScriptOperationFailure::NotFound) => {
                let Some(cell) = runtime.selector().cell_from_site_id(site_id) else {
                    return Ok(rejected(ScriptOperationFailure::NotFound));
                };
                if [
                    anchor[0].div_euclid(SITE_CELL_BLOCKS),
                    anchor[2].div_euclid(SITE_CELL_BLOCKS),
                ] != cell
                {
                    return Ok(rejected(ScriptOperationFailure::NotFound));
                }
                site_id.to_owned()
            }
            Err(failure) => return Ok(rejected(failure)),
        };
        let site_revision = storage
            .settlements()
            .site(&structure_site_id)
            .map_or(0, |site| site.revision);
        if expected_site_revision != site_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        if world.claims_overlap(plugin_id, bounds)
            || storage
                .settlements()
                .structures_of(plugin_id)
                .any(|structure| {
                    structure.state != ScriptStructureState::Cancelled
                        && bounds_overlap(&structure.bounds(), &bounds)
                })
        {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let Some(token) = storage.settlements().survey(survey_token) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if token.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if token.expires_transaction < storage.revision
            || world.footprint_changed_since(token.bounds, token.world_revision)
        {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let instance = BlueprintInstance::new(Arc::clone(&blueprint), turn, anchor);
        let stages = match stage_plans(&instance) {
            Ok(stages) => stages,
            Err(failure) => return Ok(rejected(failure)),
        };
        let plan = structure_resource_plan(&stages);
        if plan.validate().is_err() {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let transaction_id = storage
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        let structure = DurableStructure {
            structure_id: mint_structure_id(plugin_id, transaction_id, blueprint_id, anchor),
            plugin_id: plugin_id.to_owned(),
            site_id: structure_site_id,
            blueprint_id: blueprint_id.to_owned(),
            blueprint_hash: blueprint.content_hash().to_owned(),
            origin: anchor,
            rotation,
            reserved_footprint,
            state: ScriptStructureState::Prepared,
            stages,
            stage_index: 0,
            watermark: 0,
            built_blocks: 0,
            reservation_ref: None,
            resource_plan_hash: resource_plan_hash(&plan),
            consumed: BTreeMap::new(),
            prepare_revision: world.observe_footprint(bounds),
            pause_reason: None,
            revision: 0,
        };
        let snapshot = structure_snapshot(storage, &structure);
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Structure {
                structure: Box::new(snapshot),
            }),
            vec![DurableSettlementChange::Structure {
                structure: Box::new(structure),
            }],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn advance_structure(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        structure_id: &str,
        stage: &str,
        reservation_ref: &str,
        expected_revision: u64,
        work_units: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let Some(record) = storage.settlements().structure(structure_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if !matches!(
            record.state,
            ScriptStructureState::Prepared | ScriptStructureState::Running
        ) {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if record.revision != expected_revision {
            return Ok(ScriptOperationOutcome::rejected_with_payload(
                ScriptOperationFailure::StaleRevision,
                settlement_payload(ScriptSettlementResult::Structure {
                    structure: Box::new(structure_snapshot(storage, record)),
                }),
            ));
        }
        let Some(planned_stage) = record.stage() else {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        };
        if planned_stage.stage != stage {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        if let Some(bound) = record.reservation_ref()
            && bound != reservation_ref
        {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let Some((_, reservation)) = storage.settlement_reservation(plugin_id, reservation_ref)
        else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if reservation.released || reservation.resource_plan_hash != record.resource_plan_hash {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if reservation
            .bound_to
            .as_deref()
            .is_some_and(|bound| bound != structure_id)
        {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let mut record = record.clone();
        if world.footprint_changed_since(record.bounds(), record.prepare_revision) {
            record.state = ScriptStructureState::Paused;
            record.pause_reason = Some(PAUSE_SITE_CHANGED.to_owned());
            let snapshot = structure_snapshot(storage, &record);
            return commit_settlement(
                storage,
                plugin_id,
                request,
                settlement_payload(ScriptSettlementResult::Structure {
                    structure: Box::new(snapshot),
                }),
                vec![DurableSettlementChange::Structure {
                    structure: Box::new(record),
                }],
            );
        }
        let Some(blueprint) = runtime.catalog().get(&record.blueprint_id).cloned() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if blueprint.content_hash() != record.blueprint_hash {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let Some(index) = record.stages.iter().position(|plan| plan.stage == stage) else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        if index != record.stage_index {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let Some(turn) = QuarterTurn::from_degrees(record.rotation) else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        let instance = BlueprintInstance::new(blueprint, turn, record.origin);
        let cells = match stage_cells(&instance, index) {
            Ok(cells) => cells,
            Err(failure) => return Ok(rejected(failure)),
        };
        if cells.len() as u64 != planned_stage.work_units {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let stage_offset: u64 = record.stages[..index]
            .iter()
            .map(|plan| plan.work_units)
            .sum();
        let built_in_stage = record.built_blocks.saturating_sub(stage_offset);
        let remaining = (cells.len() as u64).saturating_sub(built_in_stage);
        // The request authorizes at most this much work; the portion is what the
        // stage can still commit, capped by the world commit bound.
        let portion_len = work_units
            .min(remaining)
            .min(MAX_WORLD_COMMIT_PORTION as u64);
        if portion_len == 0 {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let portion = &cells[built_in_stage as usize..(built_in_stage + portion_len) as usize];
        let mut consumed: BTreeMap<String, u64> = BTreeMap::new();
        for cell in portion {
            *consumed.entry(cell.block.clone()).or_insert(0) += 1;
        }
        for (resource, quantity) in &consumed {
            let available = reservation
                .quantities
                .iter()
                .find(|entry| &entry.resource_id == resource)
                .map_or(0, |entry| entry.remaining);
            if *quantity > available {
                return Ok(rejected(ScriptOperationFailure::InsufficientItems));
            }
        }
        let material_debit = match self.plan_structure_material_debit(
            storage,
            plugin_id,
            &reservation.endpoint,
            &consumed,
        ) {
            Ok(debit) => debit,
            Err(failure) => return Ok(rejected(failure)),
        };
        let placements = portion
            .iter()
            .map(|cell| StructureBlockPlacement {
                pos: cell.pos,
                state: cell.state,
            })
            .collect::<Vec<_>>();
        record.prepare_revision =
            match world.predict_structure_portion_revision(record.bounds(), &placements) {
                Ok(revision) => revision,
                Err(failure) => return Ok(rejected(failure)),
            };
        let receipt = ScriptStructureReceipt::new(
            structure_id.to_owned(),
            stage.to_owned(),
            record.watermark + 1,
            u32::try_from(portion_len).map_err(|_| PluginStorageMutationError::RevisionOverflow)?,
            portion_len,
            materials_of(&consumed),
            0,
        );
        record.watermark += 1;
        record.built_blocks += portion_len;
        record.reservation_ref = Some(reservation_ref.to_owned());
        record.pause_reason = None;
        for (resource, quantity) in consumed {
            *record.consumed.entry(resource).or_insert(0) += quantity;
        }
        if built_in_stage + portion_len == cells.len() as u64 {
            record.stage_index += 1;
        }
        record.state = if record.stage_index == record.stages.len() {
            ScriptStructureState::Committed
        } else {
            ScriptStructureState::Running
        };
        let prepared = match storage.prepare_settlement_operation_batch(
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Receipt {
                receipt: Box::new(receipt),
            }),
            vec![DurableSettlementChange::Structure {
                structure: Box::new(record),
            }],
        )? {
            ScriptStoragePrepareOutcome::Prepared(batch) => batch,
            ScriptStoragePrepareOutcome::Rejected => {
                return Ok(rejected(ScriptOperationFailure::Capacity));
            }
        };
        match self
            .commit_prepared_structure_portion(
                storage,
                plugin_id,
                structure_id,
                &placements,
                &material_debit,
                prepared,
            )
            .await?
        {
            crate::script::storage::world_inventory::PreparedStructurePortionCommit::Refused(
                failure,
            ) => Ok(rejected(failure)),
            crate::script::storage::world_inventory::PreparedStructurePortionCommit::Committed => {
                let operation_id = request
                    .operation_id()
                    .expect("structure advance has an operation id");
                Ok(storage
                    .operation_receipt(plugin_id, operation_id)
                    .expect("committed structure receipt remains installed")
                    .outcome
                    .clone())
            }
        }
    }

    pub(super) fn pause_structure(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        structure_id: &str,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = storage.settlements().structure(structure_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if !matches!(
            record.state,
            ScriptStructureState::Prepared | ScriptStructureState::Running
        ) {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let changed = self.settlement_world().is_some_and(|world| {
            world.footprint_changed_since(record.bounds(), record.prepare_revision)
        });
        let mut record = record.clone();
        record.state = ScriptStructureState::Paused;
        record.pause_reason = changed.then(|| PAUSE_SITE_CHANGED.to_owned());
        let snapshot = structure_snapshot(storage, &record);
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Structure {
                structure: Box::new(snapshot),
            }),
            vec![DurableSettlementChange::Structure {
                structure: Box::new(record),
            }],
        )
    }

    pub(super) fn resume_structure(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        structure_id: &str,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = storage.settlements().structure(structure_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if record.state != ScriptStructureState::Paused {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        // A pause is never authority to overwrite an altered or unloaded site.
        // The caller restores the exact accepted footprint, then retries from
        // this authoritative paused revision.
        if world.footprint_changed_since(record.bounds(), record.prepare_revision) {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if let Some(reservation_ref) = record.reservation_ref() {
            let Some((_, reservation)) = storage.settlement_reservation(plugin_id, reservation_ref)
            else {
                return Ok(rejected(ScriptOperationFailure::NotFound));
            };
            if reservation.released
                || reservation.resource_plan_hash != record.resource_plan_hash
                || reservation
                    .bound_to
                    .as_deref()
                    .is_some_and(|bound| bound != structure_id)
            {
                return Ok(rejected(ScriptOperationFailure::Blocked));
            }
        }
        let mut record = record.clone();
        record.state = if record.built_blocks == 0 {
            ScriptStructureState::Prepared
        } else {
            ScriptStructureState::Running
        };
        record.pause_reason = None;
        let snapshot = structure_snapshot(storage, &record);
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Structure {
                structure: Box::new(snapshot),
            }),
            vec![DurableSettlementChange::Structure {
                structure: Box::new(record),
            }],
        )
    }

    pub(super) fn cancel_structure(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        structure_id: &str,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = storage.settlements().structure(structure_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if !record.is_active() {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let mut record = record.clone();
        record.state = ScriptStructureState::Cancelled;
        let snapshot = structure_snapshot(storage, &record);
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Structure {
                structure: Box::new(snapshot),
            }),
            vec![DurableSettlementChange::Structure {
                structure: Box::new(record),
            }],
        )
    }
}

/// The one-work-portion-per-stage material plan of a structure. Its hash is the
/// fence a C1 reservation must carry before the structure may spend from it.
fn structure_resource_plan(stages: &[ScriptStructureStagePlan]) -> ScriptInventoryResourcePlan {
    ScriptInventoryResourcePlan::new(
        stages
            .iter()
            .map(|stage| {
                ScriptInventoryWorkPortion::new(
                    stage.work_units,
                    stage
                        .materials
                        .iter()
                        .map(|material| {
                            ScriptInventoryMaterial::new(
                                material.resource.clone(),
                                material.quantity,
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}
/// Build the stage plan of one placed blueprint.
///
/// A blueprint that authors no stages builds its body in one stage named
/// [`BODY_STAGE`]; every other blueprint plans one stage per authored stage.
fn stage_plans(
    instance: &BlueprintInstance,
) -> Result<Vec<ScriptStructureStagePlan>, ScriptOperationFailure> {
    let count = instance.blueprint().stages().len().max(1);
    if count > MAX_STRUCTURE_STAGES {
        return Err(ScriptOperationFailure::Blocked);
    }
    let mut stages = Vec::with_capacity(count);
    let mut resources: BTreeSet<String> = BTreeSet::new();
    for index in 0..count {
        let cells = stage_cells(instance, index)?;
        if cells.is_empty() {
            return Err(ScriptOperationFailure::Blocked);
        }
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for cell in &cells {
            *counts.entry(cell.block.clone()).or_insert(0) += 1;
        }
        if counts.len() > MAX_STRUCTURE_RESOURCE_TYPES {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        resources.extend(counts.keys().cloned());
        if resources.len() > MAX_STRUCTURE_RESOURCE_TYPES {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        let block_count =
            u32::try_from(cells.len()).map_err(|_| ScriptOperationFailure::InvalidRequest)?;
        stages.push(ScriptStructureStagePlan::new(
            stage_name(instance, index),
            block_count,
            u64::from(block_count),
            materials_of(&counts),
        ));
    }
    Ok(stages)
}
fn rotated_extent(size: [i32; 3], turn: QuarterTurn) -> [i32; 3] {
    match turn {
        QuarterTurn::None | QuarterTurn::Cw180 => size,
        QuarterTurn::Cw90 | QuarterTurn::Cw270 => [size[2], size[1], size[0]],
    }
}
fn bounds_overlap(left: &ScriptSurveyBounds, right: &ScriptSurveyBounds) -> bool {
    (0..3).all(|axis| left.min[axis] <= right.max[axis] && right.min[axis] <= left.max[axis])
}
/// Durable structure identity: unique per committed transaction, stable for the
/// lifetime of the record.
fn mint_structure_id(
    plugin_id: &str,
    transaction_id: u64,
    blueprint_id: &str,
    anchor: [i32; 3],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SETTLEMENT_DOMAIN);
    hasher.update(b"structure");
    hasher.update((plugin_id.len() as u32).to_le_bytes());
    hasher.update(plugin_id.as_bytes());
    hasher.update(transaction_id.to_le_bytes());
    hasher.update((blueprint_id.len() as u32).to_le_bytes());
    hasher.update(blueprint_id.as_bytes());
    for value in anchor {
        hasher.update(value.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// One planned cell of a stage, after palette and rotation are resolved.
#[derive(Debug, Clone)]
struct StageCell {
    pos: [i32; 3],
    state: mc_world::BlockStateId,
    block: String,
}
fn stage_name(instance: &BlueprintInstance, index: usize) -> String {
    instance
        .blueprint()
        .stages()
        .get(index)
        .map_or_else(|| BODY_STAGE.to_owned(), |stage| stage.id.clone())
}
/// Resolve one stage into world-space cells, one per distinct position.
///
/// A stage that authors several cells at one position keeps only the last
/// authored state, which is what applying every cell in order leaves behind, so
/// a committed portion can never split a position group.
fn stage_cells(
    instance: &BlueprintInstance,
    index: usize,
) -> Result<Vec<StageCell>, ScriptOperationFailure> {
    let blueprint = instance.blueprint();
    let (blocks, placed): (&[BlueprintBlock], Vec<mc_worldgen::PlacedBlock>) =
        if blueprint.stages().is_empty() {
            if index != 0 {
                return Err(ScriptOperationFailure::InvalidRequest);
            }
            (blueprint.blocks(), instance.placed_blocks())
        } else {
            let Some(stage) = blueprint.stages().get(index) else {
                return Err(ScriptOperationFailure::InvalidRequest);
            };
            (stage.blocks.as_slice(), instance.placed_stage_blocks(stage))
        };
    let palette: BTreeMap<u16, &str> = blueprint
        .palette()
        .iter()
        .map(|entry| (entry.index, entry.block.as_str()))
        .collect();
    let mut authored = Vec::with_capacity(blocks.len());
    for block in blocks {
        let block_id = palette
            .get(&block.palette)
            .copied()
            .ok_or(ScriptOperationFailure::InvalidRequest)?;
        authored.push((placed_position(instance, block), block_id));
    }
    // `placed` is sorted by position with a stable sort over the authored order,
    // so the two sequences describe the same cells one to one.
    authored.sort_by_key(|(pos, _)| *pos);
    if authored.len() != placed.len() {
        return Err(ScriptOperationFailure::InvalidRequest);
    }
    let mut cells: Vec<StageCell> = authored
        .into_iter()
        .zip(placed)
        .map(|((pos, block), placed)| {
            debug_assert_eq!(pos, placed.pos);
            StageCell {
                pos,
                state: placed.state,
                block: block.to_owned(),
            }
        })
        .collect();
    cells.reverse();
    cells.dedup_by(|left, right| left.pos == right.pos);
    cells.reverse();
    Ok(cells)
}
/// World position of one blueprint cell, matching
/// [`mc_worldgen::BlueprintInstance`] placement exactly.
fn placed_position(instance: &BlueprintInstance, block: &BlueprintBlock) -> [i32; 3] {
    let local = instance
        .turn()
        .rotate_offset([block.x, block.y, block.z], instance.blueprint().size());
    let origin = instance.origin();
    [
        origin[0].saturating_add(local[0]),
        origin[1].saturating_add(local[1]),
        origin[2].saturating_add(local[2]),
    ]
}
fn materials_of(counts: &BTreeMap<String, u64>) -> Vec<ScriptStructureMaterial> {
    counts
        .iter()
        .filter(|(_, quantity)| **quantity > 0)
        .take(MAX_STRUCTURE_RESOURCE_TYPES)
        .map(|(resource, quantity)| ScriptStructureMaterial::new(resource.clone(), *quantity))
        .collect()
}
fn planned_remaining(structure: &DurableStructure) -> BTreeMap<String, u64> {
    let mut remaining: BTreeMap<String, u64> = BTreeMap::new();
    for stage in &structure.stages {
        for material in &stage.materials {
            *remaining.entry(material.resource.clone()).or_insert(0) += material.quantity;
        }
    }
    for (resource, consumed) in &structure.consumed {
        let entry = remaining.entry(resource.clone()).or_insert(0);
        *entry = entry.saturating_sub(*consumed);
    }
    remaining.retain(|_, quantity| *quantity > 0);
    remaining
}
pub(super) fn structure_snapshot(
    storage: &PluginStorage,
    structure: &DurableStructure,
) -> ScriptStructureSnapshot {
    // `remaining` is the material this structure may still spend. A terminal
    // structure has none: a committed one spent its whole plan, and a cancelled
    // one returned the rest. An active structure with no reservation yet reports
    // what its plan still needs rather than claiming unset-aside material.
    let remaining = match structure
        .reservation_ref()
        .and_then(|reference| storage.settlement_reservation(&structure.plugin_id, reference))
    {
        Some((_, reservation)) if structure.is_active() => reservation
            .quantities
            .iter()
            .map(|quantity| (quantity.resource_id.clone(), quantity.remaining))
            .collect::<BTreeMap<_, _>>(),
        Some(_) => BTreeMap::new(),
        None if structure.is_active() => planned_remaining(structure),
        None => BTreeMap::new(),
    };
    let (current_stage_index, completed_work_units) = structure.snapshot_progress();
    ScriptStructureSnapshot::new(
        structure.structure_id.clone(),
        structure.blueprint_id.clone(),
        structure.site_id.clone(),
        structure.state,
        structure.revision,
        structure.origin,
        structure.rotation,
        structure.reserved_footprint,
        structure.stages.clone(),
        current_stage_index,
        completed_work_units,
        structure.resource_plan_hash.clone(),
        structure.reservation_ref.clone(),
        structure.watermark,
        materials_of(&structure.consumed),
        materials_of(&remaining),
        structure.pause_reason.clone(),
    )
}
