//! Durable settlement sites, surveys, and staged construction.
//!
//! This module owns the core authority of the settlement operations: site
//! discovery and query over the deterministic worldgen selector, resident site
//! reservations against a site point of interest, bounded terrain surveys, and
//! the prepare/advance/pause/cancel lifecycle of a staged structure. Nothing
//! here trusts a plugin: a site id is resolved through the selector (a forged id
//! has no cell), every structure is owner scoped, and every committed portion is
//! checked against the durable C1 reservation before it is applied.
//!
//! Durable state lives in the plugin storage journal next to the operation
//! receipts that make every mutation idempotent, so a replayed journal rebuilds
//! the exact same ledger. Authoritative terrain is reached only through
//! [`SettlementWorld`]; when no adapter is installed the operations that need
//! terrain answer `runtime_unavailable` instead of guessing.

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::Arc;

use mc_data::ItemStack;
use mc_entity::Vec3;
use mc_script::{
    MAX_BLUEPRINT_FOOTPRINT_AXIS, MAX_BLUEPRINT_ID_BYTES, MAX_SCRIPT_WORLD_TIME,
    MAX_SETTLEMENT_POIS, MAX_SETTLEMENT_RESIDENTS, MAX_SETTLEMENT_SITE_AXIS,
    MAX_SETTLEMENT_SITE_PAGE, MAX_STRUCTURE_ACTIVE_PER_PLUGIN, MAX_STRUCTURE_ID_BYTES,
    MAX_STRUCTURE_RESOURCE_TYPES, MAX_STRUCTURE_STAGES, MAX_SURVEY_TOKEN_BYTES,
    MAX_WAREHOUSE_HANDLE_BYTES, MAX_WORLD_COMMIT_PORTION, ScriptBuildingCertificate,
    ScriptChunkAvailability, ScriptInventoryEndpoint, ScriptInventoryMaterial,
    ScriptInventoryResourcePlan, ScriptInventoryWorkPortion, ScriptOperation,
    ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest,
    ScriptResidentSiteReservation, ScriptSettlementBuilding, ScriptSettlementOperation,
    ScriptSettlementPoi, ScriptSettlementResult, ScriptSettlementSite, ScriptSettlementSitePage,
    ScriptSitePoiKind, ScriptSitePoiState, ScriptSiteProvenance, ScriptSiteVariant,
    ScriptStructureMaterial, ScriptStructureReceipt, ScriptStructureSnapshot,
    ScriptStructureStagePlan, ScriptStructureState, ScriptSurveyBounds, ScriptSurveySnapshot,
    ScriptWarehouseBinding, ScriptWarehouseSource, resident_generation_id,
    resident_generation_id_for_poi, warehouse_handle,
};
use mc_world::GeneratedVillageSite;
use mc_worldgen::{
    BlockEntitySeedKind, BlueprintBlock, BlueprintCatalog, BlueprintInstance, PoiKind, QuarterTurn,
    SITE_CELL_BLOCKS, SettlementSelector, SiteCandidate, SiteLayout, SiteVariant,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    DurableOperationReceipt, PluginStorage, PluginStorageMutationError, PluginStorageStartError,
    PreparedStorageBatch, ScriptStoragePrepareOutcome, VillageSiteGround, cell_chunk_bounds,
    village_poi_capacity, village_site_id, village_site_start_chunk,
};
use crate::play::owned_inventory::{
    WarehouseTransferRequest, owned_inventory_fingerprint, resource_plan_hash,
};

mod certificate;
mod construction;
use construction::structure_snapshot;
mod survey;
mod warehouse;

use certificate::{DurableBuildingCertificate, validate_building_certificate};
use survey::{DurableSurveyToken, validate_survey_token};
use warehouse::{
    DurableWarehouseBinding, DurableWarehouseSource, same_warehouse_target,
    validate_warehouse_binding, warehouse_container_position,
};

/// The dimension a generated vanilla village belongs to.
///
/// The settlement lane's v1 profile owns overworld terrain only — the live
/// world adapter refuses a survey of any other dimension — so a generated site
/// is an overworld site and its id is minted for that dimension.
const VILLAGE_DIMENSION: &str = "minecraft:overworld";
/// Blocks along one chunk axis.
const CHUNK_AXIS: i32 = 16;
/// Domain separator for every deterministic settlement identity.
const SETTLEMENT_DOMAIN: &[u8] = b"solaris.settlement.v1";
/// Stage name of a blueprint that authors no construction stages.
const BODY_STAGE: &str = "body";
/// Pause reason recorded when the reserved footprint changed under a structure.
const PAUSE_SITE_CHANGED: &str = "site_changed";

/// One point-of-interest reservation inside a settlement site.
///
/// The resident ledger is the authority for `consumed`; the durable bit mirrors
/// the observation so a site snapshot stays truthful after a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurablePoiReservation {
    token: String,
    plugin_id: String,
    consumed: bool,
    released: bool,
}

/// Durable reservation state of one settlement site. Reservations are global
/// world state: a home occupied by one plugin's villager is not free for
/// another, so entries are keyed by point of interest and carry their owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableSiteState {
    site_id: String,
    pois: BTreeMap<String, DurablePoiReservation>,
    revision: u64,
}

impl DurableSiteState {
    fn new(site_id: String) -> Self {
        Self {
            site_id,
            pois: BTreeMap::new(),
            revision: 0,
        }
    }

    fn reservation(&self, poi_id: &str) -> Option<&DurablePoiReservation> {
        self.pois.get(poi_id)
    }
}

/// Durable state of one staged structure.
///
/// `stages` and `reserved_footprint` are persisted with the plan so a restart
/// reconstructs the exact snapshot without re-deriving the catalog, and
/// `blueprint_hash` pins the authored blueprint the plan was built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableStructure {
    structure_id: String,
    plugin_id: String,
    site_id: String,
    blueprint_id: String,
    blueprint_hash: String,
    origin: [i32; 3],
    rotation: u16,
    reserved_footprint: [i32; 3],
    state: ScriptStructureState,
    stages: Vec<ScriptStructureStagePlan>,
    stage_index: usize,
    watermark: u64,
    built_blocks: u64,
    reservation_ref: Option<String>,
    resource_plan_hash: String,
    consumed: BTreeMap<String, u64>,
    prepare_revision: u64,
    pause_reason: Option<String>,
    revision: u64,
}

impl DurableStructure {
    fn reservation_ref(&self) -> Option<&str> {
        self.reservation_ref.as_deref()
    }

    fn is_active(&self) -> bool {
        matches!(
            self.state,
            ScriptStructureState::Prepared
                | ScriptStructureState::Running
                | ScriptStructureState::Paused
        )
    }

    /// Whether this structure is still placed in the world.
    ///
    /// Every state but `Cancelled` is placed: a completed structure keeps the
    /// ground it was built on and its authored containers are the warehouses
    /// the settlement profile exists to expose, so warehouse handles must
    /// survive construction and outlive it. Only a cancelled (removed)
    /// structure stops being warehouse-addressable; a structure that never
    /// existed is refused before this predicate is reached.
    fn is_placed(&self) -> bool {
        !matches!(self.state, ScriptStructureState::Cancelled)
    }

    fn stage(&self) -> Option<&ScriptStructureStagePlan> {
        self.stages.get(self.stage_index)
    }

    /// Progress exposed to scripts directly from the durable construction record.
    ///
    /// The terminal stage index is one past the final authored stage and has no
    /// in-stage work, so a recovered worker does not need to infer completion
    /// from lifecycle state or aggregate material accounting.
    fn snapshot_progress(&self) -> (u32, u64) {
        let current_stage_index =
            u32::try_from(self.stage_index).expect("structure stage count is bounded");
        if self.stage_index == self.stages.len() {
            return (current_stage_index, 0);
        }
        let completed_prior_stages = self.stages[..self.stage_index]
            .iter()
            .try_fold(0_u64, |total, stage| total.checked_add(stage.work_units))
            .expect("durable structure progress is validated");
        (
            current_stage_index,
            self.built_blocks
                .checked_sub(completed_prior_stages)
                .expect("durable structure progress is validated"),
        )
    }

    /// Reserved territory of the placed blueprint, in world coordinates.
    fn bounds(&self) -> ScriptSurveyBounds {
        bounds_of(self.origin, self.reserved_footprint)
    }
}

/// Owner, committed reservation and revision of one prepared structure, resolved
/// for the C4 resident builder path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StructureBuilderTarget {
    pub(super) plugin_id: String,
    pub(super) reservation_ref: String,
    pub(super) revision: u64,
}

/// One durable ledger change applied inside a storage frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum DurableSettlementChange {
    Site {
        site: Box<DurableSiteState>,
    },
    Structure {
        structure: Box<DurableStructure>,
    },
    Survey {
        survey: Box<DurableSurveyToken>,
    },
    Warehouse {
        binding: Box<DurableWarehouseBinding>,
    },
    Building {
        certificate: Box<DurableBuildingCertificate>,
    },
}

impl DurableSettlementChange {
    pub(super) fn revision(&self) -> u64 {
        match self {
            Self::Site { site } => site.revision,
            Self::Structure { structure } => structure.revision,
            Self::Survey { survey } => survey.revision,
            Self::Warehouse { binding } => binding.revision,
            Self::Building { certificate } => certificate.revision,
        }
    }

    fn set_revision(&mut self, revision: u64) {
        match self {
            Self::Site { site } => site.revision = revision,
            Self::Structure { structure } => structure.revision = revision,
            Self::Survey { survey } => survey.revision = revision,
            Self::Warehouse { binding } => binding.revision = revision,
            Self::Building { certificate } => certificate.revision = revision,
        }
    }

