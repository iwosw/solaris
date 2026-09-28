//! Host command admission: the ledger that issues one nonce per host-attested request,
//! the admitted command value, the plugin target it belongs to and the per-plugin identity
//! the host admits under. Split out of `lib.rs` by CP-REFACTOR-040; the crate root
//! re-exports the public types, so `mc_script::{...}` keeps working.

use crate::client_view::MAX_CLIENT_VIEW_EXACT_INTEGER;
use crate::client_view::ScriptClientSelectionBegin;
use crate::client_view::ScriptClientSelectionConstraints;
use crate::client_view::ScriptClientViewOpen;
use crate::client_view::ScriptClientViewPresent;
use crate::manifest::CommandCapabilities;
use crate::manifest::RequiredCommandCapability;
use crate::manifest::ScriptCommandCapabilityKind;
use crate::manifest::ValidatedScriptPluginManifest;
use crate::manifest::validate_custom_payload_channel;
use crate::operations::ScriptOperation;
use crate::operations::ScriptOperationOutcome;
use crate::resident_order_operations::ScriptResidentOrderOperation;
use crate::settlement_operations::ScriptSettlementOperation;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::Weak;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use super::{
    CommandBatch, MAX_SCRIPT_CHAT_MESSAGE_BYTES, MAX_SCRIPT_COMMAND_QUEUE_CAPACITY,
    MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES, MAX_SCRIPT_DISCONNECT_REASON_BYTES, ScriptAxisAlignedZone,
    ScriptCommand, ScriptDtoError, ScriptEntityDamageFailure, ScriptEntityDamageRequest,
    ScriptEntitySpawnFailure, ScriptEvent, ScriptEventKind, ScriptInventoryClick,
    ScriptInventoryMenu, ScriptInventoryStorageTransaction, ScriptLoaderItemGrantFailure,
    ScriptLoaderItemGrantRequest, ScriptOnlinePlayerSnapshot, ScriptOnlinePlayersRequest,
    ScriptPlayerContext, ScriptPlayerId, ScriptPlayerInventoryFailure,
    ScriptPlayerInventoryTransaction, ScriptPlayerTeleportFailure, ScriptPlayerTeleportRequest,
    ScriptPluginStorageCompareAndSwapRequest, ScriptPluginStorageDeleteRequest,
    ScriptPluginStorageFailure, ScriptPluginStorageGetRequest, ScriptPosition,
    ScriptWorldBlockSetFailure, ScriptWorldBlockSetRequest, ScriptWorldTimeSetFailure,
    ScriptWorldTimeSetRequest, validate_bounded_nonempty, validate_contract_resource_id,
    validate_plugin_storage_value, validate_script_id, validate_target_plugin_id,
};
/// Origin attached by the component host after a handler returns bounded commands.
///
/// The constructor is crate-private so components and external adapters cannot
/// fabricate an origin. Adapters may inspect the id to route a completion event.
#[derive(Clone)]
pub struct ScriptCommandProvenance {
    plugin_id: Arc<str>,
    nonce: u64,
    _cancellation: Option<Arc<HostAdmissionCancellation>>,
}

impl std::fmt::Debug for ScriptCommandProvenance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScriptCommandProvenance")
            .field("plugin_id", &self.plugin_id)
            .field("nonce", &self.nonce)
            .finish()
    }
}

impl PartialEq for ScriptCommandProvenance {
    fn eq(&self, other: &Self) -> bool {
        self.plugin_id == other.plugin_id && self.nonce == other.nonce
    }
}

impl Eq for ScriptCommandProvenance {}

impl ScriptCommandProvenance {
    #[cfg(test)]
    pub(super) fn for_host_plugin(plugin_id: Arc<str>, nonce: u64) -> Self {
        Self {
            plugin_id,
            nonce,
            _cancellation: None,
        }
    }

