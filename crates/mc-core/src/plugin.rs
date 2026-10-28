//! Production component ownership and native tick-owned plugin persistence.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

use mc_plugin_host::adapter::PlayerSessions;
use mc_script::{
    ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload, ScriptStorageChange,
    ScriptStorageEntry, ScriptStorageMutation,
};
use serde::{Deserialize, Serialize};

use crate::config::{CoreConfig, CoreError};
use crate::tick::TickState;
mod builds;
pub(crate) use builds::*;
mod events;
pub(crate) use events::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StorageRecord {
    pub value: Option<String>,
    pub revision: u64,
}
/// Retain operation identities for the world's lifetime: forgetting one would
/// allow its retry to execute again without a durable older-id refusal rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OperationReceipt {
    operation: mc_script::ScriptOperationRequest,
    outcome: ScriptOperationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum DomainPostImage {
    Present(serde_json::Value),
    Removed,
}

/// Absolute post-images of only the keys touched by one native tick.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginDelta {
    storage: BTreeMap<String, BTreeMap<String, StorageRecord>>,
    receipts: BTreeMap<String, BTreeMap<String, OperationReceipt>>,
    domains: BTreeMap<String, BTreeMap<String, DomainPostImage>>,
    revision: u64,
}

impl PluginDelta {
    #[must_use]
    pub fn storage_value_version(&self, owner: &str, key: &str) -> Option<(Option<&str>, u64)> {
        self.storage
            .get(owner)?
            .get(key)
            .map(|record| (record.value.as_deref(), record.revision))
    }

    /// Cheap overestimate of the heap this delta owns, for the journal queue's
    /// byte budget: key/value text by length, JSON values by capacity, tree
    /// slots (with node slack and root allocation) per element, and a flat
    /// allowance for each receipt's operation payload. No serialization.
    pub(crate) fn heap_estimate(&self) -> usize {
        use crate::journal_budget::{NESTED_STATE_ALLOWANCE, tree_root, tree_slot, value_heap};
        use std::mem::size_of;
        let string = size_of::<String>();
        let storage = self.storage.iter().map(|(owner, records)| {
            owner.capacity()
                + tree_slot(string + size_of::<BTreeMap<String, StorageRecord>>())
                + tree_root(records.len(), string + size_of::<StorageRecord>())
                + records
                    .iter()
                    .map(|(key, record)| {
                        key.capacity()
                            + tree_slot(string + size_of::<StorageRecord>())
                            + record.value.as_ref().map_or(0, String::capacity)
                    })
                    .sum::<usize>()
        });
        let receipts = self.receipts.iter().map(|(owner, records)| {
            owner.capacity()
                + tree_slot(string + size_of::<BTreeMap<String, OperationReceipt>>())
                + tree_root(records.len(), string + size_of::<OperationReceipt>())
                + records
                    .keys()
                    .map(|key| {
                        key.capacity()
                            + tree_slot(string + size_of::<OperationReceipt>())
                            + NESTED_STATE_ALLOWANCE
                    })
                    .sum::<usize>()
        });
        let domains = self.domains.iter().map(|(owner, images)| {
            owner.capacity()
                + tree_slot(string + size_of::<BTreeMap<String, DomainPostImage>>())
                + tree_root(images.len(), string + size_of::<DomainPostImage>())
                + images
                    .iter()
                    .map(|(key, image)| {
                        key.capacity()
                            + tree_slot(string + size_of::<DomainPostImage>())
                            + match image {
                                DomainPostImage::Present(value) => value_heap(value),
                                DomainPostImage::Removed => 0,
                            }
                    })
                    .sum::<usize>()
        });
        let roots = tree_root(
            self.storage.len(),
            string + size_of::<BTreeMap<String, StorageRecord>>(),
        ) + tree_root(
            self.receipts.len(),
            string + size_of::<BTreeMap<String, OperationReceipt>>(),
        ) + tree_root(
            self.domains.len(),
            string + size_of::<BTreeMap<String, DomainPostImage>>(),
        );
        roots + storage.chain(receipts).chain(domains).sum::<usize>()
    }
}

type DirtyKeys = BTreeMap<String, BTreeSet<String>>;

fn mark_key(keys: &mut DirtyKeys, owner: &str, key: &str) {
    if let Some(keys) = keys.get_mut(owner) {
        if !keys.contains(key) {
            keys.insert(key.into());
        }
    } else {
        keys.insert(owner.into(), BTreeSet::from([key.into()]));
    }
}

/// One authoritative image: key tombstones, operation receipts and domain custody
/// share the same native checkpoint and tick WAL as their physical effects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginState {
    storage: BTreeMap<String, BTreeMap<String, StorageRecord>>,
    receipts: BTreeMap<String, BTreeMap<String, OperationReceipt>>,
    domains: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    revision: u64,
    #[serde(skip)]
    pub(crate) dirty: bool,
    #[serde(skip)]
    dirty_storage: DirtyKeys,
    #[serde(skip)]
    dirty_receipts: DirtyKeys,
    #[serde(skip)]
    dirty_domains: DirtyKeys,
}

impl PluginState {
    pub(crate) fn get_domain(&self, owner: &str, key: &str) -> Option<&serde_json::Value> {
        self.domains.get(owner)?.get(key)
    }

    pub(crate) fn get_domain_mut(
        &mut self,
        owner: &str,
        key: &str,
    ) -> Option<&mut serde_json::Value> {
        let value = self.domains.get_mut(owner)?.get_mut(key)?;
        self.dirty = true;
        mark_key(&mut self.dirty_domains, owner, key);
        Some(value)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn domain_entries(
        &self,
        owner: &str,
    ) -> impl Iterator<Item = (&str, &serde_json::Value)> {
        self.domains
            .get(owner)
            .into_iter()
            .flat_map(|domain| domain.iter())
            .map(|(key, value)| (key.as_str(), value))
    }

    pub(crate) fn domain_prefix<'a>(
        &'a self,
        owner: &str,
        prefix: &str,
    ) -> impl Iterator<Item = (&'a str, &'a serde_json::Value)> {
        self.domains
            .get(owner)
            .into_iter()
            .flat_map(move |domain| {
                domain.range::<str, _>((
                    std::ops::Bound::Included(prefix),
                    std::ops::Bound::Unbounded,
                ))
            })
            .take_while(move |(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.as_str(), value))
    }

