//! Warehouse binding: durable binding records, source resolution and the three
//! bind operations.
//!
//! Moved out of `settlement.rs` by CP-REFACTOR-030. The ledger authority, the
//! shared atomic `apply` and the non-warehouse concerns stay in the parent
//! module; `settlement.rs` resolves the live world and dispatches here.
//! Sibling items are reached through `use super::*` so the move does not grow a
//! parallel import surface.

use super::*;

/// One durable, owner-bound warehouse handle.
///
/// A binding grants exactly one plugin access to one core-resolved container.
/// `revision` is the storage transaction that minted the binding, which is also
/// the fence a warehouse endpoint reports. `handle` is opaque and is the only
/// identity a plugin ever names back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::script::storage) enum DurableWarehouseSource {
    Authored {
        structure_id: String,
        container_id: u32,
    },
    Village {
        site_id: String,
        container_id: u32,
        position: [i32; 3],
    },
    Manual {
        position: [i32; 3],
    },
}

/// One warehouse handle's durable source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::script::storage) struct DurableWarehouseBinding {
    pub(super) handle: String,
    pub(super) plugin_id: String,
    pub(super) source: DurableWarehouseSource,
    pub(super) revision: u64,
}

impl DurableWarehouseBinding {
    fn source(&self) -> ScriptWarehouseSource {
        match &self.source {
            DurableWarehouseSource::Authored {
                structure_id,
                container_id,
            } => ScriptWarehouseSource::Authored {
                structure_id: structure_id.clone(),
                container_id: *container_id,
            },
            DurableWarehouseSource::Village {
                site_id,
                container_id,
                ..
            } => ScriptWarehouseSource::VanillaVillage {
                site_id: site_id.clone(),
                container_id: *container_id,
            },
            DurableWarehouseSource::Manual { position, .. } => ScriptWarehouseSource::Manual {
                position: *position,
            },
        }
    }

    fn snapshot(&self) -> ScriptWarehouseBinding {
        ScriptWarehouseBinding::new(self.handle.clone(), self.source(), self.revision)
    }
}

impl SettlementLedger {
    /// Exact world position of one binding, resolving authored containers
    /// through their structure. Village and manual bindings carry it durably.
    ///
    /// An authored binding whose structure or catalog no longer resolves maps
    /// to no position and is skipped by the cross-kind check: it no longer
    /// names a committable container, and failing every later bind on catalog
    /// drift would be worse. No absolute invariant under catalog drift.
    fn binding_position(
        &self,
        runtime: &SettlementRuntime,
        binding: &DurableWarehouseBinding,
    ) -> Option<[i32; 3]> {
        match &binding.source {
            DurableWarehouseSource::Authored {
                structure_id,
                container_id,
            } => {
                let structure = self.structure(structure_id)?;
                warehouse_container_position(runtime, structure, *container_id).ok()
            }
            DurableWarehouseSource::Village { position, .. }
            | DurableWarehouseSource::Manual { position } => Some(*position),
        }
    }
    /// Any binding resolving to `position`, whatever its kind: one physical
    /// container carries at most one handle.
    fn warehouse_for_position(
        &self,
        runtime: &SettlementRuntime,
        position: [i32; 3],
    ) -> Option<&DurableWarehouseBinding> {
        self.warehouses.values().find(|binding| {
            self.binding_position(runtime, binding)
                .is_some_and(|resolved| resolved == position)
        })
    }
}

pub(super) fn validate_warehouse_binding(
    binding: &DurableWarehouseBinding,
) -> Result<(), PluginStorageStartError> {
    validate_identifier(&binding.handle, MAX_WAREHOUSE_HANDLE_BYTES)?;
    validate_identifier(&binding.plugin_id, 128)?;
    match &binding.source {
        DurableWarehouseSource::Authored { structure_id, .. } => {
            validate_identifier(structure_id, MAX_STRUCTURE_ID_BYTES)?
        }
        DurableWarehouseSource::Village {
            site_id, position, ..
        } => {
            validate_identifier(site_id, 64)?;
            validate_warehouse_position(position)?;
        }
        DurableWarehouseSource::Manual { position } => validate_warehouse_position(position)?,
    }
    validate_revision(binding.revision)
}

