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
    mc_script::ScriptPlayerContext::try_new(uuid.to_string(), name, operator, x, y, z).ok()
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

/// The tick only checks delivered answers; guest work stays on the host thread.
fn poll_structure_builds(state: &mut TickState) {
    let mut pending = std::mem::take(
        &mut state
            .plugin_runtime
            .as_mut()
            .expect("runtime retained")
            .pending_structure_builds,
    );
    for _ in 0..pending.len() {
        let mut build = pending.pop_front().expect("bounded pending queue");
        let Some(answer) = build.decision.try_resolve() else {
            pending.push_back(build);
            continue;
        };
        let original = &build.commands[0];
        let mc_script::ScriptCommand::Operation { request } = original.request() else {
            unreachable!("pending operation");
        };
        let owner = original.plugin_id();
        let outcome = if let Some(outcome) = operation_precheck(state, owner, request) {
            outcome
        } else {
            match answer {
                Err(failure) => ScriptOperationOutcome::rejected(hook_operation_failure(failure)),
                Ok(mut approval) => {
                    match crate::plugin_structures::prepare_build(state, owner, request) {
                        Ok(Some(current)) if current == build.plan => {
                            match crate::build_precommit::consume_approval(state, &mut approval) {
                                Ok(()) => operation(state, owner, request),
                                Err(failure) => ScriptOperationOutcome::rejected(
                                    hook_operation_failure(failure),
                                ),
                            }
                        }
                        _ => {
                            ScriptOperationOutcome::rejected(ScriptOperationFailure::StaleRevision)
                        }
                    }
                }
            }
        };
        let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
        runtime.pending_build_commands -= build.commands.len();
        let outcome_state = outcome.state().as_str();
        let mut outcome = Some(outcome);
        let mut commands = build.commands.into_iter();
        while let Some(admitted) = commands.next() {
            let owner = admitted.plugin_id().to_owned();
            let (session, request) = command_identity(admitted.request());
            let result = if commands.len() == 0 {
                outcome.take().expect("last operation result")
            } else {
                outcome.as_ref().expect("shared operation result").clone()
            };
            runtime.pending_traces.push(ExecutionTrace {
                owner,
                session,
                request,
                outcome: outcome_state,
            });
            if let Ok(event) = admitted.operation_result(result) {
                runtime.pending_events.push(event);
            }
        }
    }
    state
        .plugin_runtime
        .as_mut()
        .expect("runtime retained")
        .pending_structure_builds = pending;
}

/// NPC work calls this only after its ordinary live work prerequisites passed.
/// `false` leaves progress unchanged while the host owns the queued question.
pub(crate) fn admit_npc_structure_build(
    state: &mut TickState,
    owner: &str,
    handle: &str,
    work_revision: u64,
    entity_uuid: uuid::Uuid,
    operation: &mc_script::ScriptWorldStructureOperation,
) -> Result<bool, ScriptOperationFailure> {
    let Some(runtime) = state.plugin_runtime.as_mut() else {
        return Ok(true);
    };
    if let Some(index) = runtime
        .pending_npc_builds
        .iter()
        .position(|pending| pending.owner == owner && pending.handle == handle)
    {
        let mut pending = runtime.pending_npc_builds.swap_remove(index);
        if pending.work_revision != work_revision
            || pending.entity_uuid != entity_uuid
            || pending.dimension != state.dimension
            || !matches!(&pending.plan, NpcBuildPlan::Structure { operation: original, .. } if original == operation)
        {
            runtime.pending_build_commands -= 1;
            return Err(ScriptOperationFailure::StaleRevision);
        }
        let Some(answer) = pending.decision.try_resolve() else {
            runtime.pending_npc_builds.push(pending);
            return Ok(false);
        };
        runtime.pending_build_commands -= 1;
        let NpcBuildPlan::Structure { plan, .. } = pending.plan else {
            unreachable!("structure identity checked above");
        };
        let mut approval = answer.map_err(hook_operation_failure)?;
        if crate::plugin_structures::prepare_structure_build(state, owner, operation)?
            .is_none_or(|current| current != plan)
        {
            return Err(ScriptOperationFailure::StaleRevision);
        }
        crate::build_precommit::consume_approval(state, &mut approval)
            .map_err(hook_operation_failure)?;
        return Ok(true);
    }
    if !runtime
        .boundary
        .has_precommit_hooks(mc_script::precommit::HookKind::Build)
    {
        return Ok(true);
    }
    if runtime.pending_build_commands >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
        return Err(ScriptOperationFailure::Busy);
    }
    let Some(plan) = crate::plugin_structures::prepare_structure_build(state, owner, operation)?
    else {
        return Ok(true);
    };
    let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
    let decision = runtime
        .boundary
        .begin_precommit(plan.context.clone())
        .map_err(hook_operation_failure)?;
    runtime.pending_npc_builds.push(PendingNpcBuild {
        owner: owner.into(),
        handle: handle.into(),
        work_revision,
        entity_uuid,
        dimension: state.dimension,
        plan: NpcBuildPlan::Structure {
            operation: operation.clone(),
            plan,
        },
        decision,
    });
    runtime.pending_build_commands += 1;
    Ok(false)
}