    pub(crate) fn set_domain(&mut self, owner: &str, key: &str, value: serde_json::Value) {
        self.domains
            .entry(owner.into())
            .or_default()
            .insert(key.into(), value);
        self.dirty = true;
        mark_key(&mut self.dirty_domains, owner, key);
    }

    pub(crate) fn delete_domain(&mut self, owner: &str, key: &str) {
        if self
            .domains
            .get_mut(owner)
            .is_some_and(|domain| domain.remove(key).is_some())
        {
            self.dirty = true;
            mark_key(&mut self.dirty_domains, owner, key);
        }
    }

    pub(crate) fn domain_plugins(&self) -> impl Iterator<Item = &str> {
        self.domains.keys().map(String::as_str)
    }

    /// Storage owners with live values (for legacy-state refusal scans).
    pub(crate) fn storage_owners(&self) -> impl Iterator<Item = (&str, &str)> {
        self.storage.iter().flat_map(|(owner, records)| {
            records
                .iter()
                .filter(|(_, record)| record.value.is_some())
                .map(move |(key, _)| (owner.as_str(), key.as_str()))
        })
    }

    pub(crate) fn record(&self, owner: &str, key: &str) -> Option<&StorageRecord> {
        self.storage.get(owner)?.get(key)
    }

    #[cfg(test)]
    /// Inspect the native persisted image, including deletion tombstones.
    #[must_use]
    pub fn storage_value_version(&self, owner: &str, key: &str) -> Option<(Option<&str>, u64)> {
        self.record(owner, key)
            .map(|record| (record.value.as_deref(), record.revision))
    }

    pub(crate) fn operation_receipt(
        &self,
        owner: &str,
        id: &str,
    ) -> Option<&ScriptOperationOutcome> {
        self.receipts
            .get(owner)?
            .get(id)
            .map(|receipt| &receipt.outcome)
    }

    pub(crate) fn version(&self, owner: &str, key: &str) -> Option<u64> {
        self.record(owner, key)
            .filter(|record| record.value.is_some())
            .map(|record| record.revision)
    }

    pub(crate) fn check_storage(&self, owner: &str, mutations: &[ScriptStorageMutation]) -> bool {
        mutations.iter().all(|mutation| {
            let expected = match mutation {
                ScriptStorageMutation::CompareAndSwap {
                    expected_version, ..
                }
                | ScriptStorageMutation::Delete {
                    expected_version, ..
                } => *expected_version,
                _ => return false,
            };
            self.version(owner, mutation.key()) == expected
        }) && self.revision < mc_script::MAX_SCRIPT_WORLD_TIME
    }

    /// The caller validates every inventory/world precondition before this commit.
    pub(crate) fn apply_storage(
        &mut self,
        owner: &str,
        mutations: &[ScriptStorageMutation],
    ) -> Vec<ScriptStorageChange> {
        assert!(
            self.check_storage(owner, mutations),
            "storage CAS must be prepared before commit"
        );
        self.revision += 1;
        let records = self.storage.entry(owner.into()).or_default();
        let mut changes = Vec::with_capacity(mutations.len());
        for mutation in mutations {
            let (key, value) = match mutation {
                ScriptStorageMutation::CompareAndSwap { key, value, .. } => {
                    (key, Some(value.clone()))
                }
                ScriptStorageMutation::Delete { key, .. } => (key, None),
                _ => unreachable!("validated storage mutation"),
            };
            let deleted = value.is_none();
            records.insert(
                key.clone(),
                StorageRecord {
                    value,
                    revision: self.revision,
                },
            );
            changes.push(ScriptStorageChange::new(key.clone(), deleted));
            mark_key(&mut self.dirty_storage, owner, key);
        }
        self.dirty = true;
        changes
    }

    fn take_delta(&mut self) -> PluginDelta {
        self.dirty = false;
        PluginDelta {
            storage: std::mem::take(&mut self.dirty_storage)
                .into_iter()
                .map(|(owner, keys)| {
                    let records = keys
                        .into_iter()
                        .map(|key| {
                            let record = self.storage[&owner][&key].clone();
                            (key, record)
                        })
                        .collect();
                    (owner, records)
                })
                .collect(),
            receipts: std::mem::take(&mut self.dirty_receipts)
                .into_iter()
                .map(|(owner, keys)| {
                    let records = keys
                        .into_iter()
                        .map(|key| {
                            let receipt = self.receipts[&owner][&key].clone();
                            (key, receipt)
                        })
                        .collect();
                    (owner, records)
                })
                .collect(),
            domains: std::mem::take(&mut self.dirty_domains)
                .into_iter()
                .map(|(owner, keys)| {
                    let records = keys
                        .into_iter()
                        .map(|key| {
                            let image = self
                                .get_domain(&owner, &key)
                                .map_or(DomainPostImage::Removed, |value| {
                                    DomainPostImage::Present(value.clone())
                                });
                            (key, image)
                        })
                        .collect();
                    (owner, records)
                })
                .collect(),
            revision: self.revision,
        }
    }