fn validate_warehouse_position(position: &[i32; 3]) -> Result<(), PluginStorageStartError> {
    if position
        .iter()
        .any(|coordinate| coordinate.unsigned_abs() > mc_script::MAX_SCRIPT_BLOCK_COORDINATE as u32)
    {
        return Err(PluginStorageStartError::Malformed(
            "settlement warehouse position",
        ));
    }
    Ok(())
}

pub(super) fn same_warehouse_target(
    left: &DurableWarehouseBinding,
    right: &DurableWarehouseBinding,
) -> bool {
    match (&left.source, &right.source) {
        (
            DurableWarehouseSource::Authored {
                structure_id: left_id,
                container_id: left_container,
            },
            DurableWarehouseSource::Authored {
                structure_id: right_id,
                container_id: right_container,
            },
        ) => left_id == right_id && left_container == right_container,
        (
            DurableWarehouseSource::Village {
                site_id: left_site,
                position: left_position,
                ..
            },
            DurableWarehouseSource::Village {
                site_id: right_site,
                position: right_position,
                ..
            },
        ) => left_site == right_site && left_position == right_position,
        (
            DurableWarehouseSource::Manual {
                position: left_position,
            },
            DurableWarehouseSource::Manual {
                position: right_position,
            },
        ) => left_position == right_position,
        _ => false,
    }
}

fn village_warehouse_handle(
    plugin_id: &str,
    site_id: &str,
    container_id: u32,
) -> Result<String, ()> {
    let handle = format!("warehouse:{plugin_id}:village:{site_id}:{container_id}");
    (handle.len() <= MAX_WAREHOUSE_HANDLE_BYTES)
        .then_some(handle)
        .ok_or(())
}

fn manual_warehouse_handle(plugin_id: &str, position: [i32; 3]) -> Result<String, ()> {
    let handle = format!(
        "warehouse:{plugin_id}:manual:{}:{}:{}",
        position[0], position[1], position[2]
    );
    (handle.len() <= MAX_WAREHOUSE_HANDLE_BYTES)
        .then_some(handle)
        .ok_or(())
}

/// World position of one authored container of a durable structure.
///
/// `container_id` is the ordinal of an `empty_container` seed in the
/// blueprint's authored block-entity order, which is rotation independent; the
/// returned position projects that seed through the structure's own rotation
/// and origin, exactly as [`BlueprintInstance::placed_block_entities`] reports
/// it. A catalog whose blueprint no longer matches the content hash the
/// structure was prepared against is a stale revision, never a silently
/// different container.
pub(super) fn warehouse_container_position(
    runtime: &SettlementRuntime,
    structure: &DurableStructure,
    container_id: u32,
) -> Result<[i32; 3], ScriptOperationFailure> {
    let blueprint = runtime
        .catalog()
        .get(&structure.blueprint_id)
        .ok_or(ScriptOperationFailure::NotFound)?;
    if blueprint.content_hash() != structure.blueprint_hash {
        return Err(ScriptOperationFailure::StaleRevision);
    }
    let turn = QuarterTurn::from_degrees(structure.rotation)
        .ok_or(ScriptOperationFailure::InvalidRequest)?;
    let index =
        usize::try_from(container_id).map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    let local = blueprint
        .block_entities()
        .iter()
        .filter(|seed| seed.kind == BlockEntitySeedKind::EmptyContainer)
        .nth(index)
        .ok_or(ScriptOperationFailure::NotFound)?
        .at;
    let instance = BlueprintInstance::new(Arc::clone(blueprint), turn, structure.origin);
    let rotated = instance
        .turn()
        .rotate_offset(local, instance.blueprint().size());
    let origin = instance.origin();
    Ok([
        origin[0].saturating_add(rotated[0]),
        origin[1].saturating_add(rotated[1]),
        origin[2].saturating_add(rotated[2]),
    ])
}