pub(crate) fn poll_npc_block_build(
    state: &mut TickState,
    owner: &str,
    handle: &str,
    work_revision: u64,
    entity_uuid: uuid::Uuid,
    work: &mc_script::ScriptNpcWorkOrder,
) -> Result<NpcBlockBuild, ScriptOperationFailure> {
    let Some(runtime) = state.plugin_runtime.as_mut() else {
        return Ok(NpcBlockBuild::Direct);
    };
    if let Some(index) = runtime
        .pending_npc_builds
        .iter()
        .position(|pending| pending.owner == owner && pending.handle == handle)
    {
        let mut pending = runtime.pending_npc_builds.swap_remove(index);
        if pending.work_revision != work_revision
            || pending.entity_uuid != entity_uuid
            || pending.dimension != state.dimension
            || !matches!(&pending.plan, NpcBuildPlan::Blocks { work: original, .. } if original == work)
        {
            runtime.pending_build_commands -= 1;
            return Err(ScriptOperationFailure::StaleRevision);
        }
        let Some(answer) = pending.decision.try_resolve() else {
            runtime.pending_npc_builds.push(pending);
            return Ok(NpcBlockBuild::Pending);
        };
        runtime.pending_build_commands -= 1;
        let approval = answer.map_err(hook_operation_failure)?;
        let NpcBuildPlan::Blocks { plan, .. } = pending.plan else {
            unreachable!("physical work identity checked above");
        };
        return Ok(NpcBlockBuild::Ready { plan, approval });
    }
    if !runtime
        .boundary
        .has_precommit_hooks(mc_script::precommit::HookKind::Build)
    {
        return Ok(NpcBlockBuild::Direct);
    }
    if runtime.pending_build_commands >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
        return Err(ScriptOperationFailure::Busy);
    }
    Ok(NpcBlockBuild::Prepare)
}

pub(crate) fn begin_npc_block_build(
    state: &mut TickState,
    owner: &str,
    handle: &str,
    work_revision: u64,
    entity_uuid: uuid::Uuid,
    work: &mc_script::ScriptNpcWorkOrder,
    plan: PreparedNpcBlockWork,
) -> Result<(), ScriptOperationFailure> {
    let context = mc_script::precommit::BuildContext::try_new(
        mc_script::precommit::HookActor::Plugin(owner.into()),
        state.dimension.name(),
        vec![plan.edit],
    )
    .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    let runtime = state
        .plugin_runtime
        .as_mut()
        .expect("hooked native work runtime");
    if runtime.pending_build_commands >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
        return Err(ScriptOperationFailure::Busy);
    }
    let decision = runtime
        .boundary
        .begin_precommit(mc_script::precommit::HookContext::Build(context))
        .map_err(hook_operation_failure)?;
    runtime.pending_npc_builds.push(PendingNpcBuild {
        owner: owner.into(),
        handle: handle.into(),
        work_revision,
        entity_uuid,
        dimension: state.dimension,
        plan: NpcBuildPlan::Blocks {
            work: work.clone(),
            plan,
        },
        decision,
    });
    runtime.pending_build_commands += 1;
    Ok(())
}