    pub(super) fn validate(&self) -> Result<(), PluginStorageStartError> {
        match self {
            Self::Site { site } => validate_site_state(site),
            Self::Structure { structure } => validate_structure(structure),
            Self::Survey { survey } => validate_survey_token(survey),
            Self::Warehouse { binding } => validate_warehouse_binding(binding),
            Self::Building { certificate } => validate_building_certificate(certificate),
        }
    }
}

/// Core-owned settlement ledger, replayed from the journal on open.
#[derive(Debug, Default)]
pub(super) struct SettlementLedger {
    sites: BTreeMap<String, DurableSiteState>,
    structures: BTreeMap<String, DurableStructure>,
    surveys: BTreeMap<String, DurableSurveyToken>,
    warehouses: BTreeMap<String, DurableWarehouseBinding>,
    buildings: BTreeMap<String, DurableBuildingCertificate>,
}

impl SettlementLedger {
    pub(super) fn site(&self, site_id: &str) -> Option<&DurableSiteState> {
        self.sites.get(site_id)
    }

    pub(super) fn structure(&self, structure_id: &str) -> Option<&DurableStructure> {
        self.structures.get(structure_id)
    }

    /// The immutable certificate one id names, owner included.
    pub(super) fn building(&self, certificate_id: &str) -> Option<&DurableBuildingCertificate> {
        self.buildings.get(certificate_id)
    }

    /// The binding one opaque warehouse handle names, owner included.
    pub(super) fn warehouse(&self, handle: &str) -> Option<&DurableWarehouseBinding> {
        self.warehouses.get(handle)
    }

    /// Structures of one plugin that still hold their reserved territory.
    pub(super) fn active_structures(&self, plugin_id: &str) -> Vec<&DurableStructure> {
        let mut active: Vec<&DurableStructure> = self
            .structures
            .values()
            .filter(|structure| structure.plugin_id == plugin_id && structure.is_active())
            .collect();
        active.sort_unstable_by(|left, right| left.structure_id.cmp(&right.structure_id));
        active
    }

    /// Structures of one plugin, active or not: every one of them still owns the
    /// ground it was prepared against, because built blocks are never removed.
    fn structures_of(&self, plugin_id: &str) -> impl Iterator<Item = &DurableStructure> {
        self.structures
            .values()
            .filter(move |structure| structure.plugin_id == plugin_id)
    }

    /// The site state and point of interest carrying one spawn-site token.
    fn reservation_by_token(&self, token: &str) -> Option<(&DurableSiteState, &str)> {
        for site in self.sites.values() {
            for (poi_id, reservation) in &site.pois {
                if reservation.token == token {
                    return Some((site, poi_id.as_str()));
                }
            }
        }
        None
    }

    pub(super) fn apply(
        &mut self,
        change: &DurableSettlementChange,
    ) -> Result<(), PluginStorageStartError> {
        match change {
            DurableSettlementChange::Site { site } => {
                validate_site_state(site)?;
                if let Some(previous) = self.sites.get(&site.site_id) {
                    if previous == site.as_ref() {
                        return Ok(());
                    }
                    if previous.revision >= site.revision {
                        return Err(PluginStorageStartError::Malformed(
                            "settlement site revision",
                        ));
                    }
                    // Reservations are never dropped: another plugin's occupied
                    // home must survive any later change to the same site.
                    if previous
                        .pois
                        .keys()
                        .any(|poi_id| !site.pois.contains_key(poi_id))
                    {
                        return Err(PluginStorageStartError::Malformed("settlement poi dropped"));
                    }
                }
                self.sites
                    .insert(site.site_id.clone(), site.as_ref().clone());
            }
            DurableSettlementChange::Structure { structure } => {
                validate_structure(structure)?;
                if let Some(previous) = self.structures.get(&structure.structure_id) {
                    if previous == structure.as_ref() {
                        return Ok(());
                    }
                    if previous.revision >= structure.revision
                        || previous.plugin_id != structure.plugin_id
                    {
                        return Err(PluginStorageStartError::Malformed(
                            "settlement structure revision",
                        ));
                    }
                }
                self.structures
                    .insert(structure.structure_id.clone(), structure.as_ref().clone());
            }
            DurableSettlementChange::Survey { survey } => {
                validate_survey_token(survey)?;
                if let Some(previous) = self.surveys.get(&survey.token) {
                    if previous == survey.as_ref() {
                        return Ok(());
                    }
                    if previous.revision >= survey.revision
                        || previous.plugin_id != survey.plugin_id
                    {
                        return Err(PluginStorageStartError::Malformed(
                            "settlement survey revision",
                        ));
                    }
                }
                self.surveys
                    .insert(survey.token.clone(), survey.as_ref().clone());
            }
            DurableSettlementChange::Warehouse { binding } => {
                validate_warehouse_binding(binding)?;
                if let Some(previous) = self.warehouses.get(&binding.handle) {
                    if previous == binding.as_ref() {
                        return Ok(());
                    }
                    if previous.revision >= binding.revision
                        || previous.plugin_id != binding.plugin_id
                        || previous.source != binding.source
                    {
                        return Err(PluginStorageStartError::Malformed(
                            "settlement warehouse revision",
                        ));
                    }
                }
                // One core-resolved container carries at most one binding.
                if self.warehouses.values().any(|existing| {
                    existing.handle != binding.handle && same_warehouse_target(existing, binding)
                }) {
                    return Err(PluginStorageStartError::Malformed(
                        "settlement warehouse container rebound",
                    ));
                }
                self.warehouses
                    .insert(binding.handle.clone(), binding.as_ref().clone());
            }
            DurableSettlementChange::Building { certificate } => {
                validate_building_certificate(certificate)?;
                // A certificate is immutable: the same id with any different
                // record is malformed, never a capacity-changing update.
                if self
                    .buildings
                    .get(&certificate.certificate_id)
                    .is_some_and(|previous| previous != certificate.as_ref())
                {
                    return Err(PluginStorageStartError::Malformed(
                        "settlement building changed",
                    ));
                }
                // One normalized footprint carries at most one certificate: a
                // second id over the same volume is a rebound, never extra
                // capacity or workplaces.
                if self.buildings.values().any(|existing| {
                    existing.certificate_id != certificate.certificate_id
                        && existing.footprint == certificate.footprint
                }) {
                    return Err(PluginStorageStartError::Malformed(
                        "settlement building volume rebound",
                    ));
                }
                self.buildings.insert(
                    certificate.certificate_id.clone(),
                    certificate.as_ref().clone(),
                );
            }
        }
        Ok(())
    }

    pub(super) fn reset(&mut self) {
        self.sites.clear();
        self.structures.clear();
        self.surveys.clear();
        self.warehouses.clear();
        self.buildings.clear();
    }

    pub(super) fn change_log(&self) -> Vec<DurableSettlementChange> {
        self.sites
            .values()
            .map(|site| DurableSettlementChange::Site {
                site: Box::new(site.clone()),
            })
            .chain(
                self.structures
                    .values()
                    .map(|structure| DurableSettlementChange::Structure {
                        structure: Box::new(structure.clone()),
                    }),
            )
            .chain(
                self.surveys
                    .values()
                    .map(|survey| DurableSettlementChange::Survey {
                        survey: Box::new(survey.clone()),
                    }),
            )
            .chain(
                self.warehouses
                    .values()
                    .map(|binding| DurableSettlementChange::Warehouse {
                        binding: Box::new(binding.clone()),
                    }),
            )
            .chain(
                self.buildings
                    .values()
                    .map(|certificate| DurableSettlementChange::Building {
                        certificate: Box::new(certificate.clone()),
                    }),
            )
            .collect()
    }
}

pub(super) fn decode_settlement_change(
    payload: &[u8],
) -> Result<DurableSettlementChange, PluginStorageStartError> {
    let change: DurableSettlementChange = serde_json::from_slice(payload)
        .map_err(|_| PluginStorageStartError::Malformed("settlement change"))?;
    change.validate()?;
    Ok(change)
}

/// The settlement projection one operation receipt needs to be indexed: the
/// owner and the bound reservation reference.
pub(super) struct SettlementProjection {
    pub(super) plugin_id: String,
    pub(super) reservation_ref: Option<String>,
}

impl PluginStorage {
    pub(super) fn settlements(&self) -> &SettlementLedger {
        &self.settlements
    }

    /// Everything a settlement receipt needs to find the reservation its
    /// structure spends from.
    pub(super) fn settlement_projection(&self, structure_id: &str) -> Option<SettlementProjection> {
        let structure = self.settlements.structure(structure_id)?;
        Some(SettlementProjection {
            plugin_id: structure.plugin_id.clone(),
            reservation_ref: structure.reservation_ref().map(str::to_owned),
        })
    }

    /// Owner, committed reservation and revision of one prepared structure, for
    /// the C4 resident builder path.
    ///
    /// A structure that has not advanced yet holds no reservation reference, so
    /// the one matching C1 reservation is resolved by its immutable resource
    /// plan hash: unbound, or already bound to this exact structure. A structure
    /// without a compatible reservation cannot consume materials and is not a
    /// build target.
    pub(super) fn resident_builder_target(
        &self,
        structure_id: &str,
    ) -> Option<StructureBuilderTarget> {
        let structure = self.settlements.structure(structure_id)?;
        let reservation_ref = match structure.reservation_ref() {
            Some(reference) => reference.to_owned(),
            None => self
                .reservations
                .iter()
                .find_map(|((owner, reference), (_, snapshot))| {
                    (owner == &structure.plugin_id
                        && !snapshot.released
                        && snapshot.resource_plan_hash == structure.resource_plan_hash
                        && snapshot
                            .bound_to
                            .as_deref()
                            .is_none_or(|bound| bound == structure_id))
                    .then(|| reference.clone())
                })?,
        };
        Some(StructureBuilderTarget {
            plugin_id: structure.plugin_id.clone(),
            reservation_ref,
            revision: structure.revision,
        })
    }