    fn for_issued_host_plugin(
        plugin_id: Arc<str>,
        nonce: u64,
        ledger: &Arc<HostAdmissionLedger>,
    ) -> Self {
        Self {
            plugin_id,
            nonce,
            _cancellation: Some(Arc::new(HostAdmissionCancellation {
                ledger: Arc::downgrade(ledger),
                nonce,
            })),
        }
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
}

#[derive(Debug)]
struct HostAdmissionCancellation {
    ledger: Weak<HostAdmissionLedger>,
    nonce: u64,
}

impl Drop for HostAdmissionCancellation {
    fn drop(&mut self) {
        if let Some(ledger) = self.ledger.upgrade() {
            ledger.cancel(self.nonce);
        }
    }
}

#[derive(Debug)]
pub(super) struct HostAdmissionRecord {
    plugin_id: Arc<str>,
    request: Weak<ScriptCommand>,
    pub(super) generation: Option<u64>,
}

#[derive(Debug, Default)]
pub(super) struct HostAdmissionLedger {
    next_nonce: AtomicU64,
    pub(super) pending: StdMutex<BTreeMap<u64, HostAdmissionRecord>>,
}

impl HostAdmissionLedger {
    pub(super) fn issue(
        self: &Arc<Self>,
        plugin_id: Arc<str>,
        batch: CommandBatch,
        generation: Option<u64>,
    ) -> Result<Vec<ScriptCommand>, CommandBatch> {
        let mut pending = match self.pending.lock() {
            Ok(pending) => pending,
            Err(mut poisoned) => {
                poisoned.get_mut().clear();
                return Err(batch);
            }
        };
        if pending.len().saturating_add(batch.commands.len()) > MAX_SCRIPT_COMMAND_QUEUE_CAPACITY {
            return Err(batch);
        }
        let count = match u64::try_from(batch.commands.len()) {
            Ok(count) => count,
            Err(_) => return Err(batch),
        };
        let first =
            match self
                .next_nonce
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(count)
                }) {
                Ok(first) => first,
                Err(_) => return Err(batch),
            };
        let mut attached = Vec::with_capacity(batch.commands.len());
        for (offset, request) in (1_u64..=count).zip(batch.commands) {
            let nonce = first + offset;
            let request = Arc::new(request);
            pending.insert(
                nonce,
                HostAdmissionRecord {
                    plugin_id: Arc::clone(&plugin_id),
                    request: Arc::downgrade(&request),
                    generation,
                },
            );
            attached.push(ScriptCommand::HostAttached {
                provenance: ScriptCommandProvenance::for_issued_host_plugin(
                    Arc::clone(&plugin_id),
                    nonce,
                    self,
                ),
                request,
            });
        }
        Ok(attached)
    }

    fn cancel(&self, nonce: u64) {
        match self.pending.lock() {
            Ok(mut pending) => {
                pending.remove(&nonce);
            }
            Err(mut poisoned) => {
                poisoned.get_mut().clear();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn pending_len(&self) -> usize {
        self.pending
            .lock()
            .expect("host admission ledger lock poisoned")
            .len()
    }

    pub(super) fn accept(
        &self,
        provenance: ScriptCommandProvenance,
        request: Arc<ScriptCommand>,
        current_generation: Option<u64>,
    ) -> Result<AdmittedScriptCommand, ScriptCommandAcceptanceError> {
        let mut pending = match self.pending.lock() {
            Ok(pending) => pending,
            Err(mut poisoned) => {
                poisoned.get_mut().clear();
                return Err(ScriptCommandAcceptanceError::AuthorityPoisoned);
            }
        };
        let Some(record) = pending.remove(&provenance.nonce) else {
            return Err(ScriptCommandAcceptanceError::UnknownOrConsumed);
        };
        if record.plugin_id != provenance.plugin_id
            || !record.request.ptr_eq(&Arc::downgrade(&request))
        {
            return Err(ScriptCommandAcceptanceError::RequestMismatch);
        }
        Ok(AdmittedScriptCommand {
            plugin_id: record.plugin_id,
            request,
            generation: record.generation.or(current_generation),
        })
    }
}

/// One exact host-attested plugin request accepted by the server boundary.
///
/// The value is intentionally not cloneable. A router must accept the raw
/// `HostAttached` command through `ScriptBoundary::accept_host_command` before
/// dispatching any plugin-owned side effect.
#[derive(Debug, PartialEq, Eq)]
pub struct AdmittedScriptCommand {
    plugin_id: Arc<str>,
    request: Arc<ScriptCommand>,
    generation: Option<u64>,
}

impl AdmittedScriptCommand {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn request(&self) -> &ScriptCommand {
        &self.request
    }

    pub fn into_request(self) -> ScriptCommand {
        Arc::try_unwrap(self.request).unwrap_or_else(|request| request.as_ref().clone())
    }

    pub fn into_open_inventory_menu(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptPlayerId, ScriptInventoryMenu), ScriptDtoError> {
        let ScriptCommand::OpenInventoryMenu { player_id, menu } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "inventory menu admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            *player_id,
            menu.clone(),
        ))
    }

    pub fn into_open_client_view(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptClientViewOpen), ScriptDtoError> {
        let request =
            Arc::try_unwrap(self.request).unwrap_or_else(|request| request.as_ref().clone());
        let ScriptCommand::OpenClientView { request } = request else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "client view open admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            request,
        ))
    }

    pub fn into_present_client_view(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptClientViewPresent), ScriptDtoError> {
        let request =
            Arc::try_unwrap(self.request).unwrap_or_else(|request| request.as_ref().clone());
        let ScriptCommand::PresentClientView { request } = request else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "client view present admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            request,
        ))
    }

    pub fn into_close_client_view(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptPlayerId, String), ScriptDtoError> {
        let ScriptCommand::CloseClientView {
            player_id,
            view_instance_id,
        } = self.request.as_ref()
        else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "client view close admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            *player_id,
            view_instance_id.clone(),
        ))
    }

    pub fn into_begin_client_selection(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptClientSelectionBegin), ScriptDtoError> {
        let request =
            Arc::try_unwrap(self.request).unwrap_or_else(|request| request.as_ref().clone());
        let ScriptCommand::BeginClientSelection {
            request_id,
            player_id,
            view_instance_id,
            view_revision,
            action_id,
            constraints,
        } = request
        else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "client selection begin admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            ScriptClientSelectionBegin::try_new(
                &request_id,
                player_id,
                &view_instance_id,
                view_revision,
                &action_id,
                constraints,
            )?,
        ))
    }

    pub fn into_cancel_client_selection(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptPlayerId, String), ScriptDtoError> {
        let ScriptCommand::CancelClientSelection {
            player_id,
            selection_context_id,
        } = self.request.as_ref()
        else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "client selection cancel admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            *player_id,
            selection_context_id.clone(),
        ))
    }

    pub fn into_upsert_zone(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptAxisAlignedZone), ScriptDtoError> {
        let ScriptCommand::UpsertZone { zone } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "zone admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            zone.clone(),
        ))
    }

    pub fn into_remove_zone(self) -> Result<(ScriptPluginTarget, String), ScriptDtoError> {
        let ScriptCommand::RemoveZone { zone_id } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "zone removal admission",
            });
        };
        Ok((
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            zone_id.clone(),
        ))
    }

    pub fn plugin_storage_get_result(
        self,
        value: Option<&str>,
        version: Option<u64>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::PluginStorageGet { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage get admission",
            });
        };
        if let Some(value) = value {
            validate_plugin_storage_value(value)?;
        }
        ScriptEvent::plugin_storage_get_result(
            &self.plugin_id,
            request,
            value.map(str::to_owned),
            version,
        )
        .map(|event| event.with_origin_generation(self.generation))
    }

    pub fn plugin_storage_cas_result(
        self,
        applied: bool,
        version: Option<u64>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::PluginStorageCompareAndSwap { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage compare-and-swap admission",
            });
        };
        ScriptEvent::plugin_storage_cas_result(&self.plugin_id, request, applied, version)
            .map(|event| event.with_origin_generation(self.generation))
    }

    pub fn plugin_storage_delete_result(
        self,
        deleted: bool,
        version: Option<u64>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::PluginStorageDelete { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "plugin storage delete admission",
            });
        };
        ScriptEvent::plugin_storage_delete_result(&self.plugin_id, request, deleted, version)
            .map(|event| event.with_origin_generation(self.generation))
    }

    pub fn plugin_storage_failure_result(
        self,
        failure: ScriptPluginStorageFailure,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let target_plugin_id = validate_target_plugin_id(&self.plugin_id)?;
        let kind = match self.request.as_ref() {
            ScriptCommand::PluginStorageGet { request } => {
                ScriptEventKind::PluginStorageGetResult {
                    request_id: request.request_id().to_owned(),
                    key: request.key().to_owned(),
                    value: None,
                    version: None,
                    failure: Some(failure),
                }
            }
            ScriptCommand::PluginStorageCompareAndSwap { request } => {
                ScriptEventKind::PluginStorageCasResult {
                    request_id: request.request_id().to_owned(),
                    key: request.key().to_owned(),
                    applied: false,
                    version: None,
                    failure: Some(failure),
                }
            }
            ScriptCommand::PluginStorageDelete { request } => {
                ScriptEventKind::PluginStorageDeleteResult {
                    request_id: request.request_id().to_owned(),
                    key: request.key().to_owned(),
                    deleted: false,
                    version: None,
                    failure: Some(failure),
                }
            }
            _ => {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "plugin storage failure admission",
                });
            }
        };
        Ok(ScriptEvent {
            target_plugin_id: Some(target_plugin_id),
            kind,
            origin_generation: self.generation,
        })
    }

    pub fn inventory_storage_transaction_result(
        self,
        committed: bool,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::InventoryStorageTransaction { transaction } = self.request.as_ref()
        else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "inventory-storage transaction admission",
            });
        };
        ScriptEvent::inventory_storage_transaction_result(&self.plugin_id, transaction, committed)
            .map(|event| event.with_origin_generation(self.generation))
    }

    pub fn operation_result(
        self,
        outcome: ScriptOperationOutcome,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::Operation { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "operation admission",
            });
        };
        ScriptEvent::operation_result(
            &self.plugin_id,
            request.request_id(),
            request.operation_id(),
            outcome,
        )
        .map(|event| event.with_origin_generation(self.generation))
    }

    pub fn player_inventory_transaction_result(
        self,
        failure: Option<ScriptPlayerInventoryFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::PlayerInventoryTransaction { transaction } = self.request.as_ref()
        else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "player inventory transaction admission",
            });
        };
        ScriptEvent::player_inventory_transaction_result(&self.plugin_id, transaction, failure)
    }

    pub fn player_teleport_result(
        self,
        failure: Option<ScriptPlayerTeleportFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::TeleportPlayer { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "player teleport admission",
            });
        };
        ScriptEvent::player_teleport_result(&self.plugin_id, request, failure)
    }

    pub fn world_time_set_result(
        self,
        failure: Option<ScriptWorldTimeSetFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::SetWorldTime { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "world time admission",
            });
        };
        ScriptEvent::world_time_set_result(&self.plugin_id, request, failure)
    }

    pub fn world_block_set_result(
        self,
        applied: bool,
        failure: Option<ScriptWorldBlockSetFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::SetWorldBlock { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "world block admission",
            });
        };
        ScriptEvent::world_block_set_result(&self.plugin_id, request, applied, failure)
    }

    pub fn loader_item_grant_result(
        self,
        failure: Option<ScriptLoaderItemGrantFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::GrantLoaderBlockItem { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "loader item grant admission",
            });
        };
        ScriptEvent::loader_item_grant_result(&self.plugin_id, request, failure)
    }

    pub fn entity_spawn_result(
        self,
        failure: Option<ScriptEntitySpawnFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::SpawnEntity {
            request_id,
            actor,
            entity_type,
            position,
        } = self.request.as_ref()
        else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "entity spawn admission",
            });
        };
        ScriptEvent::entity_spawn_result(
            &self.plugin_id,
            request_id,
            *actor,
            entity_type,
            *position,
            failure,
        )
    }

    pub fn entity_damage_result(
        self,
        health: Option<f32>,
        killed: bool,
        failure: Option<ScriptEntityDamageFailure>,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::DamageEntity { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "entity damage admission",
            });
        };
        ScriptEvent::entity_damage_result(&self.plugin_id, request, health, killed, failure)
    }

    pub fn online_players_result(
        self,
        players: Vec<ScriptOnlinePlayerSnapshot>,
        truncated: bool,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        let ScriptCommand::ListOnlinePlayers { request } = self.request.as_ref() else {
            return Err(ScriptDtoError::InconsistentResult {
                field: "online players admission",
            });
        };
        ScriptEvent::online_players_result(&self.plugin_id, request, players, truncated)
    }

    /// Consume an admitted outbound custom-payload command, moving the payload
    /// out of the shared request without copying when uniquely owned.
    pub fn into_send_custom_payload(
        self,
    ) -> Result<(ScriptPluginTarget, ScriptPlayerId, String, Vec<u8>), ScriptDtoError> {
        let target = ScriptPluginTarget {
            plugin_id: Arc::clone(&self.plugin_id),
        };
        match Arc::try_unwrap(self.request) {
            Ok(ScriptCommand::SendCustomPayload {
                player_id,
                channel,
                payload,
            }) => Ok((target, player_id, channel, payload)),
            Ok(_) => Err(ScriptDtoError::InconsistentResult {
                field: "custom payload admission",
            }),
            Err(request) => match request.as_ref() {
                ScriptCommand::SendCustomPayload {
                    player_id,
                    channel,
                    payload,
                } => Ok((target, *player_id, channel.clone(), payload.clone())),
                _ => Err(ScriptDtoError::InconsistentResult {
                    field: "custom payload admission",
                }),
            },
        }
    }
}