fn retire_inactive_npc_builds(state: &mut TickState) {
    let mut pending = std::mem::take(
        &mut state
            .plugin_runtime
            .as_mut()
            .expect("runtime retained")
            .pending_npc_builds,
    );
    let before = pending.len();
    pending.retain(|build| {
        crate::plugin_npcs::pending_work_is_current(
            state,
            &build.owner,
            &build.handle,
            build.entity_uuid,
            build.work_revision,
            build.dimension,
        )
    });
    let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
    runtime.pending_build_commands -= before - pending.len();
    runtime.pending_npc_builds = pending;
}

fn prepare_world_block_set(
    state: &TickState,
    request: &mc_script::ScriptWorldBlockSetRequest,
) -> Result<PreparedWorldBlockSet, mc_script::ScriptWorldBlockSetFailure> {
    use mc_script::ScriptWorldBlockSetFailure as Failure;
    let dimension = crate::dimensions::Dimension::parse(request.dimension())
        .ok_or(Failure::UnsupportedDimension)?;
    let block = mc_data::Identifier::parse(request.block_id())
        .ok()
        .and_then(|id| state.registry.blocks.block(&id))
        .map(|block| block.default.0)
        .ok_or(Failure::UnknownBlock)?;
    let target = if dimension == state.dimension {
        state
    } else {
        state
            .other_dimensions
            .get(&dimension)
            .ok_or(Failure::UnsupportedDimension)?
    };
    if request.y() < target.dimension.geometry().min_y()
        || request.y() >= target.dimension.geometry().max_y()
    {
        return Err(Failure::OutOfWorld);
    }
    let previous = target
        .world
        .block_at(request.x(), request.y(), request.z())
        .ok_or(Failure::Busy)?;
    let position = crate::boundary::BlockPos {
        x: request.x(),
        y: request.y(),
        z: request.z(),
    };
    if crate::plugin_inventories::has_reservations_for_block(target, position) {
        return Err(Failure::Busy);
    }
    Ok(PreparedWorldBlockSet {
        dimension,
        position,
        previous,
        proposed: block,
    })
}

fn apply_world_block_set(state: &mut TickState, plan: &PreparedWorldBlockSet, plugin: &str) {
    let target = crate::plugin_inventories::world_mut(state, plan.dimension.name())
        .expect("preflight dimension");
    let position = plan.position;
    crate::block_log::note_plugin(target, position, plugin);
    target.stage_command_container_removal(position);
    target
        .world
        .set_block(position.x, position.y, position.z, plan.proposed);
    target.plugin_block_writes.push((position, plan.proposed));
}

fn hook_block_set_failure(
    failure: mc_script::precommit::HookFailure,
) -> mc_script::ScriptWorldBlockSetFailure {
    use mc_script::ScriptWorldBlockSetFailure as Failure;
    use mc_script::precommit::HookFailure;
    match failure {
        HookFailure::Cancelled | HookFailure::Stale => Failure::Rejected,
        HookFailure::QueueFull => Failure::Busy,
        _ => Failure::RuntimeUnavailable,
    }
}

fn execute_world_block_set(
    state: &mut TickState,
    admitted: mc_script::AdmittedScriptCommand,
) -> Option<mc_script::ScriptEvent> {
    let mc_script::ScriptCommand::SetWorldBlock { request } = admitted.request() else {
        unreachable!("block set dispatch");
    };
    let plan = match prepare_world_block_set(state, request) {
        Ok(plan) => plan,
        Err(failure) => return admitted.world_block_set_result(false, Some(failure)).ok(),
    };
    let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
    if !runtime
        .boundary
        .has_precommit_hooks(mc_script::precommit::HookKind::Build)
    {
        apply_world_block_set(state, &plan, admitted.plugin_id());
        return admitted.world_block_set_result(true, None).ok();
    }
    if runtime.pending_build_commands >= mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
        return admitted
            .world_block_set_result(false, Some(mc_script::ScriptWorldBlockSetFailure::Busy))
            .ok();
    }
    let context = mc_script::precommit::BuildContext::try_new(
        mc_script::precommit::HookActor::Plugin(admitted.plugin_id().into()),
        plan.dimension.name(),
        vec![mc_script::precommit::BuildEdit::new(
            plan.position.x,
            plan.position.y,
            plan.position.z,
            plan.previous,
            plan.proposed,
        )],
    )
    .expect("one validated native block edit");
    match runtime
        .boundary
        .begin_precommit(mc_script::precommit::HookContext::Build(context))
    {
        Ok(decision) => {
            runtime
                .pending_world_block_sets
                .push_back(PendingWorldBlockSet {
                    command: admitted,
                    plan,
                    decision,
                });
            runtime.pending_build_commands += 1;
            None
        }
        Err(failure) => admitted
            .world_block_set_result(false, Some(hook_block_set_failure(failure)))
            .ok(),
    }
}

