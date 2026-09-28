//! Building certificates and the recognition of a player-built volume.
//!
//! Split out of `settlement.rs` by CP-REFACTOR-040. The ledger authority, the
//! shared atomic `apply` and the world adapters stay in the parent module.

use super::*;

/// One durable, owner-bound building certificate: the exact normalized request
/// core validated against the live world, keyed by its opaque certificate id.
///
/// Capacity, workplaces, and capabilities are never stored twice here. The
/// certificate records the validated interaction points; the resident work
/// path derives the live capability from those same points at execution, so a
/// certified player building and a blueprinted structure share one downstream
/// interpretation. A repeated recognition of the same normalized volume
/// answers the original certificate and revision without a second ledger
/// entry, so re-recognition never mints extra capacity or workplaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::script::storage) struct DurableBuildingCertificate {
    pub(super) certificate_id: String,
    pub(super) plugin_id: String,
    pub(super) footprint: ScriptSurveyBounds,
    purpose: String,
    interaction_points: Vec<mc_script::ScriptBuildingInteractionPoint>,
    world_revision: u64,
    pub(super) revision: u64,
}

impl DurableBuildingCertificate {
    pub(super) fn snapshot(&self) -> ScriptBuildingCertificate {
        ScriptBuildingCertificate::new(
            self.certificate_id.clone(),
            self.footprint,
            self.purpose.clone(),
            self.interaction_points.clone(),
            self.world_revision,
            self.revision,
        )
    }
}

impl SettlementLedger {
    /// The certificate covering one normalized footprint, if any plugin holds it.
    fn building_for_footprint(
        &self,
        footprint: &ScriptSurveyBounds,
    ) -> Option<&DurableBuildingCertificate> {
        self.buildings
            .values()
            .find(|certificate| certificate.footprint == *footprint)
    }
}

pub(super) fn validate_building_certificate(
    certificate: &DurableBuildingCertificate,
) -> Result<(), PluginStorageStartError> {
    validate_identifier(&certificate.plugin_id, 128)?;
    certificate
        .snapshot()
        .validate()
        .map_err(|_| PluginStorageStartError::Malformed("settlement building certificate"))?;
    Ok(())
}