    /// Prepare one settlement receipt together with the ledger changes it
    /// decided, so a replay restores both or neither.
    fn prepare_settlement_operation_batch(
        &mut self,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        mut payload: ScriptOperationPayload,
        mut changes: Vec<DurableSettlementChange>,
    ) -> Result<ScriptStoragePrepareOutcome<PreparedStorageBatch>, PluginStorageMutationError> {
        let transaction_id = self
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        stamp_settlement_payload(&mut payload, transaction_id);
        let outcome = ScriptOperationOutcome::committed(transaction_id, payload)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?;
        for change in &mut changes {
            change.set_revision(transaction_id);
        }
        Ok(ScriptStoragePrepareOutcome::Prepared(
            PreparedStorageBatch {
                transaction_id,
                plugin_id: plugin_id.to_owned(),
                mutations: Vec::new(),
                inventory: None,
                operation: Some(DurableOperationReceipt {
                    plugin_id: plugin_id.to_owned(),
                    operation_id: request
                        .operation_id()
                        .expect("settlement mutation decision identity")
                        .to_owned(),
                    request_id: request.request_id().to_owned(),
                    fingerprint: owned_inventory_fingerprint(request.operation()),
                    revision: transaction_id,
                    outcome,
                    delivered: false,
                }),
                resident: Vec::new(),
                settlement: changes,
                order: Vec::new(),
                treatment: None,
            },
        ))
    }

    /// Durably append settlement ledger changes that carry no plugin operation
    /// receipt: a survey mints its token while answering a query.
    fn prepare_settlement_change_batch(
        &mut self,
        plugin_id: &str,
        mut payload: ScriptOperationPayload,
        mut changes: Vec<DurableSettlementChange>,
    ) -> Result<
        ScriptStoragePrepareOutcome<(PreparedStorageBatch, ScriptOperationOutcome)>,
        PluginStorageMutationError,
    > {
        let transaction_id = self
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        stamp_settlement_payload(&mut payload, transaction_id);
        let outcome = ScriptOperationOutcome::committed(transaction_id, payload)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?;
        for change in &mut changes {
            change.set_revision(transaction_id);
        }
        Ok(ScriptStoragePrepareOutcome::Prepared((
            PreparedStorageBatch {
                transaction_id,
                plugin_id: plugin_id.to_owned(),
                mutations: Vec::new(),
                inventory: None,
                operation: None,
                resident: Vec::new(),
                settlement: changes,
                order: Vec::new(),
                treatment: None,
            },
            outcome,
        )))
    }
}

/// Stamp the committed transaction onto the snapshot a settlement payload
/// carries, so the fence the caller reads back is its own commit.
fn stamp_settlement_payload(payload: &mut ScriptOperationPayload, transaction_id: u64) {
    let ScriptOperationPayload::Settlement { result } = payload else {
        return;
    };
    match &mut **result {
        ScriptSettlementResult::ResidentSite { reservation } => {
            reservation.revision = transaction_id;
        }
        ScriptSettlementResult::Structure { structure } => {
            structure.revision = transaction_id;
        }
        ScriptSettlementResult::Receipt { receipt } => {
            receipt.revision = transaction_id;
        }
        ScriptSettlementResult::Warehouse { binding } => {
            // A freshly minted binding carries revision 0 and takes its own
            // commit; an already-bound repeat keeps the revision its durable
            // ledger entry has, so the receipt never claims a newer fence than
            // the binding a later read reports.
            if binding.revision == 0 {
                binding.revision = transaction_id;
            }
        }
        ScriptSettlementResult::Building { certificate } => {
            if certificate.revision == 0 {
                certificate.revision = transaction_id;
            }
        }
        _ => {}
    }
}

fn settlement_payload(result: ScriptSettlementResult) -> ScriptOperationPayload {
    ScriptOperationPayload::Settlement {
        result: Box::new(result),
    }
}

fn rejected(failure: ScriptOperationFailure) -> ScriptOperationOutcome {
    ScriptOperationOutcome::rejected(failure)
}

fn settled_outcome(revision: u64, result: ScriptSettlementResult) -> ScriptOperationOutcome {
    match ScriptOperationOutcome::committed(revision, settlement_payload(result)) {
        Ok(outcome) => outcome,
        Err(_) => rejected(ScriptOperationFailure::RuntimeUnavailable),
    }
}

fn validate_axis_bounded(size: [i32; 3], max: i32) -> Result<(), PluginStorageStartError> {
    if size.iter().any(|axis| *axis <= 0 || *axis > max) {
        return Err(PluginStorageStartError::Malformed("settlement footprint"));
    }
    Ok(())
}

fn validate_revision(revision: u64) -> Result<(), PluginStorageStartError> {
    if revision == 0 || revision > MAX_SCRIPT_WORLD_TIME {
        return Err(PluginStorageStartError::Malformed("settlement revision"));
    }
    Ok(())
}

fn validate_identifier(value: &str, max: usize) -> Result<(), PluginStorageStartError> {
    if value.is_empty()
        || value.len() > max
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
        })
    {
        return Err(PluginStorageStartError::Malformed("settlement id"));
    }
    Ok(())
}

fn validate_hex_hash(value: &str) -> Result<(), PluginStorageStartError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PluginStorageStartError::Malformed("settlement hash"));
    }
    Ok(())
}

fn validate_material_counts(counts: &BTreeMap<String, u64>) -> Result<(), PluginStorageStartError> {
    if counts.len() > MAX_STRUCTURE_RESOURCE_TYPES {
        return Err(PluginStorageStartError::Malformed("settlement materials"));
    }
    for (resource, quantity) in counts {
        validate_identifier(resource, 128)?;
        if !resource.contains(':') || *quantity == 0 || *quantity > MAX_SCRIPT_WORLD_TIME {
            return Err(PluginStorageStartError::Malformed("settlement materials"));
        }
    }
    Ok(())
}

fn validate_site_state(site: &DurableSiteState) -> Result<(), PluginStorageStartError> {
    validate_identifier(&site.site_id, 64)?;
    if site.pois.len() > MAX_SETTLEMENT_POIS || site.revision > MAX_SCRIPT_WORLD_TIME {
        return Err(PluginStorageStartError::Malformed("settlement site"));
    }
    for (poi_id, reservation) in &site.pois {
        validate_identifier(poi_id, 128)?;
        validate_identifier(&reservation.plugin_id, 128)?;
        validate_identifier(&reservation.token, 128)?;
    }
    Ok(())
}

fn validate_structure(structure: &DurableStructure) -> Result<(), PluginStorageStartError> {
    validate_identifier(&structure.structure_id, MAX_STRUCTURE_ID_BYTES)?;
    validate_identifier(&structure.plugin_id, 128)?;
    validate_identifier(&structure.site_id, 64)?;
    validate_identifier(&structure.blueprint_id, MAX_BLUEPRINT_ID_BYTES)?;
    validate_hex_hash(&structure.blueprint_hash)?;
    validate_hex_hash(&structure.resource_plan_hash)?;
    if let Some(reference) = structure.reservation_ref() {
        validate_identifier(reference, 64)?;
    }
    if let Some(reason) = &structure.pause_reason {
        validate_identifier(reason, 64)?;
    }
    validate_axis_bounded(structure.reserved_footprint, MAX_BLUEPRINT_FOOTPRINT_AXIS)?;
    if QuarterTurn::from_degrees(structure.rotation).is_none() {
        return Err(PluginStorageStartError::Malformed("settlement rotation"));
    }
    if structure.stages.is_empty() || structure.stages.len() > MAX_STRUCTURE_STAGES {
        return Err(PluginStorageStartError::Malformed("settlement stages"));
    }
    if structure.stage_index > structure.stages.len()
        || structure.watermark > MAX_SCRIPT_WORLD_TIME
        || structure.prepare_revision > MAX_SCRIPT_WORLD_TIME
    {
        return Err(PluginStorageStartError::Malformed("settlement progress"));
    }
    let mut total: u64 = 0;
    for stage in &structure.stages {
        stage
            .validate()
            .map_err(|_| PluginStorageStartError::Malformed("settlement stage"))?;
        let mut previous: Option<&str> = None;
        for material in &stage.materials {
            if previous.is_some_and(|value| value >= material.resource.as_str()) {
                return Err(PluginStorageStartError::Malformed("settlement stage order"));
            }
            previous = Some(material.resource.as_str());
        }
        total = total
            .checked_add(stage.work_units)
            .ok_or(PluginStorageStartError::Malformed("settlement work"))?;
    }
    let completed_prior_stages = structure.stages[..structure.stage_index]
        .iter()
        .try_fold(0_u64, |total, stage| total.checked_add(stage.work_units))
        .ok_or(PluginStorageStartError::Malformed("settlement work"))?;
    let terminal = structure.stage_index == structure.stages.len();
    let progress_is_valid = if terminal {
        structure.built_blocks == total && structure.state == ScriptStructureState::Committed
    } else {
        let stage_end = completed_prior_stages
            .checked_add(structure.stages[structure.stage_index].work_units)
            .ok_or(PluginStorageStartError::Malformed("settlement work"))?;
        structure.built_blocks >= completed_prior_stages && structure.built_blocks < stage_end
    };
    if !progress_is_valid || (structure.state == ScriptStructureState::Committed && !terminal) {
        return Err(PluginStorageStartError::Malformed("settlement progress"));
    }
    validate_material_counts(&structure.consumed)?;
    validate_revision(structure.revision)
}