/// Opaque plugin target retained by a production adapter after accepting an
/// owning menu or zone command. External code cannot fabricate a plugin id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPluginTarget {
    plugin_id: Arc<str>,
}

impl ScriptPluginTarget {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn inventory_menu_clicked(
        &self,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        menu: &ScriptInventoryMenu,
        slot: u8,
        click: ScriptInventoryClick,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        ScriptEvent::inventory_menu_clicked(&self.plugin_id, player_id, context, menu, slot, click)
    }

    pub fn player_zone_entered(
        &self,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone: &ScriptAxisAlignedZone,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        ScriptEvent::player_zone_entered(&self.plugin_id, player_id, context, zone)
    }

    pub fn player_zone_exited(
        &self,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        zone: &ScriptAxisAlignedZone,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        ScriptEvent::player_zone_exited(&self.plugin_id, player_id, context, zone)
    }

    pub fn zone_command_result(
        &self,
        zone_id: impl AsRef<str>,
        accepted: bool,
    ) -> Result<ScriptEvent, ScriptDtoError> {
        ScriptEvent::zone_command_result(&self.plugin_id, zone_id, accepted)
    }
}

#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptCommandAcceptanceError {
    NotHostAttached,
    UnknownOrConsumed,
    RequestMismatch,
    AuthorityPoisoned,
}