    pub(crate) fn apply_delta(&mut self, delta: &PluginDelta) {
        for (owner, records) in &delta.storage {
            self.storage.entry(owner.clone()).or_default().extend(
                records
                    .iter()
                    .map(|(key, record)| (key.clone(), record.clone())),
            );
        }
        for (owner, receipts) in &delta.receipts {
            self.receipts.entry(owner.clone()).or_default().extend(
                receipts
                    .iter()
                    .map(|(id, receipt)| (id.clone(), receipt.clone())),
            );
        }
        for (owner, records) in &delta.domains {
            let domain = self.domains.entry(owner.clone()).or_default();
            for (key, image) in records {
                match image {
                    DomainPostImage::Present(value) => {
                        domain.insert(key.clone(), value.clone());
                    }
                    DomainPostImage::Removed => {
                        domain.remove(key);
                    }
                }
            }
        }
        self.revision = delta.revision;
    }

    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if self.revision > mc_script::MAX_SCRIPT_WORLD_TIME {
            return Err(CoreError::Storage(
                "native plugins: invalid revision".into(),
            ));
        }
        for (owner, records) in &self.storage {
            for (key, record) in records {
                if owner.is_empty() || record.revision == 0 || record.revision > self.revision {
                    return Err(CoreError::Storage(
                        "native plugins: invalid storage record".into(),
                    ));
                }
                mc_script::ScriptPluginStorageGetRequest::try_new("validate", key)
                    .map_err(|e| CoreError::Storage(format!("native plugins: {e:?}")))?;
                if let Some(value) = &record.value {
                    mc_script::ScriptPluginStorageCompareAndSwapRequest::try_new(
                        "validate", key, None, value,
                    )
                    .map_err(|e| CoreError::Storage(format!("native plugins: {e:?}")))?;
                }
            }
        }
        for receipts in self.receipts.values() {
            for (id, receipt) in receipts {
                if receipt.operation.operation_id() != Some(id.as_str()) {
                    return Err(CoreError::Storage(
                        "native plugins: receipt identity mismatch".into(),
                    ));
                }
                receipt
                    .operation
                    .validate()
                    .map_err(|e| CoreError::Storage(format!("native plugins: {e:?}")))?;
                receipt
                    .outcome
                    .validate()
                    .map_err(|e| CoreError::Storage(format!("native plugins: {e:?}")))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct Sessions(RwLock<BTreeMap<String, u64>>);
impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        self.0.read().get(player).copied()
    }
}

#[derive(Debug, Default)]
pub(crate) struct PluginStats {
    pub(crate) operations: std::sync::atomic::AtomicU64,
    pub(crate) command_results: std::sync::atomic::AtomicU64,
}

struct ExecutionTrace {
    owner: String,
    session: Option<u64>,
    request: String,
    outcome: &'static str,
}

struct CommittedEvents {
    tick: u64,
    events: std::collections::VecDeque<mc_script::ScriptEvent>,
    changes: Vec<(crate::boundary::ConnectionId, crate::boundary::PlayerChange)>,
    traces: Vec<ExecutionTrace>,
}

fn command_catalog(boundary: &mc_script::ScriptBoundary) -> Arc<[(String, u8)]> {
    let mut roots: Vec<_> = boundary
        .player_command_roots()
        .into_iter()
        .map(|root| (root, 0))
        .chain(
            boundary
                .operator_command_roots()
                .into_iter()
                .map(|root| (root, 2)),
        )
        .collect();
    roots.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    roots.into()
}

struct PendingStructureBuild {
    commands: Vec<mc_script::AdmittedScriptCommand>,
    plan: crate::plugin_structures::PreparedBuild,
    decision: mc_script::precommit::PendingDecision,
}

#[allow(
    clippy::large_enum_variant,
    reason = "bounded tick-owned plans avoid per-request boxing"
)]
enum NpcBuildPlan {
    Structure {
        operation: mc_script::ScriptWorldStructureOperation,
        plan: crate::plugin_structures::PreparedBuild,
    },
    Blocks {
        work: mc_script::ScriptNpcWorkOrder,
        plan: PreparedNpcBlockWork,
    },
}

pub(crate) struct PreparedNpcBlockWork {
    pub(crate) edit: mc_script::precommit::BuildEdit,
    pub(crate) images:
        BTreeMap<mc_script::ScriptInventoryEndpoint, crate::plugin_inventories::InventoryImage>,
    pub(crate) changes: Vec<mc_script::ScriptItemChange>,
    pub(crate) cursor: u32,
    pub(crate) next_cursor: u32,
}

#[allow(
    clippy::large_enum_variant,
    reason = "moving frozen plans avoids per-request boxing"
)]
pub(crate) enum NpcBlockBuild {
    Direct,
    Prepare,
    Pending,
    Ready {
        plan: PreparedNpcBlockWork,
        approval: mc_script::precommit::Approval,
    },
}

struct PendingNpcBuild {
    owner: String,
    handle: String,
    work_revision: u64,
    entity_uuid: uuid::Uuid,
    dimension: crate::dimensions::Dimension,
    plan: NpcBuildPlan,
    decision: mc_script::precommit::PendingDecision,
}

#[derive(Debug, PartialEq)]
struct PreparedWorldBlockSet {
    dimension: crate::dimensions::Dimension,
    position: crate::boundary::BlockPos,
    previous: u32,
    proposed: u32,
}

struct PendingWorldBlockSet {
    command: mc_script::AdmittedScriptCommand,
    plan: PreparedWorldBlockSet,
    decision: mc_script::precommit::PendingDecision,
}

pub(crate) struct PluginRuntime {
    host: Option<mc_plugin_host::host::PluginHost>,
    pub(crate) boundary: mc_script::ScriptBoundary,
    sessions: Arc<Sessions>,
    roots: BTreeMap<String, PathBuf>,
    commands: Arc<[(String, u8)]>,
    /// The one package whose manifest declared `death_graves`, when a deployment
    /// did. The native death path reads it every death, so it is resolved once at
    /// startup and never per tick.
    pub(crate) death_graves: Option<String>,
    /// Auth-capable packages are required while the native login fence is enabled.
    /// Retain their deployment identities even after the host unregisters them.
    pub(crate) required_auth_plugins: Vec<String>,
    catalogs: crate::plugin_structures::Catalogs,
    pub(crate) stats: Arc<PluginStats>,
    pending_changes: Vec<(crate::boundary::ConnectionId, crate::boundary::PlayerChange)>,
    pending_traces: Vec<ExecutionTrace>,
    observed_players: BTreeMap<u64, mc_script::ScriptPlayerContext>,
    pub(crate) pending_events: Vec<mc_script::ScriptEvent>,
    committed_events: std::collections::VecDeque<CommittedEvents>,
    pending_structure_builds: std::collections::VecDeque<PendingStructureBuild>,
    pending_npc_builds: Vec<PendingNpcBuild>,
    pending_world_block_sets: std::collections::VecDeque<PendingWorldBlockSet>,
    pending_build_commands: usize,
    written_clock: Option<u64>,
    delivery_failed: bool,
}

/// The one package whose deployment declared native death-grave custody.
///
/// Two packages declaring it would be two owners of one death's drops, so the
/// deployment is refused instead of picking one.
fn death_graves_owner(
    packages: &[mc_plugin_host::LoadedPackage],
) -> Result<Option<String>, CoreError> {
    let mut owner: Option<String> = None;
    for package in packages {
        if !package
            .requested_capabilities()
            .iter()
            .any(|capability| capability == "death_graves")
        {
            continue;
        }
        if owner.is_some() {
            return Err(CoreError::Plugin(
                "more than one package declares the death_graves capability".into(),
            ));
        }
        owner = Some(package.manifest().plugin_id().to_owned());
    }
    Ok(owner)
}