/// The in-memory [`SettlementWorld`]s' stand-in for one receipt-bearing world
/// decision.
///
/// Those fakes own no resident chunk images, so they journal the caller's
/// encoded receipt under a fresh decision id. The submitter's ledger projection,
/// acknowledgement, and startup replay therefore use the same durable path as
/// a live server-owned container transfer or structure portion.
#[cfg(test)]
pub(super) fn journal_test_plugin_decision(
    sessions: &crate::play::SessionRegistry,
    receipt: Vec<u8>,
) -> Result<u64, ScriptOperationFailure> {
    let Some(journal) = sessions.world_chunk_journal() else {
        return Err(ScriptOperationFailure::RuntimeUnavailable);
    };

    let decision_id = journal
        .reserve_decision_ids(1)
        .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)?[0];
    journal
        .record_reserved_decisions(
            sessions.simulation_tick(),
            vec![(decision_id, Vec::new(), Some(receipt))],
        )
        .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)?;
    Ok(decision_id)
}

/// One resolved, writable warehouse container.
///
/// The reading endpoint and the writing endpoint resolve through the same
/// helper, so a handle that reads also writes and every refusal family stays
/// identical: the handle names a durable binding the caller owns, its structure
/// is still placed, and the authored position holds a loaded container.
#[derive(Debug, Clone)]
pub(super) struct ResolvedWarehouseContainer {
    /// The binding revision, the fence a warehouse endpoint reports.
    pub(super) revision: u64,
    pub(super) position: [i32; 3],
    pub(super) items: Vec<ItemStack>,
}

/// One loaded-container reading of the authoritative world.
///
/// The variant is what keeps a missing container distinct from an unloaded
/// chunk: `Missing` is a loaded chunk without a container block/entity at the
/// position, `Unloaded` is a chunk core cannot observe at all, and `Loaded`
/// carries the canonical slot items of the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContainerReading {
    Unloaded,
    Missing,
    Loaded(Vec<ItemStack>),
}

/// Bounds of one generator-authenticated vanilla village.
///
/// This internal authority uses the settlement site's 256-block footprint
/// contract rather than the public 128-block survey tile contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VillageBounds {
    pub(crate) min: [i32; 3],
    pub(crate) max: [i32; 3],
}

/// One bounded reading of a generated village's contents.
///
/// The variant is what keeps "the village holds none of these" distinct from
/// "core cannot see the village's chunks": an unloaded site reports no contents
/// at all, so a caller never mistakes a region nobody has generated for a
/// village with nothing in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VillageReading<T> {
    Unloaded,
    Loaded(Vec<T>),
}

/// One point of interest the generated world actually holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VillagePoiReading {
    pub at: [i32; 3],
    pub kind: ScriptSitePoiKind,
}

/// One generated inhabitant, as the placement that spawned it recorded it.
///
/// `claim` is the generator's own identity for the placement and `entity_uuid`
/// is the entity identity it mints; the marker is stored with the chunk, so
/// this is the record of what was generated rather than a reading of wherever a
/// villager happens to stand now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VillageInhabitantReading {
    pub claim: String,
    pub entity_uuid: String,
    pub position: [f64; 3],
    pub age: i32,
}

/// The world facing half of the settlement runtime.
///
/// Core installs one adapter over live world storage; tests install an
/// in-memory fake. Nothing else in this module reaches authoritative terrain.
pub(crate) trait SettlementWorld: Send + Sync {
    /// Bounded terrain reading of one plugin's requested region.
    fn survey(
        &self,
        plugin_id: &str,
        dimension: &str,
        bounds: ScriptSurveyBounds,
    ) -> Result<SurveyReading, ScriptOperationFailure>;
    /// Mint the revision later [`Self::footprint_changed_since`] checks compare
    /// `bounds` against.
    /// An observation is scoped to `bounds`: the world keeps writing chunks that
    /// share space with a footprint - fluid, snow, leaf decay, the structure's
    /// own staged commits - without changing the footprint itself, so a revision
    /// only means anything for the volume it was minted over.
    fn observe_footprint(&self, bounds: ScriptSurveyBounds) -> u64;
    /// Predict and retain the exact post-portion footprint image under a fresh
    /// revision. The caller puts this revision in the same receipt that carries
    /// the portion, so the next advance distinguishes foreign edits from the
    /// structure's own committed blocks without a second storage decision.
    fn predict_structure_portion_revision(
        &self,
        bounds: ScriptSurveyBounds,
        blocks: &[StructureBlockPlacement],
    ) -> Result<u64, ScriptOperationFailure>;
    /// Whether anything inside `bounds` changed after `revision`.
    fn footprint_changed_since(&self, bounds: ScriptSurveyBounds, revision: u64) -> bool;
    /// Whether another plugin's structure or claim overlaps `bounds`.
    fn claims_overlap(&self, plugin_id: &str, bounds: ScriptSurveyBounds) -> bool;
    /// Highest opaque block Y over the footprint columns, or `None` when any
    /// footprint chunk is not loaded.
    ///
    /// One bounded column read per `(x, z)`. This is an occupancy ceiling, not
    /// a terrain surface: an opaque player- or plugin-placed block raises it,
    /// and water or other non-opaque obstacles do not. A structure may only be
    /// placed where nothing rises above its base row, so this backs the fit
    /// check demanded by the contract's "no terrain overwriting" rule.
    fn max_opaque_y(
        &self,
        bounds: ScriptSurveyBounds,
    ) -> Result<Option<i32>, ScriptOperationFailure>;
    /// Canonical reading of one authored container at a world position.
    ///
    /// A warehouse handle resolves to the container its authored seed placed;
    /// this is the only world read that answers whether that position is a
    /// loaded container and, if so, what it holds. Missing and unloaded stay
    /// distinct so a caller never reports an unloaded chunk as an empty one.
    fn container_reading(
        &self,
        position: [i32; 3],
    ) -> Result<ContainerReading, ScriptOperationFailure>;

    /// Materialized containers inside one generated village's bounded box.
    ///
    /// The world enumerates only container block entities it currently holds;
    /// an unavailable covered chunk answers `Unloaded`, never an incomplete
    /// ordinal set. Core sorts these exact positions before selecting one.
    fn village_containers(
        &self,
        bounds: VillageBounds,
    ) -> Result<VillageReading<[i32; 3]>, ScriptOperationFailure>;
    /// Commit one server-owned warehouse deposit against a bound container.
    ///
    /// The caller has already resolved the durable binding, read the loaded
    /// container and planned the move against the actor's canonical inventory;
    /// this is the world half: the owner turn fences both participants,
    /// journals the container's after-image and the encoded plugin receipt in
    /// ONE decision and answers the decision id the caller projects and
    /// acknowledges, or the typed refusal that changed nothing.
    fn commit_warehouse_transfer(
        &self,
        request: WarehouseTransferRequest,
    ) -> Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + '_>>;
    /// Commit one server-owned structure portion and its prepared plugin
    /// receipt in the same world-journal decision.
    ///
    /// The world accepts its after-images only alongside `receipt`, and returns
    /// that decision id only after append. The caller projects the prepared
    /// storage batch afterwards; recovery replays both from the same decision.
    fn commit_structure_portion<'a>(
        &'a self,
        plugin_id: &'a str,
        structure_id: &'a str,
        blocks: &'a [StructureBlockPlacement],
        material_debit: &'a StructureMaterialDebit,
        receipt: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + 'a>>;
    /// The points of interest a generated village's blocks hold.
    ///
    /// Read from the materialized chunks inside `bounds`: a bed, a bell and the
    /// job-site blocks as they actually exist, classified by
    /// [`village_poi_kind`]. A region whose chunks are not generated answers
    /// [`VillageReading::Unloaded`] rather than an empty list.
    ///
    /// [`village_poi_kind`]: super::settlement_village_sites::village_poi_kind
    fn village_pois(
        &self,
        bounds: VillageBounds,
    ) -> Result<VillageReading<VillagePoiReading>, ScriptOperationFailure>;
    /// The inhabitants a generated village placed, from its stored markers.
    ///
    /// The marker is the placement's own record — the claim, the entity
    /// identity it mints and where the generator put it — so a village that is
    /// partly generated reports the part that exists and a village whose chunks
    /// are gone answers [`VillageReading::Unloaded`].
    fn village_inhabitants(
        &self,
        bounds: VillageBounds,
    ) -> Result<VillageReading<VillageInhabitantReading>, ScriptOperationFailure>;
}

/// One bounded terrain survey reading.
#[derive(Debug, Clone)]
pub(crate) struct SurveyReading {
    pub(crate) usable_plots: u32,
    pub(crate) water_columns: u32,
    pub(crate) claimed: bool,
    pub(crate) existing_structures: u32,
    pub(crate) biome_tags: Vec<String>,
    pub(crate) resource_tags: Vec<String>,
    pub(crate) chunk_availability: ScriptChunkAvailability,
    pub(crate) revision: u64,
}

/// One block of a committed structure portion, in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StructureBlockPlacement {
    pub(crate) pos: [i32; 3],
    pub(crate) state: mc_world::BlockStateId,
}

/// One physical warehouse container after-image consumed by a structure
/// portion. The commit derives its floor release from these canonical images;
/// no independent quantity projection can diverge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructureMaterialDebit {
    pub(crate) position: [i32; 3],
    pub(crate) expected: Vec<ItemStack>,
    pub(crate) updated: Vec<ItemStack>,
}

/// Deterministic settlement identity material: selector, catalog, world id, and
/// the terrain generator the world itself generates from.
pub(crate) struct SettlementRuntime {
    selector: SettlementSelector,
    catalog: Arc<BlueprintCatalog>,
    world_identity: String,
    start_cell: [i32; 2],
    ground: Arc<dyn mc_world::ChunkGenerator>,
    /// The generator's vanilla village enumeration, when the world has one.
    ///
    /// A world whose generator places no villages (a worldgen mode without the
    /// `minecraft:villages` set, or a generator that is not the terrain
    /// generator) lists no generated sites and every settlement call keeps
    /// answering about the authored catalog alone.
    village_sites: Option<Arc<dyn VillageSiteGround>>,
}