fn poll_world_block_sets(state: &mut TickState) {
    let mut pending = std::mem::take(
        &mut state
            .plugin_runtime
            .as_mut()
            .expect("runtime retained")
            .pending_world_block_sets,
    );
    for _ in 0..pending.len() {
        let mut build = pending.pop_front().expect("bounded pending queue");
        let Some(answer) = build.decision.try_resolve() else {
            pending.push_back(build);
            continue;
        };
        let owner = build.command.plugin_id();
        let mc_script::ScriptCommand::SetWorldBlock { request } = build.command.request() else {
            unreachable!("pending block set");
        };
        let failure = match answer {
            Err(failure) => Some(hook_block_set_failure(failure)),
            Ok(mut approval) => match prepare_world_block_set(state, request) {
                Ok(current) if current == build.plan => {
                    match crate::build_precommit::consume_approval(state, &mut approval) {
                        Ok(()) => {
                            apply_world_block_set(state, &build.plan, owner);
                            None
                        }
                        Err(failure) => Some(hook_block_set_failure(failure)),
                    }
                }
                _ => Some(mc_script::ScriptWorldBlockSetFailure::Rejected),
            },
        };
        let owner = owner.to_owned();
        let (session, request) = command_identity(build.command.request());
        let runtime = state.plugin_runtime.as_mut().expect("runtime retained");
        runtime.pending_build_commands -= 1;
        runtime.pending_traces.push(ExecutionTrace {
            owner,
            session,
            request,
            outcome: if failure.is_none() {
                "committed"
            } else {
                "rejected"
            },
        });
        if let Ok(event) = build
            .command
            .world_block_set_result(failure.is_none(), failure)
        {
            runtime.pending_events.push(event);
        }
    }
    state
        .plugin_runtime
        .as_mut()
        .expect("runtime retained")
        .pending_world_block_sets = pending;
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

/// Seal durable callbacks under the completed tick's WAL batch. Observations
/// have independent per-subscriber shedding queues and never halt authority.
pub(crate) fn seal_events(state: &mut TickState, tick: u64) -> Result<bool, &'static str> {
    let Some(runtime) = state.plugin_runtime.as_mut() else {
        return Ok(false);
    };
    // Zone transitions can also be emitted directly by domain command handlers.
    runtime.pending_events.retain(|event| {
        if observation_event(event) {
            runtime.boundary.enqueue_observation(event.clone());
            false
        } else {
            true
        }
    });
    runtime.boundary.seal_observations(tick);
    let pending =
        runtime.pending_events.len() + runtime.pending_changes.len() + runtime.pending_traces.len();
    let retained: usize = runtime
        .committed_events
        .iter()
        .map(|batch| batch.events.len() + batch.changes.len() + batch.traces.len())
        .sum();
    if runtime.delivery_failed {
        return Err("plugin native authority failed: world halted (fail closed)");
    }
    if retained.saturating_add(pending) > mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY {
        return Err("plugin callback backlog overflow: world halted (fail closed)");
    }
    if pending == 0 {
        return Ok(true);
    }
    runtime.committed_events.push_back(CommittedEvents {
        tick,
        events: std::mem::take(&mut runtime.pending_events).into(),
        changes: std::mem::take(&mut runtime.pending_changes),
        traces: std::mem::take(&mut runtime.pending_traces),
    });
    Ok(true)
}