impl PluginRuntime {
    pub(crate) fn open(config: &CoreConfig) -> Result<Option<Self>, CoreError> {
        let Some(directory) = &config.plugin_directory else {
            return Ok(None);
        };
        let limits = mc_plugin_host::PluginLimits::default();
        let packages = mc_plugin_host::discover_directory(
            directory.clone(),
            &limits,
            &[mc_script::precommit::HookKind::Build],
        )
        .map_err(|e| CoreError::Plugin(e.to_string()))?
        .into_packages();
        let roots = packages
            .iter()
            .map(|p| (p.manifest().plugin_id().into(), p.root().to_owned()))
            .collect();
        let death_graves = death_graves_owner(&packages)?;
        let required_auth_plugins = if config.require_plugin_login {
            let providers: Vec<String> = packages
                .iter()
                .filter(|package| {
                    package
                        .requested_capabilities()
                        .iter()
                        .any(|capability| capability == "auth")
                })
                .map(|package| package.manifest().plugin_id().to_owned())
                .collect();
            if providers.is_empty() {
                return Err(CoreError::Plugin(
                    "plugin login requires an auth-capable package; repair the deployment and restart"
                        .into(),
                ));
            }
            providers
        } else {
            Vec::new()
        };
        let sessions = Arc::new(Sessions::default());
        let queues = mc_plugin_host::host::HostQueues {
            defer_initial_timer_clock: true,
            ..Default::default()
        };
        let host =
            mc_plugin_host::host::start_deployment(packages, limits, queues, sessions.clone())
                .map_err(|e| CoreError::Plugin(e.to_string()))?;
        if let Some((owner, refusal)) = host.contribution().refusal() {
            let message = format!("plugin {owner} startup contribution refused: {refusal}");
            host.stop();
            return Err(CoreError::Plugin(message));
        }
        let boundary = host.boundary().clone();
        let commands = command_catalog(&boundary);
        let runtime = Self {
            host: Some(host),
            boundary,
            sessions,
            roots,
            commands,
            death_graves,
            required_auth_plugins,
            stats: Arc::new(PluginStats::default()),
            pending_changes: Vec::new(),
            pending_traces: Vec::new(),
            observed_players: BTreeMap::new(),
            catalogs: Default::default(),
            pending_events: Vec::new(),
            committed_events: Default::default(),
            pending_structure_builds: Default::default(),
            pending_npc_builds: Vec::new(),
            pending_world_block_sets: Default::default(),
            pending_build_commands: 0,
            written_clock: None,
            delivery_failed: false,
        };
        Ok(Some(runtime))
    }