impl SettlementRuntime {
    /// Built by the worldgen wiring and by the settlement tests.
    ///
    /// `ground` is the startup generator the world generates from, so a site's
    /// base rows come from the terrain the players will actually stand on.
    pub(crate) fn new(
        selector: SettlementSelector,
        catalog: Arc<BlueprintCatalog>,
        world_identity: impl Into<String>,
        start_cell: [i32; 2],
        ground: Arc<dyn mc_world::ChunkGenerator>,
        village_sites: Option<Arc<dyn VillageSiteGround>>,
    ) -> Self {
        Self {
            selector,
            catalog,
            world_identity: world_identity.into(),
            start_cell,
            ground,
            village_sites,
        }
    }

    /// The generated villages whose start chunk lies in one site cell.
    ///
    /// The page walk and this scan share the cell grid, so one page of cells is
    /// one region for the authored selector and the generator alike, and a
    /// world without a village generator answers with no sites rather than an
    /// error.
    pub(crate) fn village_sites_in_cell(&self, cell: [i32; 2]) -> Vec<GeneratedVillageSite> {
        let Some(ground) = self.village_sites.as_ref() else {
            return Vec::new();
        };
        let (min_chunk, max_chunk) = cell_chunk_bounds(cell);
        ground.village_sites_in_region(min_chunk, max_chunk)
    }

    pub(crate) fn ground(&self) -> &Arc<dyn mc_world::ChunkGenerator> {
        &self.ground
    }

    pub(crate) fn selector(&self) -> &SettlementSelector {
        &self.selector
    }

    pub(crate) fn catalog(&self) -> &BlueprintCatalog {
        &self.catalog
    }

    pub(crate) fn world_identity(&self) -> &str {
        &self.world_identity
    }

    pub(crate) fn start_cell(&self) -> [i32; 2] {
        self.start_cell
    }

    /// Resolve every requested approved guard post to its anchor position and
    /// capacity, from the committed site layout the selector derives.
    ///
    /// A post handle is a C2 point-of-interest id, whose leading segment is the
    /// deterministic site id; posts that name no guard point of interest, or
    /// whose site cannot be reconstructed, are omitted (the caller reports
    /// `blocked_route`). The request is bounded: one layout is resolved per
    /// distinct site and no cell other than the named sites is ever scanned.
    pub(super) fn guard_posts(&self, posts: &[String]) -> BTreeMap<String, (Vec3, u16)> {
        let mut wanted: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for post in posts {
            let Some(site_id) = post.split('.').next() else {
                continue;
            };
            wanted.entry(site_id).or_default().push(post.as_str());
        }
        let mut resolved = BTreeMap::new();
        for (site_id, posts) in wanted {
            let Some(cell) = self.selector.cell_from_site_id(site_id) else {
                continue;
            };
            let Some(candidate) = self.selector.candidate(cell) else {
                continue;
            };
            let Ok(layout) = layout_of(self, &candidate) else {
                continue;
            };
            for poi in &layout.pois {
                if poi.kind == PoiKind::Guard && posts.contains(&poi.poi_id.as_str()) {
                    resolved.insert(
                        poi.poi_id.clone(),
                        (
                            Vec3::new(
                                f64::from(poi.at[0]) + 0.5,
                                f64::from(poi.at[1]),
                                f64::from(poi.at[2]) + 0.5,
                            ),
                            poi.capacity,
                        ),
                    );
                }
            }
        }
        resolved
    }
}

impl super::InventoryRuntime {
    /// The site descriptor of one generated vanilla village.
    fn village_site_snapshot(
        &self,
        runtime: &SettlementRuntime,
        storage: &PluginStorage,
        plugin_id: &str,
        village: &GeneratedVillageSite,
    ) -> ScriptSettlementSite {
        village_site_snapshot(
            runtime,
            storage,
            self.settlement_world(),
            plugin_id,
            village,
        )
    }

    /// Execute one settlement request. Queries answer from the deterministic
    /// selector and the durable ledger; every mutation commits its receipt
    /// together with the ledger change it decided.
    pub(crate) async fn execute_settlement_operation(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let ScriptOperation::Settlement { operation } = request.operation() else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        let Some(runtime) = self.settlement_runtime() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        match operation {
            ScriptSettlementOperation::ListSites { cursor, limit } => {
                Ok(self.list_sites(storage, runtime, plugin_id, cursor.as_deref(), *limit))
            }
            ScriptSettlementOperation::QuerySite {
                site_id,
                cursor,
                limit,
            } => Ok(self.query_site(
                storage,
                runtime,
                plugin_id,
                site_id,
                cursor.as_deref(),
                *limit,
            )),
            ScriptSettlementOperation::Status { structure_id } => {
                Ok(self.structure_status(storage, plugin_id, structure_id))
            }
            ScriptSettlementOperation::Survey {
                dimension, bounds, ..
            } => self.survey(storage, plugin_id, dimension, *bounds),
            _ => {
                let Some(operation_id) = operation.operation_id() else {
                    return Ok(rejected(ScriptOperationFailure::InvalidRequest));
                };
                if let Some(outcome) =
                    replay_settlement_operation(storage, plugin_id, request, operation_id)
                {
                    return Ok(outcome);
                }
                match operation {
                    ScriptSettlementOperation::ReserveResidentSite {
                        site_id,
                        poi_id,
                        expected_site_revision,
                        ..
                    } => self.reserve_site_poi(
                        storage,
                        runtime,
                        plugin_id,
                        request,
                        site_id,
                        poi_id,
                        *expected_site_revision,
                    ),
                    ScriptSettlementOperation::ReleaseResidentSite {
                        spawn_site_token, ..
                    } => self.release_resident_site(storage, plugin_id, request, spawn_site_token),
                    ScriptSettlementOperation::PrepareStructure {
                        site_id,
                        blueprint_id,
                        anchor,
                        rotation,
                        survey_token,
                        expected_site_revision,
                        expected_blueprint_hash,
                        ..
                    } => self.prepare_structure(
                        storage,
                        runtime,
                        plugin_id,
                        request,
                        site_id,
                        blueprint_id,
                        *anchor,
                        *rotation,
                        survey_token,
                        *expected_site_revision,
                        expected_blueprint_hash.as_deref(),
                    ),
                    ScriptSettlementOperation::AdvanceStructure {
                        structure_id,
                        stage,
                        reservation_ref,
                        expected_revision,
                        work_units,
                        ..
                    } => {
                        self.advance_structure(
                            storage,
                            runtime,
                            plugin_id,
                            request,
                            structure_id,
                            stage,
                            reservation_ref,
                            *expected_revision,
                            *work_units,
                        )
                        .await
                    }
                    ScriptSettlementOperation::PauseStructure {
                        structure_id,
                        expected_revision,
                        ..
                    } => self.pause_structure(
                        storage,
                        plugin_id,
                        request,
                        structure_id,
                        *expected_revision,
                    ),
                    ScriptSettlementOperation::ResumeStructure {
                        structure_id,
                        expected_revision,
                        ..
                    } => self.resume_structure(
                        storage,
                        plugin_id,
                        request,
                        structure_id,
                        *expected_revision,
                    ),
                    ScriptSettlementOperation::CancelStructure {
                        structure_id,
                        expected_revision,
                        ..
                    } => self.cancel_structure(
                        storage,
                        plugin_id,
                        request,
                        structure_id,
                        *expected_revision,
                    ),
                    ScriptSettlementOperation::BindWarehouse {
                        structure_id,
                        container_id,
                        ..
                    } => self.bind_warehouse(
                        storage,
                        runtime,
                        plugin_id,
                        request,
                        structure_id,
                        *container_id,
                    ),
                    ScriptSettlementOperation::BindVillageWarehouse {
                        site_id,
                        container_id,
                        ..
                    } => self.bind_village_warehouse(
                        storage,
                        runtime,
                        plugin_id,
                        request,
                        site_id,
                        *container_id,
                    ),
                    ScriptSettlementOperation::BindManualWarehouse {
                        survey_token,
                        position,
                        ..
                    } => self.bind_manual_warehouse(
                        storage,
                        runtime,
                        plugin_id,
                        request,
                        survey_token,
                        *position,
                    ),
                    ScriptSettlementOperation::RecognizeBuilding { request: cert, .. } => {
                        self.recognize_building(storage, plugin_id, request, cert)
                    }
                    _ => Ok(rejected(ScriptOperationFailure::InvalidRequest)),
                }
            }
        }
    }

    fn list_sites(
        &self,
        storage: &PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        cursor: Option<&str>,
        limit: u8,
    ) -> ScriptOperationOutcome {
        let start = match cursor {
            Some(cursor) => match parse_cursor(cursor) {
                Some(cell) => cell,
                None => return rejected(ScriptOperationFailure::CursorExpired),
            },
            None => runtime.start_cell(),
        };
        let limit = usize::from(limit).min(MAX_SETTLEMENT_SITE_PAGE);
        let mut sites = Vec::new();
        let mut next = scan_cell(start, limit);
        for cell in runtime.selector().page_cells(start, limit) {
            let mut cell_sites = Vec::new();
            if let Some(candidate) = runtime.selector().candidate(cell) {
                match layout_of(runtime, &candidate)
                    .and_then(|layout| site_snapshot(runtime, storage, &candidate, &layout))
                {
                    Ok(site) => cell_sites.push(site),
                    Err(failure) => return rejected(failure),
                }
            }
            // A cell holds at most the one authored candidate the selector
            // mints and the villages the generator starts inside it; both are
            // sites of the same page, so one cursor covers the region.
            for village in runtime.village_sites_in_cell(cell) {
                cell_sites.push(self.village_site_snapshot(runtime, storage, plugin_id, &village));
            }
            // A page holds at most `MAX_SETTLEMENT_SITE_PAGE` sites, and a cell
            // is taken whole or not at all: taking part of one would either
            // duplicate the sites a later page re-reads from the same cell or
            // drop them silently. The walk therefore stops *before* the cell it
            // cannot fit and leaves the cursor on it, so the next page starts
            // exactly there. A cell's own count is bounded well below the page
            // bound — one authored candidate plus the villages the placement
            // grid starts inside 32 chunks — so the first cell of a page always
            // fits and the walk always progresses.
            if !sites.is_empty() && sites.len() + cell_sites.len() > MAX_SETTLEMENT_SITE_PAGE {
                next = cell;
                break;
            }
            sites.extend(cell_sites);
        }
        let mut page = ScriptSettlementSitePage::new(sites, Some(encode_cursor(next)));
        page.canonicalize();
        settled_outcome(
            storage.revision,
            ScriptSettlementResult::Sites {
                page: Box::new(page),
            },
        )
    }