impl ScriptCommand {
    /// Return the host capability required before admitting this command.
    pub fn required_capability_kind(&self) -> Option<ScriptCommandCapabilityKind> {
        self.required_capability()
            .map(RequiredCommandCapability::kind)
    }

    pub(super) fn required_capability(&self) -> Option<RequiredCommandCapability<'_>> {
        match self {
            Self::HostAttached { request, .. } => request.required_capability(),
            Self::SendChatMessage { .. }
            | Self::BroadcastChatMessage { .. }
            | Self::DisconnectPlayer { .. }
            | Self::OpenClientView { .. }
            | Self::PresentClientView { .. }
            | Self::CloseClientView { .. }
            | Self::BeginClientSelection { .. }
            | Self::CancelClientSelection { .. }
            | Self::ClientSound { .. }
            | Self::GrantLoaderBlockItem { .. } => None,
            Self::SpawnEntity { entity_type, .. } => {
                Some(RequiredCommandCapability::SpawnEntityType { entity_type })
            }
            Self::DamageEntity { .. } => Some(RequiredCommandCapability::EntityDamage),
            Self::PluginStorageGet { .. }
            | Self::PluginStorageCompareAndSwap { .. }
            | Self::PluginStorageDelete { .. } => Some(RequiredCommandCapability::PluginStorage),
            Self::Operation { request } => Some(match request.operation() {
                ScriptOperation::Inventory { .. } => RequiredCommandCapability::InventoryTransfers,
                ScriptOperation::Resident { .. } => RequiredCommandCapability::PersistentResidents,
                ScriptOperation::ResidentOrder { operation } => match operation {
                    ScriptResidentOrderOperation::AssignWork { .. }
                    | ScriptResidentOrderOperation::CancelWork { .. } => {
                        RequiredCommandCapability::ResidentWork
                    }
                    ScriptResidentOrderOperation::IssueOrder { .. }
                    | ScriptResidentOrderOperation::CancelOrder { .. }
                    | ScriptResidentOrderOperation::Demobilize { .. }
                    | ScriptResidentOrderOperation::Capture { .. } => {
                        RequiredCommandCapability::ResidentOrders
                    }
                },
                ScriptOperation::Settlement { operation } => match operation {
                    ScriptSettlementOperation::ListSites { .. }
                    | ScriptSettlementOperation::QuerySite { .. }
                    | ScriptSettlementOperation::ReserveResidentSite { .. }
                    | ScriptSettlementOperation::ReleaseResidentSite { .. }
                    | ScriptSettlementOperation::Survey { .. } => {
                        RequiredCommandCapability::WorldSites
                    }
                    ScriptSettlementOperation::PrepareStructure { .. }
                    | ScriptSettlementOperation::AdvanceStructure { .. }
                    | ScriptSettlementOperation::PauseStructure { .. }
                    | ScriptSettlementOperation::ResumeStructure { .. }
                    | ScriptSettlementOperation::CancelStructure { .. }
                    | ScriptSettlementOperation::Status { .. }
                    | ScriptSettlementOperation::BindWarehouse { .. }
                    | ScriptSettlementOperation::BindVillageWarehouse { .. }
                    | ScriptSettlementOperation::BindManualWarehouse { .. }
                    | ScriptSettlementOperation::RecognizeBuilding { .. } => {
                        RequiredCommandCapability::StructureOperations
                    }
                },
                _ => RequiredCommandCapability::StorageBatches,
            }),
            Self::OpenInventoryMenu { .. } | Self::CloseInventoryMenu { .. } => {
                Some(RequiredCommandCapability::InventoryMenus)
            }
            Self::InventoryStorageTransaction { .. } => {
                Some(RequiredCommandCapability::InventoryStorageTransactions)
            }
            Self::PlayerInventoryTransaction { .. } => {
                Some(RequiredCommandCapability::PlayerInventory)
            }
            Self::UpsertZone { .. } | Self::RemoveZone { .. } => {
                Some(RequiredCommandCapability::Zones)
            }
            Self::TeleportPlayer { .. } => Some(RequiredCommandCapability::PlayerTeleport),
            Self::SetWorldTime { .. } => Some(RequiredCommandCapability::WorldTime),
            Self::SetWorldBlock { .. } => Some(RequiredCommandCapability::WorldBlocks),
            Self::ListOnlinePlayers { .. } => Some(RequiredCommandCapability::PlayerQueries),
            Self::SendCustomPayload { channel, .. } => {
                Some(RequiredCommandCapability::CustomPayloadChannel { channel })
            }
        }
    }

    pub(super) fn validate_contract(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::HostAttached { .. } => Err(ScriptDtoError::InconsistentResult {
                field: "nested host-attached command",
            }),
            Self::SendChatMessage { message, .. } | Self::BroadcastChatMessage { message } => {
                validate_bounded_nonempty("chat message", message, MAX_SCRIPT_CHAT_MESSAGE_BYTES)
            }
            Self::DisconnectPlayer { reason, .. } => validate_bounded_nonempty(
                "disconnect reason",
                reason,
                MAX_SCRIPT_DISCONNECT_REASON_BYTES,
            ),
            Self::SpawnEntity {
                request_id,
                entity_type,
                position,
                ..
            } => {
                validate_script_id(request_id)?;
                validate_contract_resource_id(entity_type)?;
                ScriptPosition::try_new(position.x(), position.y(), position.z())
                    .ok_or(ScriptDtoError::InvalidBounds)
                    .map(drop)
            }
            Self::DamageEntity { request } => ScriptEntityDamageRequest::try_new(
                request.request_id(),
                request.entity_id(),
                request.amount(),
            )
            .map(drop),
            Self::OpenInventoryMenu { menu, .. } => {
                ScriptInventoryMenu::try_new(menu.id(), menu.title(), menu.slots().to_vec())
                    .map(drop)
            }
            // The view lifecycle DTOs have private fields and checked constructors.
            Self::OpenClientView { request } => ScriptClientViewOpen::try_new(
                request.request_id(),
                request.player_id(),
                request.owned_view_id(),
                request.model().clone(),
            )
            .map(drop),
            Self::PresentClientView { request } => ScriptClientViewPresent::try_new(
                request.player_id(),
                request.view_instance_id(),
                request.expected_revision(),
                request.model().clone(),
            )
            .map(drop),
            Self::CloseClientView {
                view_instance_id, ..
            } => validate_contract_resource_id(view_instance_id).map(drop),
            Self::BeginClientSelection {
                request_id,
                view_instance_id,
                view_revision,
                action_id,
                constraints,
                ..
            } => {
                validate_script_id(request_id)?;
                validate_contract_resource_id(view_instance_id)?;
                validate_contract_resource_id(action_id)?;
                if *view_revision == 0 || *view_revision > MAX_CLIENT_VIEW_EXACT_INTEGER {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                ScriptClientSelectionConstraints::try_new(
                    constraints.dimension(),
                    constraints.range_limit(),
                    constraints.ttl_ticks(),
                    constraints.formation(),
                    constraints.radius(),
                )
                .map(drop)
            }
            Self::CancelClientSelection {
                selection_context_id,
                ..
            } => validate_contract_resource_id(selection_context_id).map(drop),
            // The sound DTO has private fields and a checked constructor.
            Self::ClientSound { .. } => Ok(()),
            Self::GrantLoaderBlockItem { request } => ScriptLoaderItemGrantRequest::try_new(
                request.request_id(),
                request.player_id(),
                request.block_id(),
                request.count(),
            )
            .map(drop),
            Self::UpsertZone { zone } => ScriptAxisAlignedZone::try_new_with_protection(
                zone.id(),
                zone.dimension(),
                zone.minimum(),
                zone.maximum(),
                zone.protection().cloned(),
            )
            .map(drop),
            Self::PluginStorageGet { request } => {
                ScriptPluginStorageGetRequest::try_new(request.request_id(), request.key())
                    .map(drop)
            }
            Self::PluginStorageCompareAndSwap { request } => {
                ScriptPluginStorageCompareAndSwapRequest::try_new(
                    request.request_id(),
                    request.key(),
                    request.expected_version(),
                    request.value(),
                )
                .map(drop)
            }
            Self::PluginStorageDelete { request } => ScriptPluginStorageDeleteRequest::try_new(
                request.request_id(),
                request.key(),
                request.expected_version(),
            )
            .map(drop),
            Self::Operation { request } => request.validate(),
            Self::CloseInventoryMenu { menu_id, .. } => validate_script_id(menu_id).map(drop),
            Self::InventoryStorageTransaction { transaction } => {
                ScriptInventoryStorageTransaction::try_new(
                    transaction.id(),
                    transaction.player_id(),
                    transaction.inventory().to_vec(),
                    transaction.storage().to_vec(),
                )
                .map(drop)
            }
            Self::PlayerInventoryTransaction { transaction } => {
                ScriptPlayerInventoryTransaction::try_new(
                    transaction.request_id(),
                    transaction.player_id(),
                    transaction.deltas().to_vec(),
                )
                .map(drop)
            }
            Self::RemoveZone { zone_id } => validate_script_id(zone_id).map(drop),
            Self::TeleportPlayer { request } => ScriptPlayerTeleportRequest::try_new(
                request.request_id(),
                request.player_id(),
                request.position(),
            )
            .map(drop),
            Self::SetWorldTime { request } => {
                ScriptWorldTimeSetRequest::try_new(request.request_id(), request.world_time())
                    .map(drop)
            }
            Self::SetWorldBlock { request } => ScriptWorldBlockSetRequest::try_new(
                request.request_id(),
                request.dimension(),
                request.block_id(),
                request.x(),
                request.y(),
                request.z(),
            )
            .map(drop),
            Self::ListOnlinePlayers { request } => {
                ScriptOnlinePlayersRequest::try_new(request.request_id(), request.limit()).map(drop)
            }
            Self::SendCustomPayload {
                channel, payload, ..
            } => {
                validate_custom_payload_channel(channel).map_err(|_| {
                    ScriptDtoError::InvalidResourceId {
                        field: "custom payload channel",
                        actual_bytes: channel.len(),
                    }
                })?;
                if payload.len() > MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES {
                    return Err(ScriptDtoError::ValueTooLong {
                        field: "custom payload",
                        max_bytes: MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES,
                        actual_bytes: payload.len(),
                    });
                }
                Ok(())
            }
        }
    }
}