    #[cfg(test)]
    pub(crate) fn with_test_boundary(boundary: mc_script::ScriptBoundary) -> Self {
        let commands = command_catalog(&boundary);
        Self {
            host: None,
            boundary,
            sessions: Arc::new(Sessions::default()),
            roots: BTreeMap::new(),
            commands,
            death_graves: None,
            required_auth_plugins: Vec::new(),
            catalogs: Default::default(),
            stats: Arc::new(PluginStats::default()),
            pending_changes: Vec::new(),
            pending_traces: Vec::new(),
            observed_players: BTreeMap::new(),
            pending_events: Vec::new(),
            committed_events: Default::default(),
            pending_structure_builds: Default::default(),
            pending_npc_builds: Vec::new(),
            pending_world_block_sets: Default::default(),
            pending_build_commands: 0,
            written_clock: None,
            delivery_failed: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_test_catalogs(&mut self, catalogs: crate::plugin_structures::Catalogs) {
        self.catalogs = catalogs;
    }

    fn queue_change(
        &mut self,
        connection: crate::boundary::ConnectionId,
        change: crate::boundary::PlayerChange,
    ) {
        if self.pending_changes.len() < mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY {
            self.pending_changes.push((connection, change));
        } else {
            self.delivery_failed = true;
        }
    }

    pub(crate) fn commands(&self) -> Arc<[(String, u8)]> {
        Arc::clone(&self.commands)
    }

    pub(crate) fn load_catalogs(
        &mut self,
        registry: &crate::registry::CoreRegistry,
    ) -> Result<(), CoreError> {
        self.catalogs = crate::plugin_structures::load_catalog(&self.roots, &registry.blocks)?;
        Ok(())
    }

    pub(crate) fn blueprint(&self, owner: &str, id: &str) -> Option<&Arc<mc_worldgen::Blueprint>> {
        self.catalogs.get(owner)?.get(id)
    }

    pub(crate) fn client_sound_allowed(
        &self,
        owner: &str,
        _sound: &mc_script::ScriptClientSound,
    ) -> bool {
        self.client_bundles().iter().any(|bundle| {
            bundle.owner_plugin_id() == owner
                && bundle
                    .content()
                    .contains(&mc_script::ClientContentKind::Sounds)
                && bundle
                    .permissions()
                    .contains(&mc_script::ClientPermission::PlaySounds)
        })
    }

    pub(crate) fn client_view_allowed(&self, owner: &str) -> bool {
        self.host.as_ref().is_none_or(|host| {
            host.client_bundles().iter().any(|bundle| {
                bundle.owner_plugin_id() == owner
                    && bundle
                        .content()
                        .contains(&mc_script::ClientContentKind::Views)
                    && bundle
                        .permissions()
                        .contains(&mc_script::ClientPermission::PresentViews)
            })
        })
    }

    pub(crate) fn contribution(&self) -> &mc_plugin_host::startup::DeploymentContribution {
        self.host.as_ref().expect("running host").contribution()
    }

    pub(crate) fn ore_profile(&self) -> Option<mc_script::PluginWorldgenOreProfile> {
        self.host
            .as_ref()
            .expect("running host")
            .worldgen_ore_profile()
    }

    pub(crate) fn client_bundles(&self) -> &[mc_script::ClientBundle] {
        self.host.as_ref().expect("running host").client_bundles()
    }
}

impl Drop for PluginRuntime {
    fn drop(&mut self) {
        if let Some(host) = self.host.take() {
            host.stop();
        }
    }
}

fn operation_precheck(
    state: &TickState,
    owner: &str,
    request: &mc_script::ScriptOperationRequest,
) -> Option<ScriptOperationOutcome> {
    use mc_script::ScriptOperation;
    if let ScriptOperation::Status { operation_id } = request.operation() {
        return Some(
            state
                .plugins
                .receipts
                .get(owner)
                .and_then(|r| r.get(operation_id))
                .map(|r| r.outcome.clone())
                .unwrap_or_else(|| {
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound)
                }),
        );
    }
    if let Some(id) = request.operation_id()
        && let Some(receipt) = state.plugins.receipts.get(owner).and_then(|r| r.get(id))
    {
        let same = receipt.operation.operation() == request.operation()
            && receipt.operation.expected_plugin_storage_versions()
                == request.expected_plugin_storage_versions();
        return Some(if same {
            receipt.outcome.clone()
        } else {
            ScriptOperationOutcome::rejected(ScriptOperationFailure::OperationConflict)
        });
    }
    if request
        .expected_plugin_storage_versions()
        .iter()
        .any(|guard| state.plugins.version(owner, &guard.key).unwrap_or(0) != guard.version)
    {
        return Some(ScriptOperationOutcome::rejected(
            ScriptOperationFailure::StaleRevision,
        ));
    }
    if request.operation_id().is_some()
        && state.plugins.revision >= mc_script::MAX_SCRIPT_WORLD_TIME
    {
        return Some(ScriptOperationOutcome::rejected(
            ScriptOperationFailure::RuntimeUnavailable,
        ));
    }
    None
}

pub(crate) fn operation(
    state: &mut TickState,
    owner: &str,
    request: &mc_script::ScriptOperationRequest,
) -> ScriptOperationOutcome {
    use mc_script::ScriptOperation;
    if let Some(outcome) = operation_precheck(state, owner, request) {
        return outcome;
    }
    let revision = state.plugins.revision;
    let outcome = match request.operation() {
        ScriptOperation::StorageBatch { mutations, .. } => {
            if !state.plugins.check_storage(owner, mutations) {
                ScriptOperationOutcome::rejected(ScriptOperationFailure::StaleRevision)
            } else {
                let changes = state.plugins.apply_storage(owner, mutations);
                ScriptOperationOutcome::committed(
                    state.plugins.revision,
                    ScriptOperationPayload::StorageBatch { changes },
                )
                .expect("validated storage batch")
            }
        }
        ScriptOperation::StorageScan {
            prefix,
            cursor,
            limit,
        } => {
            let records = state.plugins.storage.get(owner);
            let mut records = records
                .into_iter()
                .flat_map(|records| records.iter())
                .filter(|(key, record)| {
                    key.starts_with(prefix)
                        && record.value.is_some()
                        && cursor.as_ref().is_none_or(|cursor| *key > cursor)
                });
            let entries: Vec<_> = records
                .by_ref()
                .take(usize::from(*limit))
                .map(|(key, record)| {
                    ScriptStorageEntry::new(
                        key.clone(),
                        record.value.clone().expect("live value"),
                        record.revision,
                    )
                })
                .collect();
            let cursor = records
                .next()
                .and_then(|_| entries.last().map(|e| e.key.clone()));
            ScriptOperationOutcome::committed(
                state.plugins.revision,
                ScriptOperationPayload::StoragePage { entries, cursor },
            )
            .expect("validated storage page")
        }
        ScriptOperation::Npc { .. }
        | ScriptOperation::NpcOrder { .. }
        | ScriptOperation::NpcRoute { .. } => crate::plugin_npcs::execute(state, owner, request),
        ScriptOperation::Inventory { .. } => {
            crate::plugin_inventories::execute(state, owner, request)
        }
        ScriptOperation::WorldStructure { .. } => {
            crate::plugin_structures::execute(state, owner, request)
        }
        ScriptOperation::SettlementQuery { .. } | ScriptOperation::Settlement { .. } => {
            crate::plugin_settlements::execute(state, owner, request)
        }
        _ => ScriptOperationOutcome::rejected(ScriptOperationFailure::RuntimeUnavailable),
    };
    if let Some(id) = request.operation_id() {
        if state.plugins.revision == revision {
            state.plugins.revision += 1;
        }
        state
            .plugins
            .receipts
            .entry(owner.into())
            .or_default()
            .insert(
                id.into(),
                OperationReceipt {
                    operation: request.clone(),
                    outcome: outcome.clone(),
                },
            );
        state.plugins.dirty = true;
        mark_key(&mut state.plugins.dirty_receipts, owner, id);
    }
    outcome
}

pub(crate) fn journal(state: &mut TickState, entries: &mut Vec<crate::journal::JournalEntry>) {
    if state.dimension == crate::dimensions::Dimension::Overworld && state.plugins.dirty {
        entries.push(crate::journal::JournalEntry::Plugins {
            value: Box::new(state.plugins.take_delta()),
        });
    }
}

pub(crate) fn validate_startup(state: &mut TickState, fresh: bool) -> Result<(), CoreError> {
    let identity = crate::plugin_startup::world_identity(&state.registry)?;
    if let Some(record) = state.native_globals.get("plugin_startup") {
        if record.version != 1 || record.captured_tick > state.tick || record.payload != identity {
            return Err(CoreError::Plugin(
                "native world plugin startup policy differs from its checkpoint".into(),
            ));
        }
    } else {
        if !fresh {
            return Err(CoreError::Plugin(
                "native world is missing its required plugin startup identity".into(),
            ));
        }
        state.native_globals.insert(
            "plugin_startup".into(),
            crate::native_checkpoint::record(state.tick, identity),
        );
    }
    state.plugins.validate()?;
    crate::plugin_npcs::validate(state)?;
    crate::plugin_inventories::validate(state)?;
    crate::plugin_structures::validate(state)?;
    crate::physical_attacks::validate_restore(state)?;
    Ok(())
}

pub(crate) fn player_context(
    state: &TickState,
    connection: crate::boundary::ConnectionId,
    operator: bool,
) -> Option<mc_script::ScriptPlayerContext> {
    let entity = state.connections.entity_of(connection)?;
    let slot = state.entities.slot_of_any(entity)?;
    let uuid = state.connections.uuid_of(connection)?;
    let name = state.connections.name_of(entity)?;
    let [x, y, z] = state.entities.pos(slot);
    mc_script::ScriptPlayerContext::try_new(
        uuid.to_string(),
        name,
        operator,
        state.dimension.name(),
        x,
        y,
        z,
    )
    .ok()
}

/// A command's identity and pose come from the native connection/entity owner,
/// never from the command text or a host-maintained synthetic player.
pub(crate) fn route_command(
    state: &mut TickState,
    connection: crate::boundary::ConnectionId,
    command: &str,
    permission: u8,
) -> bool {
    let Some(runtime) = state.plugin_runtime.as_ref() else {
        return false;
    };
    let Some(root) = command
        .trim_start()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
    else {
        return false;
    };
    if runtime
        .commands
        .binary_search_by(|(name, _)| name.as_str().cmp(root))
        .is_err()
    {
        return false;
    }
    let Some(context) = player_context(state, connection, permission >= 2) else {
        state.push_player(
            connection,
            crate::boundary::PlayerChange::Chat {
                message: "Plugin command admission is unavailable.".into(),
            },
        );
        return true;
    };
    {
        let mut sessions = runtime.sessions.0.write();
        sessions.insert(context.uuid().into(), connection.0);
        sessions.insert(context.username().into(), connection.0);
    }
    let admission = runtime.boundary.prepare_player_command_with_context(
        mc_script::ScriptPlayerId::new(connection.0),
        context,
        command,
    );
    match admission {
        // The immutable deployment catalog reserves roots even after retirement.
        // NotOwned from the live routes must never fall through to native parsing.
        Ok(mc_script::PreparedPlayerCommand::Event(event)) => {
            enqueue_result(state, Some(event));
            true
        }
        other => {
            let message = match other {
                Ok(mc_script::PreparedPlayerCommand::PermissionDenied) => {
                    "You do not have permission to use this plugin command."
                }
                _ => "Plugin command admission is unavailable.",
            };
            state.push_player(
                connection,
                crate::boundary::PlayerChange::Chat {
                    message: message.into(),
                },
            );
            true
        }
    }
}

fn player_world_mut(
    state: &mut TickState,
    connection: crate::boundary::ConnectionId,
) -> Option<&mut TickState> {
    if state.connections.entity_of(connection).is_some() {
        return Some(state);
    }
    state
        .other_dimensions
        .values_mut()
        .find(|world| world.connections.entity_of(connection).is_some())
        .map(|world| &mut **world)
}

pub(crate) fn teleport_player(
    state: &mut TickState,
    request: &mc_script::ScriptPlayerTeleportRequest,
) -> Option<mc_script::ScriptPlayerTeleportFailure> {
    let connection = crate::boundary::ConnectionId(request.player_id().value());
    let Some(world) = player_world_mut(state, connection) else {
        return Some(mc_script::ScriptPlayerTeleportFailure::PlayerUnavailable);
    };
    let Some(entity) = world.connections.entity_of(connection) else {
        return Some(mc_script::ScriptPlayerTeleportFailure::PlayerUnavailable);
    };
    let Some(slot) = world.entities.slot_of(entity) else {
        return Some(mc_script::ScriptPlayerTeleportFailure::PlayerUnavailable);
    };
    if world.dimension.name() != request.dimension() {
        return Some(mc_script::ScriptPlayerTeleportFailure::DimensionMismatch);
    }
    let destination = request.position();
    let position = [destination.x(), destination.y(), destination.z()];
    crate::mounts::dismount(world, entity);
    world.entities.set_pos(slot, position);
    world.entities.set_ground(slot, false);
    if let Some(player) = world.players.get_mut(&entity) {
        player.fall_distance = 0.0;
    }
    let [yaw, pitch, _] = world.entities.rot(slot);
    world.correct_position(connection, position, yaw, pitch);
    None
}

pub(crate) fn hook_operation_failure(
    failure: mc_script::precommit::HookFailure,
) -> ScriptOperationFailure {
    use mc_script::precommit::HookFailure;
    match failure {
        HookFailure::Cancelled => ScriptOperationFailure::Forbidden,
        HookFailure::Stale => ScriptOperationFailure::StaleRevision,
        HookFailure::QueueFull => ScriptOperationFailure::Busy,
        _ => ScriptOperationFailure::RuntimeUnavailable,
    }
}

fn execute_operation_command(
    state: &mut TickState,
    admitted: mc_script::AdmittedScriptCommand,
) -> Option<mc_script::ScriptEvent> {
    let mc_script::ScriptCommand::Operation { request } = admitted.request() else {
        unreachable!("operation dispatch");
    };
    let owner = admitted.plugin_id();
    let boundary = &state
        .plugin_runtime
        .as_ref()
        .expect("runtime retained")
        .boundary;
    if !boundary.has_precommit_hooks(mc_script::precommit::HookKind::Build)
        || !matches!(
            request.operation(),
            mc_script::ScriptOperation::WorldStructure {
                operation: mc_script::ScriptWorldStructureOperation::AdvanceStructure { .. }
            }
        )
    {
        let outcome = operation(state, owner, request);
        return admitted.operation_result(outcome).ok();
    }
    if let Some(outcome) = operation_precheck(state, owner, request) {
        return admitted.operation_result(outcome).ok();
    }
    let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
    let at_capacity =
        runtime.pending_build_commands >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS;
    if let Some(pending) = runtime.pending_structure_builds.iter_mut().find(|pending| {
        let original = &pending.commands[0];
        let mc_script::ScriptCommand::Operation {
            request: original_request,
        } = original.request()
        else {
            unreachable!("pending operation");
        };
        original.plugin_id() == owner && original_request.operation_id() == request.operation_id()
    }) {
        let mc_script::ScriptCommand::Operation { request: original } =
            pending.commands[0].request()
        else {
            unreachable!("pending operation");
        };
        if original.operation() != request.operation()
            || original.expected_plugin_storage_versions()
                != request.expected_plugin_storage_versions()
        {
            return admitted
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::OperationConflict,
                ))
                .ok();
        }
        if at_capacity {
            return admitted
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::Busy,
                ))
                .ok();
        }
        pending.commands.push(admitted);
        runtime.pending_build_commands += 1;
        return None;
    }
    if at_capacity {
        return admitted
            .operation_result(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::Busy,
            ))
            .ok();
    }
    let plan = match crate::plugin_structures::prepare_build(state, owner, request) {
        Ok(Some(plan)) => plan,
        Ok(None) => {
            let outcome = operation(state, owner, request);
            return admitted.operation_result(outcome).ok();
        }
        Err(_) => {
            let outcome = operation(state, owner, request);
            return admitted.operation_result(outcome).ok();
        }
    };
    let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
    match runtime.boundary.begin_precommit(plan.context.clone()) {
        Ok(decision) => {
            runtime
                .pending_structure_builds
                .push_back(PendingStructureBuild {
                    commands: vec![admitted],
                    plan,
                    decision,
                });
            runtime.pending_build_commands += 1;
            None
        }
        Err(failure) => admitted
            .operation_result(ScriptOperationOutcome::rejected(hook_operation_failure(
                failure,
            )))
            .ok(),
    }
}