    fn query_site(
        &self,
        storage: &PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        site_id: &str,
        cursor: Option<&str>,
        limit: u8,
    ) -> ScriptOperationOutcome {
        if let Some(start_chunk) =
            village_site_start_chunk(runtime.world_identity(), VILLAGE_DIMENSION, site_id)
        {
            let cell = [
                start_chunk.0.div_euclid(SITE_CELL_BLOCKS / CHUNK_AXIS),
                start_chunk.1.div_euclid(SITE_CELL_BLOCKS / CHUNK_AXIS),
            ];
            let village = runtime
                .village_sites_in_cell(cell)
                .into_iter()
                .find(|village| village.start_chunk == start_chunk);
            let Some(village) = village else {
                return rejected(ScriptOperationFailure::NotFound);
            };
            let mut site = self.village_site_snapshot(runtime, storage, plugin_id, &village);
            let start = match cursor {
                Some(cursor) => match site
                    .pois
                    .binary_search_by(|poi| poi.poi_id.as_str().cmp(cursor))
                {
                    Ok(index) => index + 1,
                    Err(_) => return rejected(ScriptOperationFailure::CursorExpired),
                },
                None => 0,
            };
            site.pois = site
                .pois
                .into_iter()
                .skip(start)
                .take(usize::from(limit))
                .collect();
            return settled_outcome(
                storage.revision,
                ScriptSettlementResult::Site {
                    site: Box::new(site),
                },
            );
        }
        let Some(cell) = runtime.selector().cell_from_site_id(site_id) else {
            return rejected(ScriptOperationFailure::NotFound);
        };
        let Some(candidate) = runtime.selector().candidate(cell) else {
            return rejected(ScriptOperationFailure::NotFound);
        };
        let layout = match layout_of(runtime, &candidate) {
            Ok(layout) => layout,
            Err(failure) => return rejected(failure),
        };
        let mut site = match site_snapshot(runtime, storage, &candidate, &layout) {
            Ok(site) => site,
            Err(failure) => return rejected(failure),
        };
        // The page is a contiguous slice of the canonical point-of-interest
        // order; the caller pages on by naming the last id it received.
        let start = match cursor {
            Some(cursor) => {
                match site
                    .pois
                    .binary_search_by(|poi| poi.poi_id.as_str().cmp(cursor))
                {
                    Ok(index) => index + 1,
                    Err(_) => return rejected(ScriptOperationFailure::CursorExpired),
                }
            }
            None => 0,
        };
        site.pois = site
            .pois
            .into_iter()
            .skip(start)
            .take(usize::from(limit))
            .collect();
        settled_outcome(
            storage.revision,
            ScriptSettlementResult::Site {
                site: Box::new(site),
            },
        )
    }

    /// Read one durable structure or certificate the caller owns.
    ///
    /// Certificates live in `buildings`, not `structures`, but share the same
    /// read-only status query: a certificate id answers its immutable snapshot
    /// so specialization can validate certificate-backed workplaces through the
    /// same ledger authority it already uses for structures.
    fn structure_status(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        structure_id: &str,
    ) -> ScriptOperationOutcome {
        if let Some(structure) = storage.settlements().structure(structure_id) {
            if structure.plugin_id != plugin_id {
                return rejected(ScriptOperationFailure::Forbidden);
            }
            return settled_outcome(
                structure.revision,
                ScriptSettlementResult::Structure {
                    structure: Box::new(structure_snapshot(storage, structure)),
                },
            );
        }
        let Some(certificate) = storage.settlements().building(structure_id) else {
            return rejected(ScriptOperationFailure::NotFound);
        };
        if certificate.plugin_id != plugin_id {
            return rejected(ScriptOperationFailure::Forbidden);
        }
        settled_outcome(
            certificate.revision,
            ScriptSettlementResult::Building {
                certificate: Box::new(certificate.snapshot()),
            },
        )
    }

    /// Resolve one bound warehouse container, or the typed failure that keeps a
    /// plugin from reading or writing a foreign, inactive, unloaded or
    /// non-container handle.
    ///
    /// The opaque handle resolves through the durable binding; the binding owner
    /// must be the caller; the bound structure must still be active; and the
    /// authored container must still be a loaded container at its placed
    /// position. Every one of those closes with its own family rather than an
    /// empty container, so a plugin can never mistake a refusal for one. The
    /// reading and the writing endpoint resolve through this one helper.
    pub(super) fn resolve_warehouse_container(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        handle: &str,
    ) -> Result<ResolvedWarehouseContainer, ScriptOperationFailure> {
        let Some(runtime) = self.settlement_runtime() else {
            return Err(ScriptOperationFailure::RuntimeUnavailable);
        };
        let Some(world) = self.settlement_world() else {
            return Err(ScriptOperationFailure::RuntimeUnavailable);
        };
        let Some(binding) = storage.settlements().warehouse(handle) else {
            return Err(ScriptOperationFailure::NotFound);
        };
        if binding.plugin_id != plugin_id {
            return Err(ScriptOperationFailure::Forbidden);
        }
        let position = match &binding.source {
            DurableWarehouseSource::Village {
                site_id, position, ..
            } => {
                let village = generated_village(runtime, site_id)?;
                let bounds = village_bounds(&village)?;
                if !position_in_bounds(*position, bounds) {
                    return Err(ScriptOperationFailure::NotFound);
                }
                let containers = match world.village_containers(bounds)? {
                    VillageReading::Loaded(containers) => containers,
                    VillageReading::Unloaded => return Err(ScriptOperationFailure::Unloaded),
                };
                if !containers.contains(position) {
                    return Err(ScriptOperationFailure::NotFound);
                }
                *position
            }
            DurableWarehouseSource::Manual { position } => *position,
            DurableWarehouseSource::Authored {
                structure_id,
                container_id,
            } => {
                let Some(structure) = storage.settlements().structure(structure_id).cloned() else {
                    return Err(ScriptOperationFailure::NotFound);
                };
                if !structure.is_placed() {
                    return Err(ScriptOperationFailure::Blocked);
                }
                warehouse_container_position(runtime, &structure, *container_id)?
            }
        };
        let items = match world.container_reading(position)? {
            ContainerReading::Loaded(items) => items,
            ContainerReading::Unloaded => return Err(ScriptOperationFailure::Unloaded),
            ContainerReading::Missing => return Err(ScriptOperationFailure::NotFound),
        };
        Ok(ResolvedWarehouseContainer {
            revision: binding.revision,
            position,
            items,
        })
    }

    /// Resolve the durable position of a source which has already reported as
    /// unloaded. Reservation floors retain that exact position until the source
    /// can be read again, rather than briefly permitting a player drawdown.
    pub(super) fn unloaded_warehouse_position(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        handle: &str,
    ) -> Result<[i32; 3], ScriptOperationFailure> {
        let Some(runtime) = self.settlement_runtime() else {
            return Err(ScriptOperationFailure::RuntimeUnavailable);
        };
        if self.settlement_world().is_none() {
            return Err(ScriptOperationFailure::RuntimeUnavailable);
        }
        let Some(binding) = storage.settlements().warehouse(handle) else {
            return Err(ScriptOperationFailure::NotFound);
        };
        if binding.plugin_id != plugin_id {
            return Err(ScriptOperationFailure::Forbidden);
        }
        match &binding.source {
            DurableWarehouseSource::Village {
                site_id, position, ..
            } => {
                let village = generated_village(runtime, site_id)?;
                let bounds = village_bounds(&village)?;
                if !position_in_bounds(*position, bounds) {
                    return Err(ScriptOperationFailure::NotFound);
                }
                Ok(*position)
            }
            DurableWarehouseSource::Manual { position } => Ok(*position),
            DurableWarehouseSource::Authored {
                structure_id,
                container_id,
            } => {
                let Some(structure) = storage.settlements().structure(structure_id) else {
                    return Err(ScriptOperationFailure::NotFound);
                };
                if !structure.is_placed() {
                    return Err(ScriptOperationFailure::Blocked);
                }
                warehouse_container_position(runtime, structure, *container_id)
            }
        }
    }