/// The identity one plugin instance's commands are admitted under.
///
/// The host builds it from the validated manifest and keeps it for the life of
/// the instance; a plugin never names its own identity, and this type is the only
/// way to reach the admission ledger.
#[derive(Debug, Clone)]
pub struct HostCommandAdmission {
    plugin_id: Arc<str>,
    capabilities: Arc<CommandCapabilities>,
}

impl HostCommandAdmission {
    /// The admission of one validated manifest: its id and its granted
    /// capabilities.
    #[must_use]
    pub fn from_manifest(manifest: &ValidatedScriptPluginManifest) -> Self {
        Self {
            plugin_id: Arc::from(manifest.plugin_id()),
            capabilities: Arc::new(manifest.to_command_capabilities()),
        }
    }

    /// The plugin id these commands will be attributed to.
    #[must_use]
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// What the manifest behind this admission is allowed to command.
    ///
    /// Exposed so a runtime host can convert a guest's batch against the same
    /// grants the boundary will check again: the answer is what the package
    /// declared, never what a guest claims at call time.
    #[must_use]
    pub fn capabilities(&self) -> &CommandCapabilities {
        &self.capabilities
    }
}

impl ScriptCommandProvenance {
    /// Ledger nonce this provenance was issued under.
    pub(super) fn nonce(&self) -> u64 {
        self.nonce
    }
}

impl AdmittedScriptCommand {
    /// Consume the admitted request into the plugin target and the request itself.
    pub(crate) fn into_target_and_request(self) -> (ScriptPluginTarget, Arc<ScriptCommand>) {
        (
            ScriptPluginTarget {
                plugin_id: self.plugin_id,
            },
            self.request,
        )
    }
}

impl HostCommandAdmission {
    /// Plugin identity as the shared handle the admission ledger issues under.
    pub(super) fn plugin_id_arc(&self) -> Arc<str> {
        Arc::clone(&self.plugin_id)
    }
}