pub(crate) fn pump(state: &mut TickState) {
    let Some(boundary) = state
        .plugin_runtime
        .as_ref()
        .map(|runtime| runtime.boundary.clone())
    else {
        return;
    };
    poll_structure_builds(state);
    retire_inactive_npc_builds(state);
    poll_world_block_sets(state);
    for _ in 0..mc_script::MAX_SCRIPT_COMMAND_QUEUE_CAPACITY {
        let Some(command) = boundary.try_recv_command() else {
            break;
        };
        let admitted = match boundary.accept_host_command(command) {
            Ok(command) => command,
            Err(error) => {
                tracing::error!(?error, "plugin command provenance rejected");
                continue;
            }
        };
        let owner = admitted.plugin_id().to_owned();
        let (session, request) = command_identity(admitted.request());
        state
            .plugin_runtime
            .as_ref()
            .expect("runtime retained")
            .stats
            .operations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let result = execute(state, admitted);
        let outcome =
            match result.as_ref().map(mc_script::ScriptEvent::kind) {
                Some(mc_script::ScriptEventKind::OperationResult { outcome, .. }) => {
                    outcome.state().as_str()
                }
                Some(mc_script::ScriptEventKind::PluginStorageCasResult { applied, .. }) => {
                    if *applied {
                        "committed"
                    } else {
                        "stale_revision"
                    }
                }
                Some(mc_script::ScriptEventKind::InventoryStorageTransactionResult {
                    committed,
                    ..
                }) => {
                    if *committed {
                        "committed"
                    } else {
                        "rejected"
                    }
                }
                Some(mc_script::ScriptEventKind::PluginStorageDeleteResult { deleted, .. }) => {
                    if *deleted {
                        "committed"
                    } else {
                        "stale_revision"
                    }
                }
                Some(mc_script::ScriptEventKind::PlayerInventoryTransactionResult {
                    failure,
                    ..
                }) => {
                    if failure.is_none() {
                        "committed"
                    } else {
                        "rejected"
                    }
                }
                Some(mc_script::ScriptEventKind::PlayerTeleportResult { failure, .. }) => {
                    if failure.is_none() {
                        "committed"
                    } else {
                        "rejected"
                    }
                }
                Some(mc_script::ScriptEventKind::WorldTimeSetResult { failure, .. }) => {
                    if failure.is_none() {
                        "committed"
                    } else {
                        "rejected"
                    }
                }
                Some(mc_script::ScriptEventKind::WorldBlockSetResult { applied, .. }) => {
                    if *applied { "committed" } else { "rejected" }
                }
                Some(mc_script::ScriptEventKind::EntitySpawnResult { failure, .. }) => {
                    if failure.is_none() {
                        "committed"
                    } else {
                        "rejected"
                    }
                }
                Some(mc_script::ScriptEventKind::EntityDamageResult { failure, .. }) => {
                    if failure.is_none() {
                        "committed"
                    } else {
                        "rejected"
                    }
                }
                Some(mc_script::ScriptEventKind::LoaderItemGrantResult { failure, .. }) => {
                    if failure.is_none() {
                        "committed"
                    } else {
                        "rejected"
                    }
                }
                Some(
                    mc_script::ScriptEventKind::ClientViewOpened { failure, .. }
                    | mc_script::ScriptEventKind::ClientSelectionStarted { failure, .. },
                ) => {
                    if failure.is_none() {
                        "committed"
                    } else {
                        "rejected"
                    }
                }
                Some(mc_script::ScriptEventKind::ZoneCommandResult { accepted, .. }) => {
                    if *accepted { "committed" } else { "rejected" }
                }
                _ => "admitted",
            };
        let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
        runtime.pending_traces.push(ExecutionTrace {
            owner,
            session,
            request,
            outcome,
        });
        if let Some(event) = result {
            runtime.pending_events.push(event);
        }
    }
}