    /// Canonical snapshot of one bound warehouse container.
    pub(super) fn warehouse_inventory_snapshot(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        handle: &str,
        expected_revision: Option<u64>,
    ) -> ScriptOperationOutcome {
        let container = match self.resolve_warehouse_container(storage, plugin_id, handle) {
            Ok(container) => container,
            Err(failure) => return rejected(failure),
        };
        if expected_revision.is_some_and(|expected| expected != container.revision) {
            return rejected(ScriptOperationFailure::StaleRevision);
        }
        self.sessions().query_warehouse_inventory(
            &ScriptInventoryEndpoint::Warehouse {
                handle: handle.to_owned(),
            },
            container.revision,
            &container.items,
            self.items(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn reserve_site_poi(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        site_id: &str,
        poi_id: &str,
        expected_site_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let (generation_id, position) = if let Some(cell) =
            runtime.selector().cell_from_site_id(site_id)
        {
            let Some(candidate) = runtime.selector().candidate(cell) else {
                return Ok(rejected(ScriptOperationFailure::NotFound));
            };
            let layout = match layout_of(runtime, &candidate) {
                Ok(layout) => layout,
                Err(failure) => return Ok(rejected(failure)),
            };
            let Some((slot, poi)) = layout
                .pois
                .iter()
                .enumerate()
                .find(|(_, poi)| poi.poi_id == poi_id)
            else {
                return Ok(rejected(ScriptOperationFailure::NotFound));
            };
            if poi.kind != PoiKind::Home {
                return Ok(rejected(ScriptOperationFailure::Blocked));
            }
            let generation_id =
                match resident_generation_id(runtime.world_identity(), site_id, slot as u32) {
                    Ok(generation_id) => generation_id,
                    Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
                };
            (
                generation_id,
                Vec3::new(
                    f64::from(poi.at[0]),
                    f64::from(poi.at[1]),
                    f64::from(poi.at[2]),
                ),
            )
        } else {
            let village = match generated_village(runtime, site_id) {
                Ok(village) => village,
                Err(failure) => return Ok(rejected(failure)),
            };
            let site = self.village_site_snapshot(runtime, storage, plugin_id, &village);
            if !site.contents_known {
                return Ok(rejected(ScriptOperationFailure::Unloaded));
            }
            let Some(poi) = site.pois.iter().find(|poi| poi.poi_id == poi_id) else {
                return Ok(rejected(ScriptOperationFailure::NotFound));
            };
            if poi.kind != ScriptSitePoiKind::Home || poi.state != ScriptSitePoiState::Free {
                return Ok(rejected(ScriptOperationFailure::Blocked));
            }
            let generation_id =
                match resident_generation_id_for_poi(runtime.world_identity(), site_id, poi_id) {
                    Ok(generation_id) => generation_id,
                    Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
                };
            (
                generation_id,
                Vec3::new(
                    f64::from(poi.at[0]) + 0.5,
                    f64::from(poi.at[1]) + 1.0,
                    f64::from(poi.at[2]) + 0.5,
                ),
            )
        };
        let existing = storage.settlements().site(site_id).cloned();
        let site_revision = existing.as_ref().map_or(0, |site| site.revision);
        if expected_site_revision != site_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        if let Some(reservation) = existing.as_ref().and_then(|site| site.reservation(poi_id))
            && !reservation.released
        {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let token = match super::InventoryRuntime::reserve_resident_site(
            storage,
            plugin_id,
            &generation_id,
            position,
        ) {
            Ok(token) => token,
            Err(failure) => return Ok(rejected(failure)),
        };
        let mut site = existing.unwrap_or_else(|| DurableSiteState::new(site_id.to_owned()));
        site.pois.insert(
            poi_id.to_owned(),
            DurablePoiReservation {
                token: token.clone(),
                plugin_id: plugin_id.to_owned(),
                consumed: false,
                released: false,
            },
        );
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::ResidentSite {
                reservation: Box::new(ScriptResidentSiteReservation::new(
                    site_id.to_owned(),
                    poi_id.to_owned(),
                    token,
                    0,
                )),
            }),
            vec![DurableSettlementChange::Site {
                site: Box::new(site),
            }],
        )
    }

    fn release_resident_site(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        spawn_site_token: &str,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some((site, poi_id)) = storage
            .settlements()
            .reservation_by_token(spawn_site_token)
            .map(|(site, poi_id)| (site.clone(), poi_id.to_owned()))
        else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        let mut site = site;
        let Some(reservation) = site.pois.get_mut(&poi_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if reservation.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if reservation.released {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        }
        // A consumed spawn site can never be handed back: the resident ledger
        // already bound this home to a spawned inhabitant, so releasing it would
        // make an occupied home reservable again. Refuse with the same family
        // the reserve path uses, before any state change is committed.
        let resident_site = storage.residents().site(&reservation.token).cloned();
        let consumed = reservation.consumed
            || resident_site.is_some_and(|site| {
                site.consumed
                    || storage
                        .residents()
                        .record_by_generation(&site.generation_id)
                        .is_some()
            });
        if consumed {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        // Mirror the resident ledger's observation durably: a slot whose
        // inhabitant was already spawned stays recorded as such even after the
        // settlement-level reservation is handed back.
        reservation.consumed |= storage
            .residents()
            .site(&reservation.token)
            .is_some_and(|site| site.consumed);
        reservation.released = true;
        let site_id = site.site_id.clone();
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::ResidentSite {
                reservation: Box::new(ScriptResidentSiteReservation::new(
                    site_id,
                    poi_id,
                    spawn_site_token.to_owned(),
                    0,
                )),
            }),
            vec![DurableSettlementChange::Site {
                site: Box::new(site),
            }],
        )
    }
}

fn commit_settlement(
    storage: &mut PluginStorage,
    plugin_id: &str,
    request: &ScriptOperationRequest,
    payload: ScriptOperationPayload,
    changes: Vec<DurableSettlementChange>,
) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
    let batch =
        match storage.prepare_settlement_operation_batch(plugin_id, request, payload, changes)? {
            ScriptStoragePrepareOutcome::Prepared(batch) => batch,
            ScriptStoragePrepareOutcome::Rejected => {
                return Ok(rejected(ScriptOperationFailure::Capacity));
            }
        };
    storage.commit_batch(batch)?;
    let operation_id = request
        .operation_id()
        .expect("settlement mutation decision identity");
    Ok(storage
        .operation_receipt(plugin_id, operation_id)
        .expect("committed settlement receipt remains installed")
        .outcome
        .clone())
}

fn replay_settlement_operation(
    storage: &PluginStorage,
    plugin_id: &str,
    request: &ScriptOperationRequest,
    operation_id: &str,
) -> Option<ScriptOperationOutcome> {
    let fingerprint = owned_inventory_fingerprint(request.operation());
    storage
        .operation_receipt(plugin_id, operation_id)
        .map(|receipt| {
            if receipt.fingerprint == fingerprint {
                receipt.outcome.clone()
            } else {
                ScriptOperationOutcome::rejected(ScriptOperationFailure::OperationConflict)
            }
        })
}

/// One deterministic site layout; a catalog that cannot lay the variant out is a
/// core configuration failure, never a silent downgrade.
fn layout_of(
    runtime: &SettlementRuntime,
    candidate: &SiteCandidate,
) -> Result<SiteLayout, ScriptOperationFailure> {
    // The layout resolves its own base rows through the same generator the
    // world generates from; a generator that cannot answer a column fails the
    // layout closed rather than inventing a level.
    let ground = runtime.ground();
    runtime
        .selector()
        .layout(candidate, runtime.catalog(), &|x, z| {
            ground.surface_height(x, z)
        })
        .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)
}

/// One site snapshot. The generator's variant footprint is reported verbatim: a
/// site is reserved territory, not a building, so it is never squeezed into the
/// per-building blueprint bound.
/// The site descriptor of one generated vanilla village.
///
/// The identity, the region and the pieces come from the generator's own plan;
/// the points of interest and the inhabitants come from the chunks the world
/// actually generated. Those are the two authorities the settlement lane
/// already has — the plan decides what the world will hold, the world decides
/// what it does hold — so a village whose chunks are not generated is described
/// by its identity and region with `contents_known` false, never as a village
/// that carries nothing (`ACC-05`).
///
/// A placed `feature_pool_element` places no template and is not a building; the
/// pieces that do place one are reported with the template's id, its world
/// origin and its quarter turn.
fn village_site_snapshot(
    runtime: &SettlementRuntime,
    storage: &PluginStorage,
    world: Option<&dyn SettlementWorld>,
    plugin_id: &str,
    village: &GeneratedVillageSite,
) -> ScriptSettlementSite {
    let bounds = village_bounds(village).ok();
    let pois_reading = world
        .zip(bounds)
        .map(|(world, bounds)| world.village_pois(bounds));
    let inhabitants_reading = world
        .zip(bounds)
        .map(|(world, bounds)| world.village_inhabitants(bounds));
    let (pois, inhabitants, contents_known) = match (pois_reading, inhabitants_reading) {
        (Some(Ok(VillageReading::Loaded(pois))), Some(Ok(VillageReading::Loaded(inhabitants)))) => {
            (pois, inhabitants, true)
        }
        // A world that is absent or cannot see the village's chunks leaves the
        // contents unknown: nothing is reported as an empty village.
        _ => (Vec::new(), Vec::new(), false),
    };
    let held = resident_owned_pois(storage, plugin_id);
    let site_id = village_site_id(
        runtime.world_identity(),
        VILLAGE_DIMENSION,
        village.start_chunk,
    );
    let ledger = storage.settlements().site(&site_id);
    let mut reported_pois = Vec::with_capacity(pois.len());
    for poi in &pois {
        let poi_id = village_poi_id(poi.at);
        let state = if held.contains(&poi_id) {
            ScriptSitePoiState::Occupied
        } else {
            poi_state(storage, ledger, &poi_id)
        };
        reported_pois.push(ScriptSettlementPoi::new(
            poi_id,
            poi.kind,
            poi.at,
            village_poi_capacity(poi.kind),
            state,
        ));
    }
    let buildings = village
        .pieces
        .iter()
        .filter_map(|piece| {
            piece.template.as_ref().map(|template| {
                ScriptSettlementBuilding::new(
                    template.clone(),
                    [piece.position.x, piece.position.y, piece.position.z],
                    // The plan carries quarter turns; the descriptor's rotation
                    // is degrees, the unit every authored placement uses.
                    piece.rotation * 90,
                )
            })
        })
        .collect();
    let mut site = ScriptSettlementSite::new(
        site_id,
        ScriptSiteProvenance::VanillaVillage,
        // A generated village is a village. How large it is in the settlement
        // lane's own terms is the owner's measured `size_class`, not a
        // generation fact.
        ScriptSiteVariant::Village,
        // The descriptor is regenerated from world state; only its reservations
        // are durable, and their revision fences future reserve requests.
        ledger.map_or(0, |site| site.revision),
        contents_known,
        [village.min.x, village.min.y, village.min.z],
        [
            village.max.x - village.min.x + 1,
            village.max.y - village.min.y + 1,
            village.max.z - village.min.z + 1,
        ],
        buildings,
        reported_pois,
        inhabitants
            .into_iter()
            .map(|inhabitant| inhabitant.entity_uuid)
            .collect(),
    );
    site.canonicalize();
    site
}