impl crate::script::storage::InventoryRuntime {
    /// Recognize one player-built volume as a functional building.
    ///
    /// The survey token is the fence, as in `bind_manual_warehouse`: the
    /// caller's own live token in the overworld, unexpired and unchanged
    /// since. Every interaction point is checked against the live world
    /// through the authorities this lane already owns: entrances through
    /// `ResidentWorld::standable`, workstations through the same station
    /// path the resident `Craft` step reads, storage through
    /// `SettlementWorld::container_reading`, and home POIs through the live
    /// village bed reading. The durable result is the certificate itself,
    /// keyed by its opaque id; a repeated recognition of the same normalized
    /// footprint answers the original certificate and revision, never extra
    /// capacity or workplaces.
    pub(super) fn recognize_building(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        certificate: &mc_script::ScriptBuildingCertificateRequest,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let Some(resident) = self.resident_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let Some(token) = storage.settlements().survey(&certificate.survey_token) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if token.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if token.dimension != VILLAGE_DIMENSION {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        }
        if certificate.expected_world_revision != token.world_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        if !(token.bounds.min[0] <= certificate.footprint.min[0]
            && certificate.footprint.max[0] <= token.bounds.max[0]
            && token.bounds.min[1] <= certificate.footprint.min[1]
            && certificate.footprint.max[1] <= token.bounds.max[1]
            && token.bounds.min[2] <= certificate.footprint.min[2]
            && certificate.footprint.max[2] <= token.bounds.max[2])
        {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        if world.claims_overlap(plugin_id, certificate.footprint) {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        // Availability before freshness: the POI read covers the whole
        // footprint, so an unloaded region wins over a stale fence and the
        // caller retries when the chunk loads instead of discarding it.
        let village_bounds = VillageBounds {
            min: certificate.footprint.min,
            max: certificate.footprint.max,
        };
        let live_pois = match world.village_pois(village_bounds) {
            Ok(VillageReading::Loaded(pois)) => pois,
            Ok(VillageReading::Unloaded) => {
                return Ok(rejected(ScriptOperationFailure::Unloaded));
            }
            Err(failure) => return Ok(rejected(failure)),
        };
        for point in &certificate.interaction_points {
            match point {
                mc_script::ScriptBuildingInteractionPoint::Storage { at } => {
                    match world.container_reading(*at) {
                        Ok(ContainerReading::Loaded(_)) | Ok(ContainerReading::Missing) => {}
                        Ok(ContainerReading::Unloaded) => {
                            return Ok(rejected(ScriptOperationFailure::Unloaded));
                        }
                        Err(failure) => return Ok(rejected(failure)),
                    }
                }
                mc_script::ScriptBuildingInteractionPoint::Entrance { at } => {
                    if resident.standable(&token.dimension, *at).is_none() {
                        return Ok(rejected(ScriptOperationFailure::Unloaded));
                    }
                }
                mc_script::ScriptBuildingInteractionPoint::Workstation { at, .. } => {
                    if resident.block(&token.dimension, *at).is_none() {
                        return Ok(rejected(ScriptOperationFailure::Unloaded));
                    }
                }
                mc_script::ScriptBuildingInteractionPoint::Poi { .. } => {}
                _ => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
            }
        }
        if token.expires_transaction < storage.revision
            || world.footprint_changed_since(token.bounds, token.world_revision)
        {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let mut entrances = Vec::new();
        for point in &certificate.interaction_points {
            match point {
                mc_script::ScriptBuildingInteractionPoint::Entrance { at } => {
                    if resident.standable(&token.dimension, *at) != Some(true) {
                        return Ok(rejected(ScriptOperationFailure::Blocked));
                    }
                    entrances.push(*at);
                }
                mc_script::ScriptBuildingInteractionPoint::Poi {
                    poi_kind,
                    at,
                    capacity,
                } => {
                    let Some(live) = live_pois
                        .iter()
                        .find(|live| live.at == *at && live.kind == *poi_kind)
                    else {
                        return Ok(rejected(ScriptOperationFailure::Blocked));
                    };
                    if *capacity > village_poi_capacity(live.kind) {
                        return Ok(rejected(ScriptOperationFailure::Blocked));
                    }
                }
                mc_script::ScriptBuildingInteractionPoint::Workstation {
                    at,
                    capability,
                    workplaces,
                } => {
                    if *workplaces != 1 {
                        return Ok(rejected(ScriptOperationFailure::Blocked));
                    }
                    let Some(block) = resident.block(&token.dimension, *at) else {
                        return Ok(rejected(ScriptOperationFailure::Unloaded));
                    };
                    let Some(path) = capability.strip_prefix("minecraft:") else {
                        return Ok(rejected(ScriptOperationFailure::Blocked));
                    };
                    if block.path != path || (path == "campfire" && !block.is_lit_campfire) {
                        return Ok(rejected(ScriptOperationFailure::Blocked));
                    }
                }
                mc_script::ScriptBuildingInteractionPoint::Storage { at } => {
                    match world.container_reading(*at) {
                        Ok(ContainerReading::Loaded(_)) => {}
                        Ok(ContainerReading::Missing) => {
                            return Ok(rejected(ScriptOperationFailure::NotFound));
                        }
                        Ok(ContainerReading::Unloaded) => {
                            return Ok(rejected(ScriptOperationFailure::Unloaded));
                        }
                        Err(failure) => return Ok(rejected(failure)),
                    }
                }
                _ => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
            }
        }
        if entrances.is_empty() {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        // Real availability, not just a standable marker: every station,
        // container, and POI must be reachable on foot from an entrance
        // through the same bounded walk the resident path uses. The point
        // itself holds a block, so the walk runs from the entrance to a
        // standable horizontal neighbour of the point. One pass: `reachable`
        // on any open walk, `saw_unloaded` on any unreadable cell; a closed
        // walk is loaded but blocked.
        for point in &certificate.interaction_points {
            let target = match point {
                mc_script::ScriptBuildingInteractionPoint::Workstation { at, .. }
                | mc_script::ScriptBuildingInteractionPoint::Storage { at }
                | mc_script::ScriptBuildingInteractionPoint::Poi { at, .. } => *at,
                _ => continue,
            };
            let neighbours = [
                [target[0] + 1, target[1], target[2]],
                [target[0] - 1, target[1], target[2]],
                [target[0], target[1], target[2] + 1],
                [target[0], target[1], target[2] - 1],
            ];
            let mut reachable = false;
            let mut saw_unloaded = false;
            for entrance in &entrances {
                for neighbour in neighbours {
                    match resident.standable(&token.dimension, neighbour) {
                        Some(true) => {}
                        Some(false) => continue,
                        None => {
                            saw_unloaded = true;
                            continue;
                        }
                    }
                    match resident.route_open(&token.dimension, *entrance, neighbour) {
                        Some(true) => {
                            reachable = true;
                            break;
                        }
                        Some(false) => {}
                        None => {
                            saw_unloaded = true;
                        }
                    }
                }
                if reachable {
                    break;
                }
            }
            if reachable {
                continue;
            }
            if saw_unloaded {
                return Ok(rejected(ScriptOperationFailure::Unloaded));
            }
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let mut normalized = certificate.clone();
        normalized.canonicalize();
        if let Some(existing) = storage
            .settlements()
            .building_for_footprint(&normalized.footprint)
        {
            if existing.plugin_id != plugin_id {
                return Ok(rejected(ScriptOperationFailure::Forbidden));
            }
            return commit_settlement(
                storage,
                plugin_id,
                request,
                settlement_payload(ScriptSettlementResult::Building {
                    certificate: Box::new(existing.snapshot()),
                }),
                Vec::new(),
            );
        }
        let transaction_id = storage
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        let certificate_id = mint_building_certificate_id(
            plugin_id,
            transaction_id,
            &normalized.footprint,
            &normalized.purpose,
        );
        let record = DurableBuildingCertificate {
            certificate_id,
            plugin_id: plugin_id.to_owned(),
            footprint: normalized.footprint,
            purpose: normalized.purpose.clone(),
            interaction_points: normalized.interaction_points.clone(),
            world_revision: token.world_revision,
            revision: 0,
        };
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Building {
                certificate: Box::new(record.snapshot()),
            }),
            vec![DurableSettlementChange::Building {
                certificate: Box::new(record),
            }],
        )
    }
}