fn execute(
    state: &mut TickState,
    admitted: mc_script::AdmittedScriptCommand,
) -> Option<mc_script::ScriptEvent> {
    use crate::boundary::{ConnectionId, PlayerChange};
    use mc_script::ScriptCommand;
    let owner = admitted.plugin_id().to_owned();
    match admitted.request() {
        ScriptCommand::PluginStorageGet { request } => {
            let record = state.plugins.record(&owner, request.key());
            let value = record.and_then(|r| r.value.as_deref()).map(str::to_owned);
            let version = record.filter(|r| r.value.is_some()).map(|r| r.revision);
            admitted
                .plugin_storage_get_result(value.as_deref(), version)
                .ok()
        }
        ScriptCommand::PluginStorageCompareAndSwap { request } => {
            let key = request.key().to_owned();
            let mutation = ScriptStorageMutation::compare_and_swap(
                &key,
                request.expected_version(),
                request.value(),
            )
            .expect("admitted storage request");
            let applied = state
                .plugins
                .check_storage(&owner, std::slice::from_ref(&mutation));
            if applied {
                state.plugins.apply_storage(&owner, &[mutation]);
            }
            let version = state.plugins.version(&owner, &key);
            admitted.plugin_storage_cas_result(applied, version).ok()
        }
        ScriptCommand::PluginStorageDelete { request } => {
            let key = request.key().to_owned();
            let mutation = ScriptStorageMutation::delete(&key, request.expected_version())
                .expect("admitted storage request");
            let deleted = state
                .plugins
                .check_storage(&owner, std::slice::from_ref(&mutation));
            if deleted {
                state.plugins.apply_storage(&owner, &[mutation]);
            }
            let version = state.plugins.version(&owner, &key);
            admitted.plugin_storage_delete_result(deleted, version).ok()
        }
        ScriptCommand::Operation { .. } => execute_operation_command(state, admitted),
        ScriptCommand::SendChatMessage { player_id, message } => {
            let connection = ConnectionId(player_id.value());
            if player_world_mut(state, connection).is_some() {
                state
                    .plugin_runtime
                    .as_mut()
                    .expect("runtime retained")
                    .queue_change(
                        connection,
                        PlayerChange::Chat {
                            message: message.clone(),
                        },
                    );
            }
            None
        }
        ScriptCommand::BroadcastChatMessage { message } => {
            let TickState {
                connections,
                other_dimensions,
                plugin_runtime,
                ..
            } = state;
            let runtime = plugin_runtime.as_mut().expect("runtime retained");
            for connection in connections.all_connections().chain(
                other_dimensions
                    .values()
                    .flat_map(|world| world.connections.all_connections()),
            ) {
                runtime.queue_change(
                    connection,
                    PlayerChange::Chat {
                        message: message.clone(),
                    },
                );
            }
            None
        }
        ScriptCommand::DisconnectPlayer { player_id, reason } => {
            let connection = ConnectionId(player_id.value());
            if let Some(world) = player_world_mut(state, connection) {
                world.push_player(
                    connection,
                    PlayerChange::Disconnect {
                        reason: reason.clone(),
                    },
                );
                world.disconnects.push((connection, world.tick + 1));
            }
            None
        }
        ScriptCommand::TeleportPlayer { request } => {
            let failure = teleport_player(state, request);
            admitted.player_teleport_result(failure).ok()
        }
        ScriptCommand::ListOnlinePlayers { request } => {
            let mut players = Vec::new();
            for world in
                std::iter::once(&*state).chain(state.other_dimensions.values().map(|w| &**w))
            {
                for connection in world.connections.all_connections() {
                    if let Some(context) = player_context(
                        world,
                        connection,
                        world.plugin_players.is_operator(connection),
                    ) && let Ok(player) = mc_script::ScriptOnlinePlayerSnapshot::try_new(
                        mc_script::ScriptPlayerId::new(connection.0),
                        context,
                        world.dimension.name(),
                    ) {
                        players.push(player);
                    }
                }
            }
            players.sort_by_key(|p| p.player_id().value());
            let truncated = players.len() > request.limit();
            players.truncate(request.limit());
            admitted.online_players_result(players, truncated).ok()
        }
        ScriptCommand::SpawnEntity {
            actor,
            entity_type,
            position,
            ..
        } => {
            let type_id = state.registry.entity_type_id(entity_type);
            if type_id.is_some_and(|key| custom_mob_cap_reached(state, key)) {
                return admitted
                    .entity_spawn_result(Some(mc_script::ScriptEntitySpawnFailure::Busy))
                    .ok();
            }
            let failure = if let Some(type_id) = type_id {
                if let Some(world) = player_world_mut(state, ConnectionId(actor.value())) {
                    let at = [position.x(), position.y(), position.z()];
                    match world.registry.blueprint(type_id) {
                        Some(blueprint)
                            if !matches!(
                                blueprint.movement,
                                crate::blueprint::MovementClass::Item
                                    | crate::blueprint::MovementClass::Player
                                    | crate::blueprint::MovementClass::Projectile
                            ) =>
                        {
                            let health = blueprint.params.max_health;
                            let clear = crate::settlements::allocator::clear_body(
                                &world.world,
                                &world.registry,
                                at,
                                blueprint.half_width,
                                blueprint.height,
                            );
                            if clear {
                                let entity = world.spawn_mob(type_id, at, health);
                                world.plugin_spawned.push(entity);
                                None
                            } else {
                                Some(mc_script::ScriptEntitySpawnFailure::Busy)
                            }
                        }
                        _ => Some(mc_script::ScriptEntitySpawnFailure::Rejected),
                    }
                } else {
                    Some(mc_script::ScriptEntitySpawnFailure::ActorUnavailable)
                }
            } else {
                Some(mc_script::ScriptEntitySpawnFailure::UnknownEntityType)
            };
            admitted.entity_spawn_result(failure).ok()
        }
        ScriptCommand::DamageEntity { .. } => admitted
            .entity_damage_result(
                None,
                false,
                Some(mc_script::ScriptEntityDamageFailure::RuntimeUnavailable),
            )
            .ok(),
        ScriptCommand::SetWorldTime { request } => {
            state.day_time = request.world_time() as i64;
            admitted.world_time_set_result(None).ok()
        }
        ScriptCommand::SetWorldBlock { .. } => execute_world_block_set(state, admitted),
        _ => crate::plugin_player::execute(state, admitted),
    }
}