/// The point-of-interest id of one generated village block.
///
/// The handle names the block, so two sites never share one and an owner that
/// assigns it to a resident reads the same string back on the next query.
fn village_poi_id(at: [i32; 3]) -> String {
    format!("village_poi_{}_{}_{}", at[0], at[1], at[2])
}

/// Every point-of-interest handle this plugin's residents hold.
///
/// A generated village's occupancy is what its owner assigned: the resident
/// ledger records the handles a resident sleeps, works and gathers at, and the
/// descriptor mints the same handles, so the two agree without a second
/// occupancy ledger.
fn resident_owned_pois(storage: &PluginStorage, plugin_id: &str) -> BTreeSet<String> {
    let mut held = BTreeSet::new();
    for handle in storage.residents().handles_for_plugin(plugin_id) {
        let Some(record) = storage.residents().record(&handle) else {
            continue;
        };
        for poi in [
            record.pois.home.as_ref(),
            record.pois.work.as_ref(),
            record.pois.meeting.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            held.insert(poi.clone());
        }
    }
    held
}

fn site_snapshot(
    runtime: &SettlementRuntime,
    storage: &PluginStorage,
    candidate: &SiteCandidate,
    layout: &SiteLayout,
) -> Result<ScriptSettlementSite, ScriptOperationFailure> {
    let ledger = storage.settlements().site(&candidate.site_id);
    let homes = layout
        .pois
        .iter()
        .filter(|poi| poi.kind == PoiKind::Home)
        .count();
    if homes > MAX_SETTLEMENT_RESIDENTS {
        return Err(ScriptOperationFailure::Blocked);
    }
    let mut pois = Vec::with_capacity(layout.pois.len());
    let mut inhabitant_generation_ids = Vec::with_capacity(homes);
    for (slot, poi) in layout.pois.iter().enumerate() {
        if poi.kind == PoiKind::Home {
            let generation =
                resident_generation_id(runtime.world_identity(), &candidate.site_id, slot as u32)
                    .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            inhabitant_generation_ids.push(generation);
        }
        pois.push(ScriptSettlementPoi::new(
            poi.poi_id.clone(),
            poi_kind(poi.kind),
            poi.at,
            poi.capacity,
            poi_state(storage, ledger, &poi.poi_id),
        ));
    }
    let mut site = ScriptSettlementSite::new(
        candidate.site_id.clone(),
        ScriptSiteProvenance::Authored,
        site_variant(candidate.variant),
        ledger.map_or(0, |site| site.revision),
        // An authored site's points of interest and inhabitants come from the
        // catalog that laid it out, so they are its contents as soon as it is
        // described.
        true,
        layout.candidate.origin,
        layout.candidate.size,
        layout
            .placements
            .iter()
            .map(|placement| {
                ScriptSettlementBuilding::new(
                    placement.blueprint_id.clone(),
                    placement.origin,
                    placement.rotation,
                )
            })
            .collect(),
        pois,
        inhabitant_generation_ids,
    );
    site.canonicalize();
    Ok(site)
}

fn poi_state(
    storage: &PluginStorage,
    ledger: Option<&DurableSiteState>,
    poi_id: &str,
) -> ScriptSitePoiState {
    let Some(reservation) = ledger.and_then(|site| site.reservation(poi_id)) else {
        return ScriptSitePoiState::Free;
    };
    if reservation.released {
        return ScriptSitePoiState::Free;
    }
    let spawned = storage
        .residents()
        .site(&reservation.token)
        .is_some_and(|site| site.consumed);
    if reservation.consumed || spawned {
        ScriptSitePoiState::Occupied
    } else {
        ScriptSitePoiState::Reserved
    }
}

fn site_variant(variant: SiteVariant) -> ScriptSiteVariant {
    match variant {
        SiteVariant::Hamlet => ScriptSiteVariant::Hamlet,
        SiteVariant::Village => ScriptSiteVariant::Village,
        SiteVariant::Town => ScriptSiteVariant::Town,
    }
}

fn poi_kind(kind: PoiKind) -> ScriptSitePoiKind {
    match kind {
        PoiKind::Home => ScriptSitePoiKind::Home,
        PoiKind::Work => ScriptSitePoiKind::Work,
        PoiKind::Meeting => ScriptSitePoiKind::Meeting,
        PoiKind::Guard => ScriptSitePoiKind::Guard,
    }
}

/// Resolve a village site id through the same generator and start-chunk
/// authentication the site query path uses.
fn generated_village(
    runtime: &SettlementRuntime,
    site_id: &str,
) -> Result<GeneratedVillageSite, ScriptOperationFailure> {
    let start_chunk =
        village_site_start_chunk(runtime.world_identity(), VILLAGE_DIMENSION, site_id)
            .ok_or(ScriptOperationFailure::NotFound)?;
    let cell_axis = SITE_CELL_BLOCKS / CHUNK_AXIS;
    let cell = [
        start_chunk.0.div_euclid(cell_axis),
        start_chunk.1.div_euclid(cell_axis),
    ];
    let village = runtime
        .village_sites_in_cell(cell)
        .into_iter()
        .find(|village| village.start_chunk == start_chunk)
        .ok_or(ScriptOperationFailure::NotFound)?;
    (village_site_id(
        runtime.world_identity(),
        VILLAGE_DIMENSION,
        village.start_chunk,
    ) == site_id)
        .then_some(village)
        .ok_or(ScriptOperationFailure::NotFound)
}

pub(super) fn village_bounds(
    village: &GeneratedVillageSite,
) -> Result<VillageBounds, ScriptOperationFailure> {
    let min = [village.min.x, village.min.y, village.min.z];
    let max = [village.max.x, village.max.y, village.max.z];
    let bounded = min.iter().zip(max).all(|(min, max)| {
        let extent = i64::from(max) - i64::from(*min) + 1;
        (1..=i64::from(MAX_SETTLEMENT_SITE_AXIS)).contains(&extent)
    });
    if !bounded {
        return Err(ScriptOperationFailure::RuntimeUnavailable);
    }
    Ok(VillageBounds { min, max })
}

fn position_in_bounds(position: [i32; 3], bounds: VillageBounds) -> bool {
    (bounds.min[0]..=bounds.max[0]).contains(&position[0])
        && (bounds.min[1]..=bounds.max[1]).contains(&position[1])
        && (bounds.min[2]..=bounds.max[2]).contains(&position[2])
}

fn bounds_of(origin: [i32; 3], size: [i32; 3]) -> ScriptSurveyBounds {
    // A structure footprint is bounded by the per-building blueprint axis, which
    // is well inside one survey bounds axis, so the constructor cannot fail.
    ScriptSurveyBounds::new(
        origin,
        [
            origin[0].saturating_add(size[0] - 1),
            origin[1].saturating_add(size[1] - 1),
            origin[2].saturating_add(size[2] - 1),
        ],
    )
    .expect("a bounded structure footprint fits one survey")
}

/// Owner-scoped, bounded survey token. The storage transaction is mixed in, so
/// two identical surveys never share a token.
fn survey_token(
    plugin_id: &str,
    transaction_id: u64,
    dimension: &str,
    bounds: ScriptSurveyBounds,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SETTLEMENT_DOMAIN);
    hasher.update(b"survey");
    hasher.update((plugin_id.len() as u32).to_le_bytes());
    hasher.update(plugin_id.as_bytes());
    hasher.update(transaction_id.to_le_bytes());
    hasher.update((dimension.len() as u32).to_le_bytes());
    hasher.update(dimension.as_bytes());
    for value in bounds.min.iter().chain(bounds.max.iter()) {
        hasher.update(value.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Durable certificate identity: unique per committed transaction, stable for
/// the lifetime of the record.
fn mint_building_certificate_id(
    plugin_id: &str,
    transaction_id: u64,
    footprint: &ScriptSurveyBounds,
    purpose: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SETTLEMENT_DOMAIN);
    hasher.update(b"building");
    hasher.update((plugin_id.len() as u32).to_le_bytes());
    hasher.update(plugin_id.as_bytes());
    hasher.update(transaction_id.to_le_bytes());
    for value in footprint.min.iter().chain(footprint.max.iter()) {
        hasher.update(value.to_le_bytes());
    }
    hasher.update((purpose.len() as u32).to_le_bytes());
    hasher.update(purpose.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn encode_cursor(cell: [i32; 2]) -> String {
    format!("{}:{}", cell[0], cell[1])
}

fn parse_cursor(cursor: &str) -> Option<[i32; 2]> {
    let (x, z) = cursor.split_once(':')?;
    if z.contains(':') {
        return None;
    }
    let cell = [x.parse::<i32>().ok()?, z.parse::<i32>().ok()?];
    // Discovery walks forward from the cursor; refuse a cursor whose scan would
    // overflow rather than let the generator panic.
    if cell[0] > i32::MAX - MAX_SETTLEMENT_SITE_PAGE as i32
        || cell[1] > i32::MAX - MAX_SETTLEMENT_SITE_PAGE as i32
    {
        return None;
    }
    Some(cell)
}

/// The next cell in the selector's row-major scan order, matching
/// [`mc_worldgen::SettlementSelector::discover`].
fn scan_cell(start: [i32; 2], index: usize) -> [i32; 2] {
    [
        start[0] + (index % MAX_SETTLEMENT_SITE_PAGE) as i32,
        start[1] + (index / MAX_SETTLEMENT_SITE_PAGE) as i32,
    ]
}