/// Begin guest lifecycle callbacks only after the restored native clock is known.
pub(crate) fn start_events(state: &mut TickState) -> Result<(), CoreError> {
    let Some(runtime) = state.plugin_runtime.as_mut() else {
        return Ok(());
    };
    runtime
        .boundary
        .try_enqueue_event(mc_script::ScriptEvent::server_tick(state.tick))
        .map_err(|e| CoreError::Plugin(format!("initial native timer clock admission: {e:?}")))?;
    runtime.written_clock = Some(state.tick);
    runtime
        .boundary
        .try_enqueue_event(mc_script::ScriptEvent::server_started())
        .map_err(|e| CoreError::Plugin(format!("server.started admission: {e:?}")))?;
    Ok(())
}

pub(crate) fn acknowledge_events(state: &mut TickState, written_tick: u64) {
    let Some(mut runtime) = state.plugin_runtime.take() else {
        return;
    };
    if runtime
        .written_clock
        .is_none_or(|clock| clock < written_tick)
    {
        // The coalesced tick API drains ordinary events before its pending clock,
        // so it cannot order timer creation callbacks behind this ACK watermark.
        match runtime
            .boundary
            .try_enqueue_event(mc_script::ScriptEvent::server_tick(written_tick))
        {
            Ok(()) => {
                runtime.written_clock = Some(written_tick);
            }
            Err(error) => {
                if error != mc_script::ScriptQueueError::Full
                    && runtime
                        .committed_events
                        .iter()
                        .any(|batch| batch.tick <= written_tick && !batch.events.is_empty())
                {
                    runtime.delivery_failed = true;
                }
                state.plugin_runtime = Some(runtime);
                return;
            }
        }
    }
    while runtime
        .committed_events
        .front()
        .is_some_and(|batch| batch.tick <= written_tick)
    {
        let batch = runtime
            .committed_events
            .front_mut()
            .expect("written event batch");
        for (connection, change) in std::mem::take(&mut batch.changes) {
            if let Some(world) = player_world_mut(state, connection) {
                world.push_player(connection, change);
                runtime
                    .stats
                    .command_results
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        for trace in std::mem::take(&mut batch.traces) {
            tracing::info!(target: "plugin.command_result", plugin_id = %trace.owner, session = ?trace.session,
                tick = batch.tick, request = %trace.request, outcome = trace.outcome, "native plugin command journaled");
        }
        let mut blocked = false;
        while let Some(event) = batch.events.front() {
            match runtime.boundary.try_enqueue_event(event.clone()) {
                Ok(()) => {
                    batch.events.pop_front();
                }
                Err(mc_script::ScriptQueueError::Full) => {
                    blocked = true;
                    break;
                }
                Err(_) => {
                    runtime.delivery_failed = true;
                    blocked = true;
                    break;
                }
            }
        }
        if blocked {
            break;
        }
        runtime.committed_events.pop_front();
    }
    state.plugin_runtime = Some(runtime);
}

fn command_identity(command: &mc_script::ScriptCommand) -> (Option<u64>, String) {
    use mc_script::ScriptCommand;
    match command {
        ScriptCommand::SendChatMessage { player_id, .. } => {
            (Some(player_id.value()), "send-message".into())
        }
        ScriptCommand::PluginStorageGet { request } => (None, request.request_id().into()),
        ScriptCommand::PluginStorageCompareAndSwap { request } => {
            (None, request.request_id().into())
        }
        ScriptCommand::PluginStorageDelete { request } => (None, request.request_id().into()),
        ScriptCommand::Operation { request } => (None, request.request_id().into()),
        ScriptCommand::InventoryStorageTransaction { transaction } => (
            Some(transaction.player_id().value()),
            transaction.id().into(),
        ),
        ScriptCommand::PlayerInventoryTransaction { transaction } => (
            Some(transaction.player_id().value()),
            transaction.request_id().into(),
        ),
        ScriptCommand::SpawnEntity {
            request_id, actor, ..
        } => (Some(actor.value()), request_id.clone()),
        ScriptCommand::DamageEntity { request } => (None, request.request_id().into()),
        ScriptCommand::SetWorldTime { request } => (None, request.request_id().into()),
        ScriptCommand::SetWorldBlock { request } => (None, request.request_id().into()),
        ScriptCommand::TeleportPlayer { request } => (
            Some(request.player_id().value()),
            request.request_id().into(),
        ),
        ScriptCommand::ListOnlinePlayers { request } => (None, request.request_id().into()),
        ScriptCommand::OpenInventoryMenu { player_id, menu } => {
            (Some(player_id.value()), menu.id().into())
        }
        ScriptCommand::CloseInventoryMenu { player_id, menu_id } => {
            (Some(player_id.value()), menu_id.clone())
        }
        ScriptCommand::UnlockSession { player_id } => {
            (Some(player_id.value()), "unlock-session".into())
        }
        ScriptCommand::UpsertZone { zone } => (None, zone.id().into()),
        ScriptCommand::RemoveZone { zone_id } => (None, zone_id.clone()),
        ScriptCommand::ClientSound { player_id, sound } => {
            (Some(player_id.value()), sound.sound_id().into())
        }
        _ => (None, "native-command".into()),
    }
}

fn observation_event(event: &mc_script::ScriptEvent) -> bool {
    use mc_script::ScriptEventKind::*;
    matches!(
        event.kind(),
        PlayerJoined { .. }
            | PlayerLeft { .. }
            | PlayerChat { .. }
            | PlayerBlockBroken { .. }
            | PlayerBlockPlaced { .. }
            | PlayerItemCrafted { .. }
            | PlayerItemPickedUp { .. }
            | PlayerEntityKilled { .. }
            | PlayerEntityInteracted { .. }
            | PlayerDied { .. }
            | PlayerZoneEntered { .. }
            | PlayerZoneExited { .. }
            | ClientBrand { .. }
    )
}

pub(crate) fn enqueue_result(state: &mut TickState, event: Option<mc_script::ScriptEvent>) {
    if let (Some(runtime), Some(event)) = (&mut state.plugin_runtime, event) {
        if observation_event(&event) {
            runtime.boundary.enqueue_observation(event);
        } else if runtime.pending_events.len() < mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY {
            runtime.pending_events.push(event);
        } else {
            runtime.delivery_failed = true;
        }
    }
}

pub(crate) fn observe_input(state: &mut TickState, input: &crate::boundary::PlayerInput) {
    use crate::boundary::PlayerInput;
    if state.plugin_runtime.is_none() {
        return;
    }
    let connection = input.connection();
    match input {
        PlayerInput::Leave { .. } => {
            enqueue_result(
                state,
                Some(mc_script::ScriptEvent::player_left(
                    mc_script::ScriptPlayerId::new(connection.0),
                    "disconnected",
                )),
            );
        }
        PlayerInput::Chat { message, .. } => {
            if let Some(context) = player_context(
                state,
                connection,
                state.plugin_players.is_operator(connection),
            ) {
                enqueue_result(
                    state,
                    Some(mc_script::ScriptEvent::player_chat_with_context(
                        mc_script::ScriptPlayerId::new(connection.0),
                        message,
                        context,
                    )),
                );
            }
        }
        _ => {}
    }
}

/// Refuse only unauthenticated sessions when a required provider disappears.
/// Established players keep their native custody and the world keeps ticking.
fn refuse_unavailable_auth(state: &mut TickState) {
    let Some(runtime) = &state.plugin_runtime else {
        return;
    };
    let Some(plugin) = runtime
        .required_auth_plugins
        .iter()
        .find(|plugin| !runtime.boundary.plugin_is_active(plugin))
    else {
        return;
    };
    let pending: Vec<_> = std::iter::once(&*state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .flat_map(|world| {
            world.locked_sessions.iter().copied().filter(|connection| {
                !world
                    .disconnects
                    .iter()
                    .any(|(sealed, _)| sealed == connection)
            })
        })
        .collect();
    if pending.is_empty() {
        return;
    }
    let reason = format!(
        "Authentication unavailable: auth plugin {plugin} retired; restart required. Contact the server operator."
    );
    for connection in pending {
        if let Some(world) = player_world_mut(state, connection) {
            world.push_player(
                connection,
                crate::boundary::PlayerChange::Disconnect {
                    reason: reason.clone(),
                },
            );
            world.disconnects.push((connection, world.tick));
        }
    }
}

pub(crate) fn observe_players(state: &mut TickState) {
    if state.plugin_runtime.is_none() {
        return;
    }
    refuse_unavailable_auth(state);
    let mut current = BTreeMap::new();
    for world in
        std::iter::once(&*state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        for connection in world.connections.all_connections() {
            if let Some(context) = player_context(
                world,
                connection,
                state.plugin_players.is_operator(connection),
            ) {
                current.insert(connection.0, context);
            }
        }
    }
    let runtime = state.plugin_runtime.as_mut().expect("running host");
    let sessions = current
        .iter()
        .flat_map(|(session, context)| {
            [
                (context.uuid().to_owned(), *session),
                (context.username().to_owned(), *session),
            ]
        })
        .collect();
    // Build and drop maps outside the shared host lookup's write guard.
    let previous = std::mem::replace(&mut *runtime.sessions.0.write(), sessions);
    drop(previous);
    for (&session, context) in &current {
        if !runtime.observed_players.contains_key(&session) {
            runtime.boundary.enqueue_observation(
                mc_script::ScriptEvent::player_joined_with_context(
                    mc_script::ScriptPlayerId::new(session),
                    context.clone(),
                ),
            );
        }
    }
    runtime.observed_players = current;
}

fn game_mode(
    state: &TickState,
    entity: crate::boundary::EntityId,
) -> Option<mc_script::ScriptGameMode> {
    match state.players.get(&entity)?.game_mode {
        crate::commands::GameMode::Survival => Some(mc_script::ScriptGameMode::Survival),
        crate::commands::GameMode::Creative => Some(mc_script::ScriptGameMode::Creative),
        crate::commands::GameMode::Adventure => Some(mc_script::ScriptGameMode::Adventure),
        _ => None,
    }
}

pub(crate) fn player_died(state: &mut TickState, entity: crate::boundary::EntityId) {
    if state.plugin_runtime.is_none() {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        mc_script::ScriptEvent::try_player_died_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            game_mode(state, entity)?,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

/// Publish one native grave to the package that declared `death_graves`.
///
/// The observation is addressed to that package and rides the durable event
/// queue rather than the droppable observation queue: a shed notice would leave a
/// protected-for-nobody grave behind.
pub(crate) fn player_grave_created(
    state: &mut TickState,
    entity: crate::boundary::EntityId,
    grave: &crate::plugin_graves::Buried,
) {
    let Some(owner) = state
        .plugin_runtime
        .as_ref()
        .and_then(|runtime| runtime.death_graves.clone())
    else {
        return;
    };
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        mc_script::ScriptEvent::try_player_grave_created_with_context(
            owner,
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            &grave.dimension,
            grave.position.x,
            grave.position.y,
            grave.position.z,
            grave.items,
            grave.spilled,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

pub(crate) fn block_event(
    state: &mut TickState,
    entity: crate::boundary::EntityId,
    position: crate::boundary::BlockPos,
    block_state: u32,
    placed: bool,
) {
    if state.plugin_runtime.is_none() {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let block = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(block_state))?;
        let mode = game_mode(state, entity)?;
        if placed {
            mc_script::ScriptEvent::try_player_block_placed_with_context(
                mc_script::ScriptPlayerId::new(connection.0),
                context,
                state.dimension.name(),
                block.block.id.as_str(),
                position.x,
                position.y,
                position.z,
                mode,
            )
            .ok()
        } else {
            mc_script::ScriptEvent::try_player_block_broken_with_context(
                mc_script::ScriptPlayerId::new(connection.0),
                context,
                state.dimension.name(),
                block.block.id.as_str(),
                position.x,
                position.y,
                position.z,
                mode,
            )
            .ok()
        }
    })();
    enqueue_result(state, event);
}

pub(crate) fn pickup_event(
    state: &mut TickState,
    entity: crate::boundary::EntityId,
    item_id: u32,
    count: i32,
) {
    if state.plugin_runtime.is_none() || count <= 0 {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let item = state.registry.items.name_of(item_id)?;
        mc_script::ScriptEvent::try_player_item_picked_up_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            item.as_str(),
            count as u64,
            mc_script::ScriptItemPickupSource::ItemEntity,
            game_mode(state, entity)?,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

pub(crate) fn kill_event(
    state: &mut TickState,
    attacker: crate::boundary::EntityId,
    victim: crate::entities::EntitySlot,
) {
    if state.plugin_runtime.is_none() {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(attacker)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let row = state.entities.body(victim);
        let crate::entities::EntityKind::Mob { type_id } = row.kind else {
            return None;
        };
        let entity_type = state.registry.entity_type_name(type_id)?;
        mc_script::ScriptEvent::try_player_entity_killed_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            mc_script::ScriptEntityId::new(row.id.0 as u64),
            entity_type,
            mc_script::ScriptEntityKillSource::Melee,
            game_mode(state, attacker)?,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

pub(crate) fn damage_attacker(
    source: Option<crate::intent::DamageSource>,
) -> Option<crate::boundary::EntityId> {
    match source? {
        crate::intent::DamageSource::Melee { attacker, .. }
        | crate::intent::DamageSource::Mace { attacker, .. }
        | crate::intent::DamageSource::Sweep { attacker, .. } => Some(attacker),
        crate::intent::DamageSource::Projectile { owner, .. } => Some(owner),
        crate::intent::DamageSource::Explosion { kill_credit, .. } => {
            kill_credit.map(|(source, _)| source)
        }
    }
}

pub(crate) fn craft_event(
    state: &mut TickState,
    entity: crate::boundary::EntityId,
    item: u32,
    count: i32,
    menu: crate::inventory::MenuKind,
) {
    if state.plugin_runtime.is_none() || count <= 0 {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(entity)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let item = state.registry.items.name_of(item)?;
        let source = match menu {
            crate::inventory::MenuKind::Player => mc_script::ScriptCraftingSource::Inventory,
            crate::inventory::MenuKind::CraftingTable => {
                mc_script::ScriptCraftingSource::CraftingTable
            }
            _ => return None,
        };
        mc_script::ScriptEvent::try_player_item_crafted_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            item.as_str(),
            count as u64,
            1,
            source,
            game_mode(state, entity)?,
        )
        .ok()
    })();
    enqueue_result(state, event);
}

pub(crate) fn fail_native(state: &mut TickState) {
    if let Some(runtime) = state.plugin_runtime.as_mut() {
        runtime.delivery_failed = true;
    }
}

pub(crate) fn entity_interaction(
    state: &mut TickState,
    actor: crate::boundary::EntityId,
    target: crate::boundary::EntityId,
    hand: crate::boundary::Hand,
    secondary: bool,
    intent: Option<&crate::intent::Intent>,
) {
    if state.plugin_runtime.is_none() {
        return;
    }
    let event = (|| {
        let connection = state.connections.entity_connection(actor)?;
        let context = player_context(
            state,
            connection,
            state.plugin_players.is_operator(connection),
        )?;
        let slot = state.entities.slot_of_any(target)?;
        let type_id = match state.entities.body(slot).kind {
            crate::entities::EntityKind::Player { .. } => crate::entities::player_type_id(),
            crate::entities::EntityKind::Mob { type_id }
            | crate::entities::EntityKind::Item { type_id }
            | crate::entities::EntityKind::ExperienceOrb { type_id } => type_id,
        };
        let entity_type = state.registry.entity_type_name(type_id)?;
        let hand = match hand {
            crate::boundary::Hand::Main => mc_script::ScriptInteractionHand::MainHand,
            crate::boundary::Hand::Off => mc_script::ScriptInteractionHand::OffHand,
        };
        mc_script::ScriptEvent::try_player_entity_interacted_with_context(
            mc_script::ScriptPlayerId::new(connection.0),
            context,
            state.dimension.name(),
            mc_script::ScriptEntityId::new(target.0 as u64),
            entity_type,
            hand,
            secondary,
            game_mode(state, actor)?,
        )
        .ok()
    })();
    let Some(event) = event else {
        return;
    };
    if let Some(intent) = intent {
        if state.plugin_interactions.len() >= mc_script::MAX_SCRIPT_EVENT_QUEUE_CAPACITY {
            fail_native(state);
        } else {
            state
                .plugin_interactions
                .insert((intent.actor, intent.seq), event);
        }
    } else {
        enqueue_result(state, Some(event));
    }
}

#[cfg(test)]
#[path = "plugin_sessions_tests.rs"]
mod sessions_tests;

#[cfg(test)]
#[path = "plugin_mob_cap_tests.rs"]
mod mob_cap_tests;