/// Native work is charged to the defining namespace across dimensions, regardless
/// of which plugin requested the spawn. Dead rows release credit.
const MAX_CUSTOM_MOBS_PER_PLUGIN: usize = 1024;

fn custom_mob_cap_reached(state: &TickState, key: u32) -> bool {
    if key & crate::blueprint::CUSTOM_TYPE_FLAG == 0 {
        return false;
    }
    let Some(owner) = state
        .registry
        .entity_type_name(key)
        .and_then(|name| name.split_once(':').map(|(owner, _)| owner))
    else {
        return true;
    };
    // Reserve all possible descendants of a sized splitter (size²); each split
    // produces at most four half-sized children and cannot increase this charge.
    let reserve = if state
        .registry
        .blueprint(key)
        .is_some_and(|bp| bp.capabilities.splits_on_death)
    {
        16
    } else {
        1
    };
    let mut used = reserve;
    for world in std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        for slot in world.entities.slots() {
            let body = world.entities.body(slot);
            let crate::entities::EntityKind::Mob { type_id } = body.kind else {
                continue;
            };
            if !body.alive || type_id & crate::blueprint::CUSTOM_TYPE_FLAG == 0 {
                continue;
            }
            let Some(blueprint) = world.registry.blueprint(type_id) else {
                continue;
            };
            if blueprint.name.split_once(':').map(|(owner, _)| owner) != Some(owner) {
                continue;
            }
            used += if blueprint.capabilities.splits_on_death {
                usize::from(world.entities.row(slot).loot.slime_size.unwrap_or(1)).pow(2)
            } else {
                1
            };
            if used > MAX_CUSTOM_MOBS_PER_PLUGIN {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "plugin_sessions_tests.rs"]
mod sessions_tests;

#[cfg(test)]
#[path = "plugin_mob_cap_tests.rs"]
mod mob_cap_tests;