impl crate::script::storage::InventoryRuntime {
    /// Bind and return one warehouse handle for an authored container of an
    /// active structure the caller owns.
    ///
    /// The plugin names its own durable structure and an authored container
    /// ordinal; core verifies ownership, liveness, that the ordinal is an
    /// authored `empty_container` seed of that structure's blueprint, and that
    /// the chunk is loaded with a container at the placed position. The binding
    /// is durable and idempotent: a container already bound to the same plugin
    /// answers its original handle and revision without a second ledger entry,
    /// and a container bound to another plugin is refused.
    pub(super) fn bind_warehouse(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        structure_id: &str,
        container_id: u32,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let Some(structure) = storage.settlements().structure(structure_id).cloned() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if structure.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if !structure.is_placed() {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let position = match warehouse_container_position(runtime, &structure, container_id) {
            Ok(position) => position,
            Err(failure) => return Ok(rejected(failure)),
        };
        let existing = storage
            .settlements()
            .warehouse_for_position(runtime, position);
        if let Some(existing) = existing {
            if existing.plugin_id != plugin_id {
                return Ok(rejected(ScriptOperationFailure::Forbidden));
            }
            // A repeated bind is the same binding: answer the original handle
            // and revision, but still record the receipt for this operation id
            // so the plugin's durable intent recovers through
            // `operation_status`. No ledger change is appended, so the binding
            // and its revision stay exactly as minted.
            return commit_settlement(
                storage,
                plugin_id,
                request,
                settlement_payload(ScriptSettlementResult::Warehouse {
                    binding: Box::new(existing.snapshot()),
                }),
                Vec::new(),
            );
        }
        match world.container_reading(position) {
            Ok(ContainerReading::Loaded(_)) => {}
            Ok(ContainerReading::Unloaded) => {
                return Ok(rejected(ScriptOperationFailure::Unloaded));
            }
            Ok(ContainerReading::Missing) => {
                return Ok(rejected(ScriptOperationFailure::NotFound));
            }
            Err(failure) => return Ok(rejected(failure)),
        }
        let handle = match warehouse_handle(plugin_id, structure_id, container_id) {
            Ok(handle) => handle,
            Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        };
        let binding = DurableWarehouseBinding {
            handle,
            plugin_id: plugin_id.to_owned(),
            source: DurableWarehouseSource::Authored {
                structure_id: structure_id.to_owned(),
                container_id,
            },
            revision: 0,
        };
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Warehouse {
                binding: Box::new(binding.snapshot()),
            }),
            vec![DurableSettlementChange::Warehouse {
                binding: Box::new(binding),
            }],
        )
    }
    /// Bind a materialized container from one generator-authenticated vanilla
    /// village through the normal opaque warehouse authority.
    pub(super) fn bind_village_warehouse(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        site_id: &str,
        container_id: u32,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let village = match generated_village(runtime, site_id) {
            Ok(village) => village,
            Err(failure) => return Ok(rejected(failure)),
        };
        let bounds = match village_bounds(&village) {
            Ok(bounds) => bounds,
            Err(failure) => return Ok(rejected(failure)),
        };
        let mut containers = match world.village_containers(bounds) {
            Ok(VillageReading::Loaded(containers)) => containers,
            Ok(VillageReading::Unloaded) => return Ok(rejected(ScriptOperationFailure::Unloaded)),
            Err(failure) => return Ok(rejected(failure)),
        };
        containers.sort_unstable();
        let index = match usize::try_from(container_id) {
            Ok(index) => index,
            Err(_) => return Ok(rejected(ScriptOperationFailure::NotFound)),
        };
        let Some(position) = containers.get(index).copied() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        let existing = storage
            .settlements()
            .warehouse_for_position(runtime, position);
        if let Some(existing) = existing {
            if existing.plugin_id != plugin_id {
                return Ok(rejected(ScriptOperationFailure::Forbidden));
            }
            return commit_settlement(
                storage,
                plugin_id,
                request,
                settlement_payload(ScriptSettlementResult::Warehouse {
                    binding: Box::new(existing.snapshot()),
                }),
                Vec::new(),
            );
        }
        match world.container_reading(position) {
            Ok(ContainerReading::Loaded(_)) => {}
            Ok(ContainerReading::Unloaded) => {
                return Ok(rejected(ScriptOperationFailure::Unloaded));
            }
            Ok(ContainerReading::Missing) => return Ok(rejected(ScriptOperationFailure::NotFound)),
            Err(failure) => return Ok(rejected(failure)),
        }
        let handle = match village_warehouse_handle(plugin_id, site_id, container_id) {
            Ok(handle) => handle,
            Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        };
        let binding = DurableWarehouseBinding {
            handle,
            plugin_id: plugin_id.to_owned(),
            source: DurableWarehouseSource::Village {
                site_id: site_id.to_owned(),
                container_id,
                position,
            },
            revision: 0,
        };
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Warehouse {
                binding: Box::new(binding.snapshot()),
            }),
            vec![DurableSettlementChange::Warehouse {
                binding: Box::new(binding),
            }],
        )
    }
    /// Bind one player-built container named by its exact surveyed position.
    ///
    /// The survey token is the fence, not a second certificate: it must be the
    /// caller's own live token whose bounds contain `position`, minted before
    /// the bind and unchanged since. A foreign zone claimed after the survey is
    /// re-checked before the commit, because a zone change leaves no block
    /// digest for the footprint fence to catch. The container must be loaded
    /// and present; the durable result is the binding itself at its exact
    /// position and owner. A repeat bind answers the original handle, and one
    /// physical container carries at most one handle across all kinds.
    pub(super) fn bind_manual_warehouse(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        survey_token: &str,
        position: [i32; 3],
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let Some(token) = storage.settlements().survey(survey_token) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if token.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if token.dimension != VILLAGE_DIMENSION {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        }
        if !(token.bounds.min[0]..=token.bounds.max[0]).contains(&position[0])
            || !(token.bounds.min[1]..=token.bounds.max[1]).contains(&position[1])
            || !(token.bounds.min[2]..=token.bounds.max[2]).contains(&position[2])
        {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        if world.claims_overlap(plugin_id, token.bounds) {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        // Availability before freshness: an unloaded target chunk makes the
        // footprint digest unreadable, which the fence would otherwise report
        // as a changed site. The contract promises `unloaded` here so the
        // caller retries when the chunk loads instead of discarding its fence.
        match world.container_reading(position) {
            Ok(ContainerReading::Loaded(_)) => {}
            Ok(ContainerReading::Unloaded) => {
                return Ok(rejected(ScriptOperationFailure::Unloaded));
            }
            Ok(ContainerReading::Missing) => return Ok(rejected(ScriptOperationFailure::NotFound)),
            Err(failure) => return Ok(rejected(failure)),
        }
        if token.expires_transaction < storage.revision
            || world.footprint_changed_since(token.bounds, token.world_revision)
        {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let existing = storage
            .settlements()
            .warehouse_for_position(runtime, position);
        if let Some(existing) = existing {
            if existing.plugin_id != plugin_id {
                return Ok(rejected(ScriptOperationFailure::Forbidden));
            }
            return commit_settlement(
                storage,
                plugin_id,
                request,
                settlement_payload(ScriptSettlementResult::Warehouse {
                    binding: Box::new(existing.snapshot()),
                }),
                Vec::new(),
            );
        }
        let handle = match manual_warehouse_handle(plugin_id, position) {
            Ok(handle) => handle,
            Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        };
        let binding = DurableWarehouseBinding {
            handle,
            plugin_id: plugin_id.to_owned(),
            source: DurableWarehouseSource::Manual { position },
            revision: 0,
        };
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Warehouse {
                binding: Box::new(binding.snapshot()),
            }),
            vec![DurableSettlementChange::Warehouse {
                binding: Box::new(binding),
            }],
        )
    }
}
